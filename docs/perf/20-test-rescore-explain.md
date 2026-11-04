# Test rescore: the score window, not the retained history (#20)

`EXPLAIN (ANALYZE, BUFFERS)` before and after the change, for one chunk of 500 Tests with
2,000 Executions each, in two shapes. This file is the measured record of the change to
`test_execution_window_ids` (`backend/app/attribution.py`) and the per-chunk read of
`rescore` (`backend/app/processing.py`); the PR description carries the same numbers, plans,
seed and worst-case note.

## Fixture

PostgreSQL 17.11, alembic-head schema, two Repos at 90-day retention scale in one database,
`ANALYZE`d, warm cache, `score_window = 50` (the default):

| table | rows | notes |
| --- | --- | --- |
| `test_executions` | 2,000,000 | 1,000 Tests x 2,000 Executions; 500 Tests per Repo |
| `test_runs` | 4,000 | 2,000 per Repo; every Run executes every Test of its Repo |
| `test_cases` | 1,000 | ids 1..500 in Repo A, 501..1000 in Repo B |
| `repos` / `projects` | 2 / 2 | one each per shape; both Default branches `main` |

1 in 16 Executions fails. Each shape is measured as one chunk: all 500 Test ids of one Repo
(`batching.CHUNK` is 1,000, so one statement).

- **Shape 1 - Default-branch Runs plentiful** (`acme/plentiful`): 1,960 of its 2,000 Runs are
  on `main`, 40 on `feat/x`.
- **Shape 2 - Default-branch Runs scarce** (`acme/scarce`): exactly 3 of its 2,000 Runs are
  on `main`; the rest are on feature branches.

## Method

The statements were captured from the running code - the per-chunk read that
`processing.rescore` sends, through SQLAlchemy's `before_cursor_execute` hook - once from the
base commit ("before": the `row_number()` subquery that ranks every retained Execution) and
once from this head ("after": the two `LATERAL ... LIMIT` arms of
`test_execution_window_ids`), and then run as asyncpg prepared statements with the same bind
values the worker sends, so Postgres plans the real query. Three runs each, the last (warm)
plan kept; the same statement is planned in both shapes with the shape's own chunk of ids.
The before and after runs came from the same seeded database, one after the other.

The seeding script is **not committed to the repo** (issue #20 asks for it in the PR
description instead): its full SQL is reproduced under Reproduce below and travels with the
PR.

## Result

| shape | chunk read | before | after | shared buffers before -> after |
| --- | --- | --- | --- | --- |
| 1, Defaults plentiful | 500 Tests x 2,000 Executions | 17,499.3 ms | 1,625.6 ms | 23,080 (+18,066 temp) -> 105,992 |
| 2, Defaults scarce | the same, other Repo | 17,457.6 ms | 18,945.8 ms | 23,070 (+20,017 temp) -> 132,882 |

Shape 1 is the common case: the read gets about 11x faster and touches no whole-history sort
(the before plan spills ~24 MB per worker to disk - the temp buffers; the after plan
quicksorts its 25,500 rows in 3 MB). Shape 2 is the accepted worst case, discussed under its
own heading; it costs about what the before statement costs, without the disk spill. The
triage run's numbers on a bigger seed (1,000 Tests x 2,000 Executions, one chunk) were
63.9 s before, 4.1 s after plentiful, 8.3 s after scarce; there is no fixed time target,
hardware differs. Both trees' plans trip JIT (74-91 functions, 17-36 ms), so the comparison
is not about JIT either way.

## Why it got faster

Before, the read joined `test_executions` to `test_runs`/`projects`/`repos` for **every
retained Execution of the chunk** and only then applied `(rn_all | rn_def) <= 50`: in shape 1
the `Subquery Scan` reports `Rows Removed by Filter: 955,000` out of 1,000,000 joined rows,
under two `WindowAggs` fed by a parallel disk-spilling sort. The cost tracked retention.

After, each arm of the `UNION` stops at the window: the overall arm is a `Limit` over an
`Index Only Scan Backward using ix_exec_case_id` (`rows=50 loops=500`, `Heap Fetches: 0`),
and the Default-branch arm walks the same index newest-first and joins each Execution to its
Run until it has 50 Default rows - in shape 1, `rows=52 loops=500` read to keep 50. So the
shape-1 after-plan reads about `2 x score_window` Executions per Test (about 100 of the
2,000 retained), **not every Execution in retention**. What remains is window-proportional:
the 25,500 surviving ids are fetched by primary key (102,000 of the plan's 105,992 buffers)
and joined to their Runs.

## Shape 2: the accepted worst case

Whether an Execution is on the Default branch lives in `test_runs`/`repos`, not on
`test_executions`, so no index can seek a Test's Default-branch Executions directly. A Test
with fewer than `score_window` Default-branch Executions in retention cannot fill the
Default-branch arm's `LIMIT 50`, so that arm walks **all** of the Test's Executions
newest-first, joining each one until it runs out - and the read costs the same as before the
rewrite. That is exactly what the shape-2 after-plan shows: the same `Limit` + backward
`Index Scan using ix_exec_case_id`, but with `rows=2000 loops=500` beneath a `Limit` that
keeps 3, i.e. all 2,000 Executions of each of the 500 Tests read in that arm. This worst
case is accepted (issue #20, "Accepted worst case"): it applies only to Repos whose Default
branch rarely runs CI, it no longer pays the whole-history window sort and disk spill, and
bounding it exactly would mean copying the Run's branch onto every Execution - a new column
and index on the largest table, excluded by the issue's non-goals.

## The two reads return a different number of rows

45,000 vs 25,500 in shape 1 (26,500 vs 26,000 in shape 2). The old
`(rn_all | rn_def) <= window` filter also admitted the newest `window` off-Default rows that
lay beyond the overall window (in shape 1: the newest 40 `feat/x` rows per Test, because the
`rn_def` window function partitioned by the Default verdict and numbered them 1..40).
Nothing consumed them: `scoring.branch_scoped_score` slices flips to `window` itself, takes
Proven flakes and the dominant category from the overall window, and both streaks read
Default-branch rows only. The scores do not change: the read difference is pinned by
`test_rescore_matches_branch_scoped_score_over_all_rows` (`backend/tests/test_processing.py`
- the same score, confirmed-flake count and streaks as `branch_scoped_score` over all rows)
and `test_window_failures_counts_only_the_newest_window`
(`backend/tests/test_github.py` - for the `_window_failures` twin of this read).

## The plans

Each block is the whole `EXPLAIN (ANALYZE, BUFFERS)` output, with the captured statement and
its binds on top.

### before - base commit: shape 1 (Defaults plentiful)

`rescore`, one chunk of 500 Tests x 2,000 Executions, the `row_number()` subquery; the
`IN` list carries the 500 Test ids as single binds:

<details>
<summary>full plan</summary>

```
-- statement: SELECT anon_1.test_case_id, anon_1.commit_sha, anon_1.branch, anon_1.status, anon_1.failure_category, anon_1.default_branch FROM (SELECT test_executions.id AS id, test_executions.test_case_id AS test_case_id, test_executions.status AS status, test_executions.failure_category AS failure_category, test_runs.commit_sha AS commit_sha, test_runs.branch AS branch, repos.default_branch AS default_branch, row_number() OVER (PARTITION BY test_executions.test_case_id ORDER BY test_executions.id DESC) AS rn_all, row_number() OVER (PARTITION BY test_executions.test_case_id, repos.default_branch IS NULL OR test_runs.branch = repos.default_branch ORDER BY test_executions.id DESC) AS rn_def FROM test_executions JOIN test_runs ON test_runs.id = test_executions.test_run_id JOIN projects ON projects.id = test_runs.project_id JOIN repos ON repos.id = projects.repo_id WHERE test_executions.test_case_id IN ($3::INTEGER, $4::INTEGER, $5::INTEGER, $6::INTEGER, $7::INTEGER, $8::INTEGER, $9::INTEGER, $10::INTEGER, $11::INTEGER, $12::INTEGER, $13::INTEGER, $14::INTEGER, $15::INTEGER, $16::INTEGER, $17::INTEGER, $18::INTEGER, $19::INTEGER, $20::INTEGER, $21::INTEGER, $22::INTEGER, $23::INTEGER, $24::INTEGER, $25::INTEGER, $26::INTEGER, $27::INTEGER, $28::INTEGER, $29::INTEGER, $30::INTEGER, $31::INTEGER, $32::INTEGER, $33::INTEGER, $34::INTEGER, $35::INTEGER, $36::INTEGER, $37::INTEGER, $38::INTEGER, $39::INTEGER, $40::INTEGER, $41::INTEGER, $42::INTEGER, $43::INTEGER, $44::INTEGER, $45::INTEGER, $46::INTEGER, $47::INTEGER, $48::INTEGER, $49::INTEGER, $50::INTEGER, $51::INTEGER, $52::INTEGER, $53::INTEGER, $54::INTEGER, $55::INTEGER, $56::INTEGER, $57::INTEGER, $58::INTEGER, $59::INTEGER, $60::INTEGER, $61::INTEGER, $62::INTEGER, $63::INTEGER, $64::INTEGER, $65::INTEGER, $66::INTEGER, $67::INTEGER, $68::INTEGER, $69::INTEGER, $70::INTEGER, $71::INTEGER, $72::INTEGER, $73::INTEGER, $74::INTEGER, $75::INTEGER, $76::INTEGER, $77::INTEGER, $78::INTEGER, $79::INTEGER, $80::INTEGER, $81::INTEGER, $82::INTEGER, $83::INTEGER, $84::INTEGER, $85::INTEGER, $86::INTEGER, $87::INTEGER, $88::INTEGER, $89::INTEGER, $90::INTEGER, $91::INTEGER, $92::INTEGER, $93::INTEGER, $94::INTEGER, $95::INTEGER, $96::INTEGER, $97::INTEGER, $98::INTEGER, $99::INTEGER, $100::INTEGER, $101::INTEGER, $102::INTEGER, $103::INTEGER, $104::INTEGER, $105::INTEGER, $106::INTEGER, $107::INTEGER, $108::INTEGER, $109::INTEGER, $110::INTEGER, $111::INTEGER, $112::INTEGER, $113::INTEGER, $114::INTEGER, $115::INTEGER, $116::INTEGER, $117::INTEGER, $118::INTEGER, $119::INTEGER, $120::INTEGER, $121::INTEGER, $122::INTEGER, $123::INTEGER, $124::INTEGER, $125::INTEGER, $126::INTEGER, $127::INTEGER, $128::INTEGER, $129::INTEGER, $130::INTEGER, $131::INTEGER, $132::INTEGER, $133::INTEGER, $134::INTEGER, $135::INTEGER, $136::INTEGER, $137::INTEGER, $138::INTEGER, $139::INTEGER, $140::INTEGER, $141::INTEGER, $142::INTEGER, $143::INTEGER, $144::INTEGER, $145::INTEGER, $146::INTEGER, $147::INTEGER, $148::INTEGER, $149::INTEGER, $150::INTEGER, $151::INTEGER, $152::INTEGER, $153::INTEGER, $154::INTEGER, $155::INTEGER, $156::INTEGER, $157::INTEGER, $158::INTEGER, $159::INTEGER, $160::INTEGER, $161::INTEGER, $162::INTEGER, $163::INTEGER, $164::INTEGER, $165::INTEGER, $166::INTEGER, $167::INTEGER, $168::INTEGER, $169::INTEGER, $170::INTEGER, $171::INTEGER, $172::INTEGER, $173::INTEGER, $174::INTEGER, $175::INTEGER, $176::INTEGER, $177::INTEGER, $178::INTEGER, $179::INTEGER, $180::INTEGER, $181::INTEGER, $182::INTEGER, $183::INTEGER, $184::INTEGER, $185::INTEGER, $186::INTEGER, $187::INTEGER, $188::INTEGER, $189::INTEGER, $190::INTEGER, $191::INTEGER, $192::INTEGER, $193::INTEGER, $194::INTEGER, $195::INTEGER, $196::INTEGER, $197::INTEGER, $198::INTEGER, $199::INTEGER, $200::INTEGER, $201::INTEGER, $202::INTEGER, $203::INTEGER, $204::INTEGER, $205::INTEGER, $206::INTEGER, $207::INTEGER, $208::INTEGER, $209::INTEGER, $210::INTEGER, $211::INTEGER, $212::INTEGER, $213::INTEGER, $214::INTEGER, $215::INTEGER, $216::INTEGER, $217::INTEGER, $218::INTEGER, $219::INTEGER, $220::INTEGER, $221::INTEGER, $222::INTEGER, $223::INTEGER, $224::INTEGER, $225::INTEGER, $226::INTEGER, $227::INTEGER, $228::INTEGER, $229::INTEGER, $230::INTEGER, $231::INTEGER, $232::INTEGER, $233::INTEGER, $234::INTEGER, $235::INTEGER, $236::INTEGER, $237::INTEGER, $238::INTEGER, $239::INTEGER, $240::INTEGER, $241::INTEGER, $242::INTEGER, $243::INTEGER, $244::INTEGER, $245::INTEGER, $246::INTEGER, $247::INTEGER, $248::INTEGER, $249::INTEGER, $250::INTEGER, $251::INTEGER, $252::INTEGER, $253::INTEGER, $254::INTEGER, $255::INTEGER, $256::INTEGER, $257::INTEGER, $258::INTEGER, $259::INTEGER, $260::INTEGER, $261::INTEGER, $262::INTEGER, $263::INTEGER, $264::INTEGER, $265::INTEGER, $266::INTEGER, $267::INTEGER, $268::INTEGER, $269::INTEGER, $270::INTEGER, $271::INTEGER, $272::INTEGER, $273::INTEGER, $274::INTEGER, $275::INTEGER, $276::INTEGER, $277::INTEGER, $278::INTEGER, $279::INTEGER, $280::INTEGER, $281::INTEGER, $282::INTEGER, $283::INTEGER, $284::INTEGER, $285::INTEGER, $286::INTEGER, $287::INTEGER, $288::INTEGER, $289::INTEGER, $290::INTEGER, $291::INTEGER, $292::INTEGER, $293::INTEGER, $294::INTEGER, $295::INTEGER, $296::INTEGER, $297::INTEGER, $298::INTEGER, $299::INTEGER, $300::INTEGER, $301::INTEGER, $302::INTEGER, $303::INTEGER, $304::INTEGER, $305::INTEGER, $306::INTEGER, $307::INTEGER, $308::INTEGER, $309::INTEGER, $310::INTEGER, $311::INTEGER, $312::INTEGER, $313::INTEGER, $314::INTEGER, $315::INTEGER, $316::INTEGER, $317::INTEGER, $318::INTEGER, $319::INTEGER, $320::INTEGER, $321::INTEGER, $322::INTEGER, $323::INTEGER, $324::INTEGER, $325::INTEGER, $326::INTEGER, $327::INTEGER, $328::INTEGER, $329::INTEGER, $330::INTEGER, $331::INTEGER, $332::INTEGER, $333::INTEGER, $334::INTEGER, $335::INTEGER, $336::INTEGER, $337::INTEGER, $338::INTEGER, $339::INTEGER, $340::INTEGER, $341::INTEGER, $342::INTEGER, $343::INTEGER, $344::INTEGER, $345::INTEGER, $346::INTEGER, $347::INTEGER, $348::INTEGER, $349::INTEGER, $350::INTEGER, $351::INTEGER, $352::INTEGER, $353::INTEGER, $354::INTEGER, $355::INTEGER, $356::INTEGER, $357::INTEGER, $358::INTEGER, $359::INTEGER, $360::INTEGER, $361::INTEGER, $362::INTEGER, $363::INTEGER, $364::INTEGER, $365::INTEGER, $366::INTEGER, $367::INTEGER, $368::INTEGER, $369::INTEGER, $370::INTEGER, $371::INTEGER, $372::INTEGER, $373::INTEGER, $374::INTEGER, $375::INTEGER, $376::INTEGER, $377::INTEGER, $378::INTEGER, $379::INTEGER, $380::INTEGER, $381::INTEGER, $382::INTEGER, $383::INTEGER, $384::INTEGER, $385::INTEGER, $386::INTEGER, $387::INTEGER, $388::INTEGER, $389::INTEGER, $390::INTEGER, $391::INTEGER, $392::INTEGER, $393::INTEGER, $394::INTEGER, $395::INTEGER, $396::INTEGER, $397::INTEGER, $398::INTEGER, $399::INTEGER, $400::INTEGER, $401::INTEGER, $402::INTEGER, $403::INTEGER, $404::INTEGER, $405::INTEGER, $406::INTEGER, $407::INTEGER, $408::INTEGER, $409::INTEGER, $410::INTEGER, $411::INTEGER, $412::INTEGER, $413::INTEGER, $414::INTEGER, $415::INTEGER, $416::INTEGER, $417::INTEGER, $418::INTEGER, $419::INTEGER, $420::INTEGER, $421::INTEGER, $422::INTEGER, $423::INTEGER, $424::INTEGER, $425::INTEGER, $426::INTEGER, $427::INTEGER, $428::INTEGER, $429::INTEGER, $430::INTEGER, $431::INTEGER, $432::INTEGER, $433::INTEGER, $434::INTEGER, $435::INTEGER, $436::INTEGER, $437::INTEGER, $438::INTEGER, $439::INTEGER, $440::INTEGER, $441::INTEGER, $442::INTEGER, $443::INTEGER, $444::INTEGER, $445::INTEGER, $446::INTEGER, $447::INTEGER, $448::INTEGER, $449::INTEGER, $450::INTEGER, $451::INTEGER, $452::INTEGER, $453::INTEGER, $454::INTEGER, $455::INTEGER, $456::INTEGER, $457::INTEGER, $458::INTEGER, $459::INTEGER, $460::INTEGER, $461::INTEGER, $462::INTEGER, $463::INTEGER, $464::INTEGER, $465::INTEGER, $466::INTEGER, $467::INTEGER, $468::INTEGER, $469::INTEGER, $470::INTEGER, $471::INTEGER, $472::INTEGER, $473::INTEGER, $474::INTEGER, $475::INTEGER, $476::INTEGER, $477::INTEGER, $478::INTEGER, $479::INTEGER, $480::INTEGER, $481::INTEGER, $482::INTEGER, $483::INTEGER, $484::INTEGER, $485::INTEGER, $486::INTEGER, $487::INTEGER, $488::INTEGER, $489::INTEGER, $490::INTEGER, $491::INTEGER, $492::INTEGER, $493::INTEGER, $494::INTEGER, $495::INTEGER, $496::INTEGER, $497::INTEGER, $498::INTEGER, $499::INTEGER, $500::INTEGER, $501::INTEGER, $502::INTEGER)) AS anon_1 WHERE anon_1.rn_all <= $1::INTEGER OR anon_1.rn_def <= $2::INTEGER ORDER BY anon_1.test_case_id, anon_1.id DESC
-- binds: [50, 50, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132, 133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143, 144, 145, 146, 147, 148, 149, 150, 151, 152, 153, 154, 155, 156, 157, 158, 159, 160, 161, 162, 163, 164, 165, 166, 167, 168, 169, 170, 171, 172, 173, 174, 175, 176, 177, 178, 179, 180, 181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191, 192, 193, 194, 195, 196, 197, 198, 199, 200, 201, 202, 203, 204, 205, 206, 207, 208, 209, 210, 211, 212, 213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223, 224, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 239, 240, 241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255, 256, 257, 258, 259, 260, 261, 262, 263, 264, 265, 266, 267, 268, 269, 270, 271, 272, 273, 274, 275, 276, 277, 278, 279, 280, 281, 282, 283, 284, 285, 286, 287, 288, 289, 290, 291, 292, 293, 294, 295, 296, 297, 298, 299, 300, 301, 302, 303, 304, 305, 306, 307, 308, 309, 310, 311, 312, 313, 314, 315, 316, 317, 318, 319, 320, 321, 322, 323, 324, 325, 326, 327, 328, 329, 330, 331, 332, 333, 334, 335, 336, 337, 338, 339, 340, 341, 342, 343, 344, 345, 346, 347, 348, 349, 350, 351, 352, 353, 354, 355, 356, 357, 358, 359, 360, 361, 362, 363, 364, 365, 366, 367, 368, 369, 370, 371, 372, 373, 374, 375, 376, 377, 378, 379, 380, 381, 382, 383, 384, 385, 386, 387, 388, 389, 390, 391, 392, 393, 394, 395, 396, 397, 398, 399, 400, 401, 402, 403, 404, 405, 406, 407, 408, 409, 410, 411, 412, 413, 414, 415, 416, 417, 418, 419, 420, 421, 422, 423, 424, 425, 426, 427, 428, 429, 430, 431, 432, 433, 434, 435, 436, 437, 438, 439, 440, 441, 442, 443, 444, 445, 446, 447, 448, 449, 450, 451, 452, 453, 454, 455, 456, 457, 458, 459, 460, 461, 462, 463, 464, 465, 466, 467, 468, 469, 470, 471, 472, 473, 474, 475, 476, 477, 478, 479, 480, 481, 482, 483, 484, 485, 486, 487, 488, 489, 490, 491, 492, 493, 494, 495, 496, 497, 498, 499, 500]

Subquery Scan on anon_1  (cost=101016.88..337921.29 rows=557332 width=76) (actual time=3810.994..17427.315 rows=45000 loops=1)
  Filter: ((anon_1.rn_all <= 50) OR (anon_1.rn_def <= 50))
  Rows Removed by Filter: 955000
  Buffers: shared hit=16212 read=6868, temp read=9021 written=9045
  ->  WindowAgg  (cost=101016.88..322873.32 rows=1003198 width=93) (actual time=3810.979..15898.860 rows=1000000 loops=1)
        Buffers: shared hit=16212 read=6868, temp read=9021 written=9045
        ->  Incremental Sort  (cost=101016.67..302809.36 rows=1003198 width=85) (actual time=3810.963..12752.696 rows=1000000 loops=1)
              Sort Key: test_executions.test_case_id, test_executions.id DESC
              Presorted Key: test_executions.test_case_id
              Full-sort Groups: 500  Sort Method: quicksort  Average Memory: 31kB  Peak Memory: 31kB
              Pre-sorted Groups: 500  Sort Method: quicksort  Average Memory: 243kB  Peak Memory: 244kB
              Buffers: shared hit=16212 read=6868, temp read=9021 written=9045
              ->  WindowAgg  (cost=100827.24..240238.00 rows=1003198 width=85) (actual time=3793.430..9589.156 rows=1000000 loops=1)
                    Buffers: shared hit=16212 read=6868, temp read=9021 written=9045
                    ->  Gather Merge  (cost=100827.11..217666.05 rows=1003198 width=77) (actual time=3793.400..6380.993 rows=1000000 loops=1)
                          Workers Planned: 2
                          Workers Launched: 2
                          Buffers: shared hit=16212 read=6868, temp read=9021 written=9045
                          ->  Sort  (cost=99827.08..100872.08 rows=417999 width=77) (actual time=3777.200..4322.664 rows=333333 loops=3)
                                Sort Key: test_executions.test_case_id, (((repos.default_branch IS NULL) OR ((test_runs.branch)::text = (repos.default_branch)::text))), test_executions.id DESC
                                Sort Method: external merge  Disk: 24304kB
                                Buffers: shared hit=16212 read=6868, temp read=9021 written=9045
                                Worker 0:  Sort Method: external merge  Disk: 23832kB
                                Worker 1:  Sort Method: external merge  Disk: 24032kB
                                ->  Hash Join  (cost=206.37..42225.81 rows=417999 width=77) (actual time=40.735..2712.019 rows=333333 loops=3)
                                      Hash Cond: (test_executions.test_run_id = test_runs.id)
                                      Buffers: shared hit=16100 read=6868
                                      ->  Parallel Seq Scan on test_executions  (cost=1.25..35229.25 rows=417999 width=33) (actual time=10.902..911.957 rows=333333 loops=3)
                                            Filter: (test_case_id = ANY ('{1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63,64,65,66,67,68,69,70,71,72,73,74,75,76,77,78,79,80,81,82,83,84,85,86,87,88,89,90,91,92,93,94,95,96,97,98,99,100,101,102,103,104,105,106,107,108,109,110,111,112,113,114,115,116,117,118,119,120,121,122,123,124,125,126,127,128,129,130,131,132,133,134,135,136,137,138,139,140,141,142,143,144,145,146,147,148,149,150,151,152,153,154,155,156,157,158,159,160,161,162,163,164,165,166,167,168,169,170,171,172,173,174,175,176,177,178,179,180,181,182,183,184,185,186,187,188,189,190,191,192,193,194,195,196,197,198,199,200,201,202,203,204,205,206,207,208,209,210,211,212,213,214,215,216,217,218,219,220,221,222,223,224,225,226,227,228,229,230,231,232,233,234,235,236,237,238,239,240,241,242,243,244,245,246,247,248,249,250,251,252,253,254,255,256,257,258,259,260,261,262,263,264,265,266,267,268,269,270,271,272,273,274,275,276,277,278,279,280,281,282,283,284,285,286,287,288,289,290,291,292,293,294,295,296,297,298,299,300,301,302,303,304,305,306,307,308,309,310,311,312,313,314,315,316,317,318,319,320,321,322,323,324,325,326,327,328,329,330,331,332,333,334,335,336,337,338,339,340,341,342,343,344,345,346,347,348,349,350,351,352,353,354,355,356,357,358,359,360,361,362,363,364,365,366,367,368,369,370,371,372,373,374,375,376,377,378,379,380,381,382,383,384,385,386,387,388,389,390,391,392,393,394,395,396,397,398,399,400,401,402,403,404,405,406,407,408,409,410,411,412,413,414,415,416,417,418,419,420,421,422,423,424,425,426,427,428,429,430,431,432,433,434,435,436,437,438,439,440,441,442,443,444,445,446,447,448,449,450,451,452,453,454,455,456,457,458,459,460,461,462,463,464,465,466,467,468,469,470,471,472,473,474,475,476,477,478,479,480,481,482,483,484,485,486,487,488,489,490,491,492,493,494,495,496,497,498,499,500}'::integer[]))
                                            Rows Removed by Filter: 333333
                                            Buffers: shared hit=15860 read=6868
                                      ->  Hash  (cost=155.12..155.12 rows=4000 width=51) (actual time=29.766..29.791 rows=4000 loops=3)
                                            Buckets: 4096  Batches: 1  Memory Usage: 365kB
                                            Buffers: shared hit=180
                                            ->  Hash Join  (cost=2.12..155.12 rows=4000 width=51) (actual time=0.134..22.023 rows=4000 loops=3)
                                                  Hash Cond: (test_runs.project_id = projects.id)
                                                  Buffers: shared hit=180
                                                  ->  Seq Scan on test_runs  (cost=0.00..98.00 rows=4000 width=50) (actual time=0.024..7.477 rows=4000 loops=3)
                                                        Buffers: shared hit=174
                                                  ->  Hash  (cost=2.09..2.09 rows=2 width=9) (actual time=0.087..0.101 rows=2 loops=3)
                                                        Buckets: 1024  Batches: 1  Memory Usage: 9kB
                                                        Buffers: shared hit=6
                                                        ->  Nested Loop  (cost=0.00..2.09 rows=2 width=9) (actual time=0.051..0.083 rows=2 loops=3)
                                                              Join Filter: (projects.repo_id = repos.id)
                                                              Rows Removed by Join Filter: 1
                                                              Buffers: shared hit=6
                                                              ->  Seq Scan on projects  (cost=0.00..1.02 rows=2 width=8) (actual time=0.015..0.019 rows=2 loops=3)
                                                                    Buffers: shared hit=3
                                                              ->  Materialize  (cost=0.00..1.03 rows=2 width=9) (actual time=0.011..0.016 rows=2 loops=6)
                                                                    Buffers: shared hit=3
                                                                    ->  Seq Scan on repos  (cost=0.00..1.02 rows=2 width=9) (actual time=0.016..0.019 rows=2 loops=3)
                                                                          Buffers: shared hit=3
Planning:
  Buffers: shared hit=51
Planning Time: 0.669 ms
JIT:
  Functions: 91
  Options: Inlining false, Optimization false, Expressions true, Deforming true
  Timing: Generation 2.875 ms (Deform 1.478 ms), Inlining 0.000 ms, Optimization 1.340 ms, Emission 31.275 ms, Total 35.489 ms
Execution Time: 17499.301 ms
```

</details>

### after - this head: shape 1 (Defaults plentiful)

the same chunk through `test_execution_window_ids`: two `LATERAL ... LIMIT 50` arms;
about `2 x score_window` Executions read per Test:

<details>
<summary>full plan</summary>

```
-- statement: SELECT test_executions.test_case_id, test_runs.commit_sha, test_runs.branch, test_executions.status, test_executions.failure_category, repos.default_branch FROM test_executions JOIN test_runs ON test_runs.id = test_executions.test_run_id JOIN projects ON projects.id = test_runs.project_id JOIN repos ON repos.id = projects.repo_id WHERE test_executions.id IN (SELECT anon_1.id FROM (SELECT anon_2.test_case_id AS test_case_id, anon_3.id AS id FROM unnest($1::INTEGER[]) AS anon_2(test_case_id) JOIN LATERAL (SELECT test_executions.id AS id FROM test_executions WHERE test_executions.test_case_id = anon_2.test_case_id ORDER BY test_executions.id DESC LIMIT $2::INTEGER) AS anon_3 ON true UNION SELECT anon_4.test_case_id AS test_case_id, anon_5.id AS id FROM unnest($3::INTEGER[]) AS anon_4(test_case_id) JOIN LATERAL (SELECT test_executions.id AS id FROM test_executions JOIN test_runs ON test_runs.id = test_executions.test_run_id JOIN projects ON projects.id = test_runs.project_id JOIN repos ON repos.id = projects.repo_id WHERE test_executions.test_case_id = anon_4.test_case_id AND (repos.default_branch IS NULL OR test_runs.branch = repos.default_branch) ORDER BY test_executions.id DESC LIMIT $4::INTEGER) AS anon_5 ON true) AS anon_1) ORDER BY test_executions.test_case_id, test_executions.id DESC
-- binds: [[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132, 133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143, 144, 145, 146, 147, 148, 149, 150, 151, 152, 153, 154, 155, 156, 157, 158, 159, 160, 161, 162, 163, 164, 165, 166, 167, 168, 169, 170, 171, 172, 173, 174, 175, 176, 177, 178, 179, 180, 181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191, 192, 193, 194, 195, 196, 197, 198, 199, 200, 201, 202, 203, 204, 205, 206, 207, 208, 209, 210, 211, 212, 213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223, 224, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 239, 240, 241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255, 256, 257, 258, 259, 260, 261, 262, 263, 264, 265, 266, 267, 268, 269, 270, 271, 272, 273, 274, 275, 276, 277, 278, 279, 280, 281, 282, 283, 284, 285, 286, 287, 288, 289, 290, 291, 292, 293, 294, 295, 296, 297, 298, 299, 300, 301, 302, 303, 304, 305, 306, 307, 308, 309, 310, 311, 312, 313, 314, 315, 316, 317, 318, 319, 320, 321, 322, 323, 324, 325, 326, 327, 328, 329, 330, 331, 332, 333, 334, 335, 336, 337, 338, 339, 340, 341, 342, 343, 344, 345, 346, 347, 348, 349, 350, 351, 352, 353, 354, 355, 356, 357, 358, 359, 360, 361, 362, 363, 364, 365, 366, 367, 368, 369, 370, 371, 372, 373, 374, 375, 376, 377, 378, 379, 380, 381, 382, 383, 384, 385, 386, 387, 388, 389, 390, 391, 392, 393, 394, 395, 396, 397, 398, 399, 400, 401, 402, 403, 404, 405, 406, 407, 408, 409, 410, 411, 412, 413, 414, 415, 416, 417, 418, 419, 420, 421, 422, 423, 424, 425, 426, 427, 428, 429, 430, 431, 432, 433, 434, 435, 436, 437, 438, 439, 440, 441, 442, 443, 444, 445, 446, 447, 448, 449, 450, 451, 452, 453, 454, 455, 456, 457, 458, 459, 460, 461, 462, 463, 464, 465, 466, 467, 468, 469, 470, 471, 472, 473, 474, 475, 476, 477, 478, 479, 480, 481, 482, 483, 484, 485, 486, 487, 488, 489, 490, 491, 492, 493, 494, 495, 496, 497, 498, 499, 500], 50, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132, 133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143, 144, 145, 146, 147, 148, 149, 150, 151, 152, 153, 154, 155, 156, 157, 158, 159, 160, 161, 162, 163, 164, 165, 166, 167, 168, 169, 170, 171, 172, 173, 174, 175, 176, 177, 178, 179, 180, 181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191, 192, 193, 194, 195, 196, 197, 198, 199, 200, 201, 202, 203, 204, 205, 206, 207, 208, 209, 210, 211, 212, 213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223, 224, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 239, 240, 241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255, 256, 257, 258, 259, 260, 261, 262, 263, 264, 265, 266, 267, 268, 269, 270, 271, 272, 273, 274, 275, 276, 277, 278, 279, 280, 281, 282, 283, 284, 285, 286, 287, 288, 289, 290, 291, 292, 293, 294, 295, 296, 297, 298, 299, 300, 301, 302, 303, 304, 305, 306, 307, 308, 309, 310, 311, 312, 313, 314, 315, 316, 317, 318, 319, 320, 321, 322, 323, 324, 325, 326, 327, 328, 329, 330, 331, 332, 333, 334, 335, 336, 337, 338, 339, 340, 341, 342, 343, 344, 345, 346, 347, 348, 349, 350, 351, 352, 353, 354, 355, 356, 357, 358, 359, 360, 361, 362, 363, 364, 365, 366, 367, 368, 369, 370, 371, 372, 373, 374, 375, 376, 377, 378, 379, 380, 381, 382, 383, 384, 385, 386, 387, 388, 389, 390, 391, 392, 393, 394, 395, 396, 397, 398, 399, 400, 401, 402, 403, 404, 405, 406, 407, 408, 409, 410, 411, 412, 413, 414, 415, 416, 417, 418, 419, 420, 421, 422, 423, 424, 425, 426, 427, 428, 429, 430, 431, 432, 433, 434, 435, 436, 437, 438, 439, 440, 441, 442, 443, 444, 445, 446, 447, 448, 449, 450, 451, 452, 453, 454, 455, 456, 457, 458, 459, 460, 461, 462, 463, 464, 465, 466, 467, 468, 469, 470, 471, 472, 473, 474, 475, 476, 477, 478, 479, 480, 481, 482, 483, 484, 485, 486, 487, 488, 489, 490, 491, 492, 493, 494, 495, 496, 497, 498, 499, 500], 50]

Sort  (cost=202577.23..202702.23 rows=50000 width=76) (actual time=1548.270..1585.183 rows=25500 loops=1)
  Sort Key: test_executions.test_case_id, test_executions.id DESC
  Sort Method: quicksort  Memory: 2773kB
  Buffers: shared hit=105992
  ->  Hash Join  (cost=194120.31..196452.32 rows=50000 width=76) (actual time=1186.686..1495.002 rows=25500 loops=1)
        Hash Cond: (test_executions.test_run_id = test_runs.id)
        Buffers: shared hit=105992
        ->  Nested Loop  (cost=193915.19..195559.82 rows=50000 width=33) (actual time=1146.562..1375.538 rows=25500 loops=1)
              Buffers: shared hit=105932
              ->  HashAggregate  (cost=193914.76..193916.76 rows=200 width=8) (actual time=1146.528..1188.604 rows=25500 loops=1)
                    Group Key: test_executions_1.id
                    Batches: 1  Memory Usage: 1825kB
                    Buffers: shared hit=3932
                    ->  HashAggregate  (cost=192789.76..193289.76 rows=50000 width=12) (actual time=1061.520..1102.766 rows=25500 loops=1)
                          Group Key: anon_2.test_case_id, test_executions_1.id
                          Batches: 1  Memory Usage: 2577kB
                          Buffers: shared hit=3932
                          ->  Append  (cost=0.43..192539.76 rows=50000 width=12) (actual time=0.078..975.743 rows=50000 loops=1)
                                Buffers: shared hit=3932
                                ->  Nested Loop  (cost=0.43..1306.25 rows=25000 width=12) (actual time=0.074..186.638 rows=25000 loops=1)
                                      Buffers: shared hit=1501
                                      ->  Function Scan on unnest anon_2  (cost=0.00..5.00 rows=500 width=4) (actual time=0.027..0.765 rows=500 loops=1)
                                      ->  Limit  (cost=0.43..2.10 rows=50 width=8) (actual time=0.012..0.226 rows=50 loops=500)
                                            Buffers: shared hit=1501
                                            ->  Index Only Scan Backward using ix_exec_case_id on test_executions test_executions_1  (cost=0.43..67.43 rows=2000 width=8) (actual time=0.008..0.080 rows=50 loops=500)
                                                  Index Cond: (test_case_id = anon_2.test_case_id)
                                                  Heap Fetches: 0
                                                  Buffers: shared hit=1501
                                ->  Nested Loop  (cost=0.72..190983.51 rows=25000 width=12) (actual time=0.269..646.190 rows=25000 loops=1)
                                      Buffers: shared hit=2431
                                      ->  Function Scan on unnest anon_4  (cost=0.00..5.00 rows=500 width=4) (actual time=0.044..0.779 rows=500 loops=1)
                                      ->  Limit  (cost=0.72..381.46 rows=50 width=8) (actual time=0.041..1.146 rows=50 loops=500)
                                            Buffers: shared hit=2431
                                            ->  Nested Loop  (cost=0.72..7478.44 rows=982 width=8) (actual time=0.038..1.000 rows=50 loops=500)
                                                  Join Filter: (((repos_1.default_branch IS NULL) OR ((test_runs_1.branch)::text = (repos_1.default_branch)::text)) AND (test_runs_1.project_id = projects_1.id))
                                                  Rows Removed by Join Filter: 53
                                                  Buffers: shared hit=2431
                                                  ->  Nested Loop  (cost=0.72..7406.34 rows=2000 width=21) (actual time=0.014..0.404 rows=52 loops=500)
                                                        Buffers: shared hit=2429
                                                        ->  Index Scan Backward using ix_exec_case_id on test_executions test_executions_2  (cost=0.43..6648.29 rows=2000 width=12) (actual time=0.008..0.094 rows=52 loops=500)
                                                              Index Cond: (test_case_id = anon_4.test_case_id)
                                                              Buffers: shared hit=2273
                                                        ->  Memoize  (cost=0.29..0.46 rows=1 width=17) (actual time=0.002..0.002 rows=1 loops=26000)
                                                              Cache Key: test_executions_2.test_run_id
                                                              Cache Mode: logical
                                                              Hits: 25948  Misses: 52  Evictions: 0  Overflows: 0  Memory Usage: 6kB
                                                              Buffers: shared hit=156
                                                              ->  Index Scan using test_runs_pkey on test_runs test_runs_1  (cost=0.28..0.45 rows=1 width=17) (actual time=0.003..0.003 rows=1 loops=52)
                                                                    Index Cond: (id = test_executions_2.test_run_id)
                                                                    Buffers: shared hit=156
                                                  ->  Materialize  (cost=0.00..2.10 rows=2 width=9) (actual time=0.001..0.004 rows=2 loops=26000)
                                                        Buffers: shared hit=2
                                                        ->  Nested Loop  (cost=0.00..2.09 rows=2 width=9) (actual time=0.038..0.067 rows=2 loops=1)
                                                              Join Filter: (projects_1.repo_id = repos_1.id)
                                                              Rows Removed by Join Filter: 1
                                                              Buffers: shared hit=2
                                                              ->  Seq Scan on projects projects_1  (cost=0.00..1.02 rows=2 width=8) (actual time=0.008..0.013 rows=2 loops=1)
                                                                    Buffers: shared hit=1
                                                              ->  Materialize  (cost=0.00..1.03 rows=2 width=9) (actual time=0.008..0.013 rows=2 loops=2)
                                                                    Buffers: shared hit=1
                                                                    ->  Seq Scan on repos repos_1  (cost=0.00..1.02 rows=2 width=9) (actual time=0.009..0.012 rows=2 loops=1)
                                                                          Buffers: shared hit=1
              ->  Index Scan using test_executions_pkey on test_executions  (cost=0.43..8.40 rows=1 width=33) (actual time=0.003..0.003 rows=1 loops=25500)
                    Index Cond: (id = test_executions_1.id)
                    Buffers: shared hit=102000
        ->  Hash  (cost=155.12..155.12 rows=4000 width=51) (actual time=40.098..40.132 rows=4000 loops=1)
              Buckets: 4096  Batches: 1  Memory Usage: 365kB
              Buffers: shared hit=60
              ->  Hash Join  (cost=2.12..155.12 rows=4000 width=51) (actual time=15.434..33.947 rows=4000 loops=1)
                    Hash Cond: (test_runs.project_id = projects.id)
                    Buffers: shared hit=60
                    ->  Seq Scan on test_runs  (cost=0.00..98.00 rows=4000 width=50) (actual time=0.006..5.874 rows=4000 loops=1)
                          Buffers: shared hit=58
                    ->  Hash  (cost=2.09..2.09 rows=2 width=9) (actual time=15.404..15.428 rows=2 loops=1)
                          Buckets: 1024  Batches: 1  Memory Usage: 9kB
                          Buffers: shared hit=2
                          ->  Nested Loop  (cost=0.00..2.09 rows=2 width=9) (actual time=15.372..15.403 rows=2 loops=1)
                                Join Filter: (projects.repo_id = repos.id)
                                Rows Removed by Join Filter: 1
                                Buffers: shared hit=2
                                ->  Seq Scan on projects  (cost=0.00..1.02 rows=2 width=8) (actual time=0.004..0.009 rows=2 loops=1)
                                      Buffers: shared hit=1
                                ->  Materialize  (cost=0.00..1.03 rows=2 width=9) (actual time=7.675..7.679 rows=2 loops=2)
                                      Buffers: shared hit=1
                                      ->  Seq Scan on repos  (cost=0.00..1.02 rows=2 width=9) (actual time=15.343..15.347 rows=2 loops=1)
                                            Buffers: shared hit=1
Planning:
  Buffers: shared hit=76
Planning Time: 0.733 ms
JIT:
  Functions: 74
  Options: Inlining false, Optimization false, Expressions true, Deforming true
  Timing: Generation 1.344 ms (Deform 0.565 ms), Inlining 0.000 ms, Optimization 0.541 ms, Emission 14.946 ms, Total 16.831 ms
Execution Time: 1625.645 ms
```

</details>

### before - base commit: shape 2 (Defaults scarce)

same statement, the scarce Repo's chunk of 500 Test ids:

<details>
<summary>full plan</summary>

```
-- statement: SELECT anon_1.test_case_id, anon_1.commit_sha, anon_1.branch, anon_1.status, anon_1.failure_category, anon_1.default_branch FROM (SELECT test_executions.id AS id, test_executions.test_case_id AS test_case_id, test_executions.status AS status, test_executions.failure_category AS failure_category, test_runs.commit_sha AS commit_sha, test_runs.branch AS branch, repos.default_branch AS default_branch, row_number() OVER (PARTITION BY test_executions.test_case_id ORDER BY test_executions.id DESC) AS rn_all, row_number() OVER (PARTITION BY test_executions.test_case_id, repos.default_branch IS NULL OR test_runs.branch = repos.default_branch ORDER BY test_executions.id DESC) AS rn_def FROM test_executions JOIN test_runs ON test_runs.id = test_executions.test_run_id JOIN projects ON projects.id = test_runs.project_id JOIN repos ON repos.id = projects.repo_id WHERE test_executions.test_case_id IN ($3::INTEGER, $4::INTEGER, $5::INTEGER, $6::INTEGER, $7::INTEGER, $8::INTEGER, $9::INTEGER, $10::INTEGER, $11::INTEGER, $12::INTEGER, $13::INTEGER, $14::INTEGER, $15::INTEGER, $16::INTEGER, $17::INTEGER, $18::INTEGER, $19::INTEGER, $20::INTEGER, $21::INTEGER, $22::INTEGER, $23::INTEGER, $24::INTEGER, $25::INTEGER, $26::INTEGER, $27::INTEGER, $28::INTEGER, $29::INTEGER, $30::INTEGER, $31::INTEGER, $32::INTEGER, $33::INTEGER, $34::INTEGER, $35::INTEGER, $36::INTEGER, $37::INTEGER, $38::INTEGER, $39::INTEGER, $40::INTEGER, $41::INTEGER, $42::INTEGER, $43::INTEGER, $44::INTEGER, $45::INTEGER, $46::INTEGER, $47::INTEGER, $48::INTEGER, $49::INTEGER, $50::INTEGER, $51::INTEGER, $52::INTEGER, $53::INTEGER, $54::INTEGER, $55::INTEGER, $56::INTEGER, $57::INTEGER, $58::INTEGER, $59::INTEGER, $60::INTEGER, $61::INTEGER, $62::INTEGER, $63::INTEGER, $64::INTEGER, $65::INTEGER, $66::INTEGER, $67::INTEGER, $68::INTEGER, $69::INTEGER, $70::INTEGER, $71::INTEGER, $72::INTEGER, $73::INTEGER, $74::INTEGER, $75::INTEGER, $76::INTEGER, $77::INTEGER, $78::INTEGER, $79::INTEGER, $80::INTEGER, $81::INTEGER, $82::INTEGER, $83::INTEGER, $84::INTEGER, $85::INTEGER, $86::INTEGER, $87::INTEGER, $88::INTEGER, $89::INTEGER, $90::INTEGER, $91::INTEGER, $92::INTEGER, $93::INTEGER, $94::INTEGER, $95::INTEGER, $96::INTEGER, $97::INTEGER, $98::INTEGER, $99::INTEGER, $100::INTEGER, $101::INTEGER, $102::INTEGER, $103::INTEGER, $104::INTEGER, $105::INTEGER, $106::INTEGER, $107::INTEGER, $108::INTEGER, $109::INTEGER, $110::INTEGER, $111::INTEGER, $112::INTEGER, $113::INTEGER, $114::INTEGER, $115::INTEGER, $116::INTEGER, $117::INTEGER, $118::INTEGER, $119::INTEGER, $120::INTEGER, $121::INTEGER, $122::INTEGER, $123::INTEGER, $124::INTEGER, $125::INTEGER, $126::INTEGER, $127::INTEGER, $128::INTEGER, $129::INTEGER, $130::INTEGER, $131::INTEGER, $132::INTEGER, $133::INTEGER, $134::INTEGER, $135::INTEGER, $136::INTEGER, $137::INTEGER, $138::INTEGER, $139::INTEGER, $140::INTEGER, $141::INTEGER, $142::INTEGER, $143::INTEGER, $144::INTEGER, $145::INTEGER, $146::INTEGER, $147::INTEGER, $148::INTEGER, $149::INTEGER, $150::INTEGER, $151::INTEGER, $152::INTEGER, $153::INTEGER, $154::INTEGER, $155::INTEGER, $156::INTEGER, $157::INTEGER, $158::INTEGER, $159::INTEGER, $160::INTEGER, $161::INTEGER, $162::INTEGER, $163::INTEGER, $164::INTEGER, $165::INTEGER, $166::INTEGER, $167::INTEGER, $168::INTEGER, $169::INTEGER, $170::INTEGER, $171::INTEGER, $172::INTEGER, $173::INTEGER, $174::INTEGER, $175::INTEGER, $176::INTEGER, $177::INTEGER, $178::INTEGER, $179::INTEGER, $180::INTEGER, $181::INTEGER, $182::INTEGER, $183::INTEGER, $184::INTEGER, $185::INTEGER, $186::INTEGER, $187::INTEGER, $188::INTEGER, $189::INTEGER, $190::INTEGER, $191::INTEGER, $192::INTEGER, $193::INTEGER, $194::INTEGER, $195::INTEGER, $196::INTEGER, $197::INTEGER, $198::INTEGER, $199::INTEGER, $200::INTEGER, $201::INTEGER, $202::INTEGER, $203::INTEGER, $204::INTEGER, $205::INTEGER, $206::INTEGER, $207::INTEGER, $208::INTEGER, $209::INTEGER, $210::INTEGER, $211::INTEGER, $212::INTEGER, $213::INTEGER, $214::INTEGER, $215::INTEGER, $216::INTEGER, $217::INTEGER, $218::INTEGER, $219::INTEGER, $220::INTEGER, $221::INTEGER, $222::INTEGER, $223::INTEGER, $224::INTEGER, $225::INTEGER, $226::INTEGER, $227::INTEGER, $228::INTEGER, $229::INTEGER, $230::INTEGER, $231::INTEGER, $232::INTEGER, $233::INTEGER, $234::INTEGER, $235::INTEGER, $236::INTEGER, $237::INTEGER, $238::INTEGER, $239::INTEGER, $240::INTEGER, $241::INTEGER, $242::INTEGER, $243::INTEGER, $244::INTEGER, $245::INTEGER, $246::INTEGER, $247::INTEGER, $248::INTEGER, $249::INTEGER, $250::INTEGER, $251::INTEGER, $252::INTEGER, $253::INTEGER, $254::INTEGER, $255::INTEGER, $256::INTEGER, $257::INTEGER, $258::INTEGER, $259::INTEGER, $260::INTEGER, $261::INTEGER, $262::INTEGER, $263::INTEGER, $264::INTEGER, $265::INTEGER, $266::INTEGER, $267::INTEGER, $268::INTEGER, $269::INTEGER, $270::INTEGER, $271::INTEGER, $272::INTEGER, $273::INTEGER, $274::INTEGER, $275::INTEGER, $276::INTEGER, $277::INTEGER, $278::INTEGER, $279::INTEGER, $280::INTEGER, $281::INTEGER, $282::INTEGER, $283::INTEGER, $284::INTEGER, $285::INTEGER, $286::INTEGER, $287::INTEGER, $288::INTEGER, $289::INTEGER, $290::INTEGER, $291::INTEGER, $292::INTEGER, $293::INTEGER, $294::INTEGER, $295::INTEGER, $296::INTEGER, $297::INTEGER, $298::INTEGER, $299::INTEGER, $300::INTEGER, $301::INTEGER, $302::INTEGER, $303::INTEGER, $304::INTEGER, $305::INTEGER, $306::INTEGER, $307::INTEGER, $308::INTEGER, $309::INTEGER, $310::INTEGER, $311::INTEGER, $312::INTEGER, $313::INTEGER, $314::INTEGER, $315::INTEGER, $316::INTEGER, $317::INTEGER, $318::INTEGER, $319::INTEGER, $320::INTEGER, $321::INTEGER, $322::INTEGER, $323::INTEGER, $324::INTEGER, $325::INTEGER, $326::INTEGER, $327::INTEGER, $328::INTEGER, $329::INTEGER, $330::INTEGER, $331::INTEGER, $332::INTEGER, $333::INTEGER, $334::INTEGER, $335::INTEGER, $336::INTEGER, $337::INTEGER, $338::INTEGER, $339::INTEGER, $340::INTEGER, $341::INTEGER, $342::INTEGER, $343::INTEGER, $344::INTEGER, $345::INTEGER, $346::INTEGER, $347::INTEGER, $348::INTEGER, $349::INTEGER, $350::INTEGER, $351::INTEGER, $352::INTEGER, $353::INTEGER, $354::INTEGER, $355::INTEGER, $356::INTEGER, $357::INTEGER, $358::INTEGER, $359::INTEGER, $360::INTEGER, $361::INTEGER, $362::INTEGER, $363::INTEGER, $364::INTEGER, $365::INTEGER, $366::INTEGER, $367::INTEGER, $368::INTEGER, $369::INTEGER, $370::INTEGER, $371::INTEGER, $372::INTEGER, $373::INTEGER, $374::INTEGER, $375::INTEGER, $376::INTEGER, $377::INTEGER, $378::INTEGER, $379::INTEGER, $380::INTEGER, $381::INTEGER, $382::INTEGER, $383::INTEGER, $384::INTEGER, $385::INTEGER, $386::INTEGER, $387::INTEGER, $388::INTEGER, $389::INTEGER, $390::INTEGER, $391::INTEGER, $392::INTEGER, $393::INTEGER, $394::INTEGER, $395::INTEGER, $396::INTEGER, $397::INTEGER, $398::INTEGER, $399::INTEGER, $400::INTEGER, $401::INTEGER, $402::INTEGER, $403::INTEGER, $404::INTEGER, $405::INTEGER, $406::INTEGER, $407::INTEGER, $408::INTEGER, $409::INTEGER, $410::INTEGER, $411::INTEGER, $412::INTEGER, $413::INTEGER, $414::INTEGER, $415::INTEGER, $416::INTEGER, $417::INTEGER, $418::INTEGER, $419::INTEGER, $420::INTEGER, $421::INTEGER, $422::INTEGER, $423::INTEGER, $424::INTEGER, $425::INTEGER, $426::INTEGER, $427::INTEGER, $428::INTEGER, $429::INTEGER, $430::INTEGER, $431::INTEGER, $432::INTEGER, $433::INTEGER, $434::INTEGER, $435::INTEGER, $436::INTEGER, $437::INTEGER, $438::INTEGER, $439::INTEGER, $440::INTEGER, $441::INTEGER, $442::INTEGER, $443::INTEGER, $444::INTEGER, $445::INTEGER, $446::INTEGER, $447::INTEGER, $448::INTEGER, $449::INTEGER, $450::INTEGER, $451::INTEGER, $452::INTEGER, $453::INTEGER, $454::INTEGER, $455::INTEGER, $456::INTEGER, $457::INTEGER, $458::INTEGER, $459::INTEGER, $460::INTEGER, $461::INTEGER, $462::INTEGER, $463::INTEGER, $464::INTEGER, $465::INTEGER, $466::INTEGER, $467::INTEGER, $468::INTEGER, $469::INTEGER, $470::INTEGER, $471::INTEGER, $472::INTEGER, $473::INTEGER, $474::INTEGER, $475::INTEGER, $476::INTEGER, $477::INTEGER, $478::INTEGER, $479::INTEGER, $480::INTEGER, $481::INTEGER, $482::INTEGER, $483::INTEGER, $484::INTEGER, $485::INTEGER, $486::INTEGER, $487::INTEGER, $488::INTEGER, $489::INTEGER, $490::INTEGER, $491::INTEGER, $492::INTEGER, $493::INTEGER, $494::INTEGER, $495::INTEGER, $496::INTEGER, $497::INTEGER, $498::INTEGER, $499::INTEGER, $500::INTEGER, $501::INTEGER, $502::INTEGER)) AS anon_1 WHERE anon_1.rn_all <= $1::INTEGER OR anon_1.rn_def <= $2::INTEGER ORDER BY anon_1.test_case_id, anon_1.id DESC
-- binds: [50, 50, 501, 502, 503, 504, 505, 506, 507, 508, 509, 510, 511, 512, 513, 514, 515, 516, 517, 518, 519, 520, 521, 522, 523, 524, 525, 526, 527, 528, 529, 530, 531, 532, 533, 534, 535, 536, 537, 538, 539, 540, 541, 542, 543, 544, 545, 546, 547, 548, 549, 550, 551, 552, 553, 554, 555, 556, 557, 558, 559, 560, 561, 562, 563, 564, 565, 566, 567, 568, 569, 570, 571, 572, 573, 574, 575, 576, 577, 578, 579, 580, 581, 582, 583, 584, 585, 586, 587, 588, 589, 590, 591, 592, 593, 594, 595, 596, 597, 598, 599, 600, 601, 602, 603, 604, 605, 606, 607, 608, 609, 610, 611, 612, 613, 614, 615, 616, 617, 618, 619, 620, 621, 622, 623, 624, 625, 626, 627, 628, 629, 630, 631, 632, 633, 634, 635, 636, 637, 638, 639, 640, 641, 642, 643, 644, 645, 646, 647, 648, 649, 650, 651, 652, 653, 654, 655, 656, 657, 658, 659, 660, 661, 662, 663, 664, 665, 666, 667, 668, 669, 670, 671, 672, 673, 674, 675, 676, 677, 678, 679, 680, 681, 682, 683, 684, 685, 686, 687, 688, 689, 690, 691, 692, 693, 694, 695, 696, 697, 698, 699, 700, 701, 702, 703, 704, 705, 706, 707, 708, 709, 710, 711, 712, 713, 714, 715, 716, 717, 718, 719, 720, 721, 722, 723, 724, 725, 726, 727, 728, 729, 730, 731, 732, 733, 734, 735, 736, 737, 738, 739, 740, 741, 742, 743, 744, 745, 746, 747, 748, 749, 750, 751, 752, 753, 754, 755, 756, 757, 758, 759, 760, 761, 762, 763, 764, 765, 766, 767, 768, 769, 770, 771, 772, 773, 774, 775, 776, 777, 778, 779, 780, 781, 782, 783, 784, 785, 786, 787, 788, 789, 790, 791, 792, 793, 794, 795, 796, 797, 798, 799, 800, 801, 802, 803, 804, 805, 806, 807, 808, 809, 810, 811, 812, 813, 814, 815, 816, 817, 818, 819, 820, 821, 822, 823, 824, 825, 826, 827, 828, 829, 830, 831, 832, 833, 834, 835, 836, 837, 838, 839, 840, 841, 842, 843, 844, 845, 846, 847, 848, 849, 850, 851, 852, 853, 854, 855, 856, 857, 858, 859, 860, 861, 862, 863, 864, 865, 866, 867, 868, 869, 870, 871, 872, 873, 874, 875, 876, 877, 878, 879, 880, 881, 882, 883, 884, 885, 886, 887, 888, 889, 890, 891, 892, 893, 894, 895, 896, 897, 898, 899, 900, 901, 902, 903, 904, 905, 906, 907, 908, 909, 910, 911, 912, 913, 914, 915, 916, 917, 918, 919, 920, 921, 922, 923, 924, 925, 926, 927, 928, 929, 930, 931, 932, 933, 934, 935, 936, 937, 938, 939, 940, 941, 942, 943, 944, 945, 946, 947, 948, 949, 950, 951, 952, 953, 954, 955, 956, 957, 958, 959, 960, 961, 962, 963, 964, 965, 966, 967, 968, 969, 970, 971, 972, 973, 974, 975, 976, 977, 978, 979, 980, 981, 982, 983, 984, 985, 986, 987, 988, 989, 990, 991, 992, 993, 994, 995, 996, 997, 998, 999, 1000]

Subquery Scan on anon_1  (cost=100585.34..335933.51 rows=553779 width=76) (actual time=3819.162..17412.791 rows=26500 loops=1)
  Filter: ((anon_1.rn_all <= 50) OR (anon_1.rn_def <= 50))
  Rows Removed by Filter: 973500
  Buffers: shared hit=16314 read=6756, temp read=9995 written=10022
  ->  WindowAgg  (cost=100585.34..320981.48 rows=996802 width=93) (actual time=3819.149..15905.739 rows=1000000 loops=1)
        Buffers: shared hit=16314 read=6756, temp read=9995 written=10022
        ->  Incremental Sort  (cost=100585.12..301045.44 rows=996802 width=85) (actual time=3819.134..12771.459 rows=1000000 loops=1)
              Sort Key: test_executions.test_case_id, test_executions.id DESC
              Presorted Key: test_executions.test_case_id
              Full-sort Groups: 500  Sort Method: quicksort  Average Memory: 31kB  Peak Memory: 31kB
              Pre-sorted Groups: 500  Sort Method: quicksort  Average Memory: 258kB  Peak Memory: 259kB
              Buffers: shared hit=16314 read=6756, temp read=9995 written=10022
              ->  WindowAgg  (cost=100396.95..238918.88 rows=996802 width=85) (actual time=3800.391..9598.015 rows=1000000 loops=1)
                    Buffers: shared hit=16314 read=6756, temp read=9995 written=10022
                    ->  Gather Merge  (cost=100396.82..216490.84 rows=996802 width=77) (actual time=3800.361..6394.997 rows=1000000 loops=1)
                          Workers Planned: 2
                          Workers Launched: 2
                          Buffers: shared hit=16314 read=6756, temp read=9995 written=10022
                          ->  Sort  (cost=99396.79..100435.13 rows=415334 width=77) (actual time=3785.867..4328.250 rows=333333 loops=3)
                                Sort Key: test_executions.test_case_id, (((repos.default_branch IS NULL) OR ((test_runs.branch)::text = (repos.default_branch)::text))), test_executions.id DESC
                                Sort Method: external merge  Disk: 27056kB
                                Buffers: shared hit=16314 read=6756, temp read=9995 written=10022
                                Worker 0:  Sort Method: external merge  Disk: 26392kB
                                Worker 1:  Sort Method: external merge  Disk: 26512kB
                                ->  Hash Join  (cost=206.37..42182.51 rows=415334 width=77) (actual time=35.884..2762.623 rows=333333 loops=3)
                                      Hash Cond: (test_executions.test_run_id = test_runs.id)
                                      Buffers: shared hit=16212 read=6756
                                      ->  Parallel Seq Scan on test_executions  (cost=1.25..35229.25 rows=415334 width=33) (actual time=9.545..935.660 rows=333333 loops=3)
                                            Filter: (test_case_id = ANY ('{501,502,503,504,505,506,507,508,509,510,511,512,513,514,515,516,517,518,519,520,521,522,523,524,525,526,527,528,529,530,531,532,533,534,535,536,537,538,539,540,541,542,543,544,545,546,547,548,549,550,551,552,553,554,555,556,557,558,559,560,561,562,563,564,565,566,567,568,569,570,571,572,573,574,575,576,577,578,579,580,581,582,583,584,585,586,587,588,589,590,591,592,593,594,595,596,597,598,599,600,601,602,603,604,605,606,607,608,609,610,611,612,613,614,615,616,617,618,619,620,621,622,623,624,625,626,627,628,629,630,631,632,633,634,635,636,637,638,639,640,641,642,643,644,645,646,647,648,649,650,651,652,653,654,655,656,657,658,659,660,661,662,663,664,665,666,667,668,669,670,671,672,673,674,675,676,677,678,679,680,681,682,683,684,685,686,687,688,689,690,691,692,693,694,695,696,697,698,699,700,701,702,703,704,705,706,707,708,709,710,711,712,713,714,715,716,717,718,719,720,721,722,723,724,725,726,727,728,729,730,731,732,733,734,735,736,737,738,739,740,741,742,743,744,745,746,747,748,749,750,751,752,753,754,755,756,757,758,759,760,761,762,763,764,765,766,767,768,769,770,771,772,773,774,775,776,777,778,779,780,781,782,783,784,785,786,787,788,789,790,791,792,793,794,795,796,797,798,799,800,801,802,803,804,805,806,807,808,809,810,811,812,813,814,815,816,817,818,819,820,821,822,823,824,825,826,827,828,829,830,831,832,833,834,835,836,837,838,839,840,841,842,843,844,845,846,847,848,849,850,851,852,853,854,855,856,857,858,859,860,861,862,863,864,865,866,867,868,869,870,871,872,873,874,875,876,877,878,879,880,881,882,883,884,885,886,887,888,889,890,891,892,893,894,895,896,897,898,899,900,901,902,903,904,905,906,907,908,909,910,911,912,913,914,915,916,917,918,919,920,921,922,923,924,925,926,927,928,929,930,931,932,933,934,935,936,937,938,939,940,941,942,943,944,945,946,947,948,949,950,951,952,953,954,955,956,957,958,959,960,961,962,963,964,965,966,967,968,969,970,971,972,973,974,975,976,977,978,979,980,981,982,983,984,985,986,987,988,989,990,991,992,993,994,995,996,997,998,999,1000}'::integer[]))
                                            Rows Removed by Filter: 333333
                                            Buffers: shared hit=15972 read=6756
                                      ->  Hash  (cost=155.12..155.12 rows=4000 width=51) (actual time=26.279..26.305 rows=4000 loops=3)
                                            Buckets: 4096  Batches: 1  Memory Usage: 365kB
                                            Buffers: shared hit=180
                                            ->  Hash Join  (cost=2.12..155.12 rows=4000 width=51) (actual time=0.118..19.621 rows=4000 loops=3)
                                                  Hash Cond: (test_runs.project_id = projects.id)
                                                  Buffers: shared hit=180
                                                  ->  Seq Scan on test_runs  (cost=0.00..98.00 rows=4000 width=50) (actual time=0.020..6.361 rows=4000 loops=3)
                                                        Buffers: shared hit=174
                                                  ->  Hash  (cost=2.09..2.09 rows=2 width=9) (actual time=0.076..0.091 rows=2 loops=3)
                                                        Buckets: 1024  Batches: 1  Memory Usage: 9kB
                                                        Buffers: shared hit=6
                                                        ->  Nested Loop  (cost=0.00..2.09 rows=2 width=9) (actual time=0.045..0.077 rows=2 loops=3)
                                                              Join Filter: (projects.repo_id = repos.id)
                                                              Rows Removed by Join Filter: 1
                                                              Buffers: shared hit=6
                                                              ->  Seq Scan on projects  (cost=0.00..1.02 rows=2 width=8) (actual time=0.010..0.015 rows=2 loops=3)
                                                                    Buffers: shared hit=3
                                                              ->  Materialize  (cost=0.00..1.03 rows=2 width=9) (actual time=0.011..0.015 rows=2 loops=6)
                                                                    Buffers: shared hit=3
                                                                    ->  Seq Scan on repos  (cost=0.00..1.02 rows=2 width=9) (actual time=0.016..0.019 rows=2 loops=3)
                                                                          Buffers: shared hit=3
Planning:
  Buffers: shared hit=38
Planning Time: 0.695 ms
JIT:
  Functions: 91
  Options: Inlining false, Optimization false, Expressions true, Deforming true
  Timing: Generation 2.787 ms (Deform 1.423 ms), Inlining 0.000 ms, Optimization 1.271 ms, Emission 26.880 ms, Total 30.938 ms
Execution Time: 17457.598 ms
```

</details>

### after - this head: shape 2 (Defaults scarce) - the accepted worst case

the overall arm still stops at 50 per Test; the Default-branch arm reads all 2,000
Executions per Test to find its 3 Default rows:

<details>
<summary>full plan</summary>

```
-- statement: SELECT test_executions.test_case_id, test_runs.commit_sha, test_runs.branch, test_executions.status, test_executions.failure_category, repos.default_branch FROM test_executions JOIN test_runs ON test_runs.id = test_executions.test_run_id JOIN projects ON projects.id = test_runs.project_id JOIN repos ON repos.id = projects.repo_id WHERE test_executions.id IN (SELECT anon_1.id FROM (SELECT anon_2.test_case_id AS test_case_id, anon_3.id AS id FROM unnest($1::INTEGER[]) AS anon_2(test_case_id) JOIN LATERAL (SELECT test_executions.id AS id FROM test_executions WHERE test_executions.test_case_id = anon_2.test_case_id ORDER BY test_executions.id DESC LIMIT $2::INTEGER) AS anon_3 ON true UNION SELECT anon_4.test_case_id AS test_case_id, anon_5.id AS id FROM unnest($3::INTEGER[]) AS anon_4(test_case_id) JOIN LATERAL (SELECT test_executions.id AS id FROM test_executions JOIN test_runs ON test_runs.id = test_executions.test_run_id JOIN projects ON projects.id = test_runs.project_id JOIN repos ON repos.id = projects.repo_id WHERE test_executions.test_case_id = anon_4.test_case_id AND (repos.default_branch IS NULL OR test_runs.branch = repos.default_branch) ORDER BY test_executions.id DESC LIMIT $4::INTEGER) AS anon_5 ON true) AS anon_1) ORDER BY test_executions.test_case_id, test_executions.id DESC
-- binds: [[501, 502, 503, 504, 505, 506, 507, 508, 509, 510, 511, 512, 513, 514, 515, 516, 517, 518, 519, 520, 521, 522, 523, 524, 525, 526, 527, 528, 529, 530, 531, 532, 533, 534, 535, 536, 537, 538, 539, 540, 541, 542, 543, 544, 545, 546, 547, 548, 549, 550, 551, 552, 553, 554, 555, 556, 557, 558, 559, 560, 561, 562, 563, 564, 565, 566, 567, 568, 569, 570, 571, 572, 573, 574, 575, 576, 577, 578, 579, 580, 581, 582, 583, 584, 585, 586, 587, 588, 589, 590, 591, 592, 593, 594, 595, 596, 597, 598, 599, 600, 601, 602, 603, 604, 605, 606, 607, 608, 609, 610, 611, 612, 613, 614, 615, 616, 617, 618, 619, 620, 621, 622, 623, 624, 625, 626, 627, 628, 629, 630, 631, 632, 633, 634, 635, 636, 637, 638, 639, 640, 641, 642, 643, 644, 645, 646, 647, 648, 649, 650, 651, 652, 653, 654, 655, 656, 657, 658, 659, 660, 661, 662, 663, 664, 665, 666, 667, 668, 669, 670, 671, 672, 673, 674, 675, 676, 677, 678, 679, 680, 681, 682, 683, 684, 685, 686, 687, 688, 689, 690, 691, 692, 693, 694, 695, 696, 697, 698, 699, 700, 701, 702, 703, 704, 705, 706, 707, 708, 709, 710, 711, 712, 713, 714, 715, 716, 717, 718, 719, 720, 721, 722, 723, 724, 725, 726, 727, 728, 729, 730, 731, 732, 733, 734, 735, 736, 737, 738, 739, 740, 741, 742, 743, 744, 745, 746, 747, 748, 749, 750, 751, 752, 753, 754, 755, 756, 757, 758, 759, 760, 761, 762, 763, 764, 765, 766, 767, 768, 769, 770, 771, 772, 773, 774, 775, 776, 777, 778, 779, 780, 781, 782, 783, 784, 785, 786, 787, 788, 789, 790, 791, 792, 793, 794, 795, 796, 797, 798, 799, 800, 801, 802, 803, 804, 805, 806, 807, 808, 809, 810, 811, 812, 813, 814, 815, 816, 817, 818, 819, 820, 821, 822, 823, 824, 825, 826, 827, 828, 829, 830, 831, 832, 833, 834, 835, 836, 837, 838, 839, 840, 841, 842, 843, 844, 845, 846, 847, 848, 849, 850, 851, 852, 853, 854, 855, 856, 857, 858, 859, 860, 861, 862, 863, 864, 865, 866, 867, 868, 869, 870, 871, 872, 873, 874, 875, 876, 877, 878, 879, 880, 881, 882, 883, 884, 885, 886, 887, 888, 889, 890, 891, 892, 893, 894, 895, 896, 897, 898, 899, 900, 901, 902, 903, 904, 905, 906, 907, 908, 909, 910, 911, 912, 913, 914, 915, 916, 917, 918, 919, 920, 921, 922, 923, 924, 925, 926, 927, 928, 929, 930, 931, 932, 933, 934, 935, 936, 937, 938, 939, 940, 941, 942, 943, 944, 945, 946, 947, 948, 949, 950, 951, 952, 953, 954, 955, 956, 957, 958, 959, 960, 961, 962, 963, 964, 965, 966, 967, 968, 969, 970, 971, 972, 973, 974, 975, 976, 977, 978, 979, 980, 981, 982, 983, 984, 985, 986, 987, 988, 989, 990, 991, 992, 993, 994, 995, 996, 997, 998, 999, 1000], 50, [501, 502, 503, 504, 505, 506, 507, 508, 509, 510, 511, 512, 513, 514, 515, 516, 517, 518, 519, 520, 521, 522, 523, 524, 525, 526, 527, 528, 529, 530, 531, 532, 533, 534, 535, 536, 537, 538, 539, 540, 541, 542, 543, 544, 545, 546, 547, 548, 549, 550, 551, 552, 553, 554, 555, 556, 557, 558, 559, 560, 561, 562, 563, 564, 565, 566, 567, 568, 569, 570, 571, 572, 573, 574, 575, 576, 577, 578, 579, 580, 581, 582, 583, 584, 585, 586, 587, 588, 589, 590, 591, 592, 593, 594, 595, 596, 597, 598, 599, 600, 601, 602, 603, 604, 605, 606, 607, 608, 609, 610, 611, 612, 613, 614, 615, 616, 617, 618, 619, 620, 621, 622, 623, 624, 625, 626, 627, 628, 629, 630, 631, 632, 633, 634, 635, 636, 637, 638, 639, 640, 641, 642, 643, 644, 645, 646, 647, 648, 649, 650, 651, 652, 653, 654, 655, 656, 657, 658, 659, 660, 661, 662, 663, 664, 665, 666, 667, 668, 669, 670, 671, 672, 673, 674, 675, 676, 677, 678, 679, 680, 681, 682, 683, 684, 685, 686, 687, 688, 689, 690, 691, 692, 693, 694, 695, 696, 697, 698, 699, 700, 701, 702, 703, 704, 705, 706, 707, 708, 709, 710, 711, 712, 713, 714, 715, 716, 717, 718, 719, 720, 721, 722, 723, 724, 725, 726, 727, 728, 729, 730, 731, 732, 733, 734, 735, 736, 737, 738, 739, 740, 741, 742, 743, 744, 745, 746, 747, 748, 749, 750, 751, 752, 753, 754, 755, 756, 757, 758, 759, 760, 761, 762, 763, 764, 765, 766, 767, 768, 769, 770, 771, 772, 773, 774, 775, 776, 777, 778, 779, 780, 781, 782, 783, 784, 785, 786, 787, 788, 789, 790, 791, 792, 793, 794, 795, 796, 797, 798, 799, 800, 801, 802, 803, 804, 805, 806, 807, 808, 809, 810, 811, 812, 813, 814, 815, 816, 817, 818, 819, 820, 821, 822, 823, 824, 825, 826, 827, 828, 829, 830, 831, 832, 833, 834, 835, 836, 837, 838, 839, 840, 841, 842, 843, 844, 845, 846, 847, 848, 849, 850, 851, 852, 853, 854, 855, 856, 857, 858, 859, 860, 861, 862, 863, 864, 865, 866, 867, 868, 869, 870, 871, 872, 873, 874, 875, 876, 877, 878, 879, 880, 881, 882, 883, 884, 885, 886, 887, 888, 889, 890, 891, 892, 893, 894, 895, 896, 897, 898, 899, 900, 901, 902, 903, 904, 905, 906, 907, 908, 909, 910, 911, 912, 913, 914, 915, 916, 917, 918, 919, 920, 921, 922, 923, 924, 925, 926, 927, 928, 929, 930, 931, 932, 933, 934, 935, 936, 937, 938, 939, 940, 941, 942, 943, 944, 945, 946, 947, 948, 949, 950, 951, 952, 953, 954, 955, 956, 957, 958, 959, 960, 961, 962, 963, 964, 965, 966, 967, 968, 969, 970, 971, 972, 973, 974, 975, 976, 977, 978, 979, 980, 981, 982, 983, 984, 985, 986, 987, 988, 989, 990, 991, 992, 993, 994, 995, 996, 997, 998, 999, 1000], 50]

Sort  (cost=202577.23..202702.23 rows=50000 width=76) (actual time=18855.770..18900.155 rows=26000 loops=1)
  Sort Key: test_executions.test_case_id, test_executions.id DESC
  Sort Method: quicksort  Memory: 3004kB
  Buffers: shared hit=124727 read=8155
  ->  Hash Join  (cost=194120.31..196452.32 rows=50000 width=76) (actual time=18441.703..18797.094 rows=26000 loops=1)
        Hash Cond: (test_executions.test_run_id = test_runs.id)
        Buffers: shared hit=124727 read=8155
        ->  Nested Loop  (cost=193915.19..195559.82 rows=50000 width=33) (actual time=18400.691..18668.417 rows=26000 loops=1)
              Buffers: shared hit=124667 read=8155
              ->  HashAggregate  (cost=193914.76..193916.76 rows=200 width=8) (actual time=18400.631..18447.748 rows=26000 loops=1)
                    Group Key: test_executions_1.id
                    Batches: 1  Memory Usage: 1825kB
                    Buffers: shared hit=20667 read=8155
                    ->  HashAggregate  (cost=192789.76..193289.76 rows=50000 width=12) (actual time=18306.951..18353.329 rows=26000 loops=1)
                          Group Key: anon_2.test_case_id, test_executions_1.id
                          Batches: 1  Memory Usage: 2577kB
                          Buffers: shared hit=20667 read=8155
                          ->  Append  (cost=0.43..192539.76 rows=50000 width=12) (actual time=0.078..18259.869 rows=26500 loops=1)
                                Buffers: shared hit=20667 read=8155
                                ->  Nested Loop  (cost=0.43..1306.25 rows=25000 width=12) (actual time=0.074..186.391 rows=25000 loops=1)
                                      Buffers: shared hit=1501
                                      ->  Function Scan on unnest anon_2  (cost=0.00..5.00 rows=500 width=4) (actual time=0.030..0.777 rows=500 loops=1)
                                      ->  Limit  (cost=0.43..2.10 rows=50 width=8) (actual time=0.011..0.225 rows=50 loops=500)
                                            Buffers: shared hit=1501
                                            ->  Index Only Scan Backward using ix_exec_case_id on test_executions test_executions_1  (cost=0.43..67.43 rows=2000 width=8) (actual time=0.008..0.080 rows=50 loops=500)
                                                  Index Cond: (test_case_id = anon_2.test_case_id)
                                                  Heap Fetches: 0
                                                  Buffers: shared hit=1501
                                ->  Nested Loop  (cost=0.72..190983.51 rows=25000 width=12) (actual time=0.276..17997.947 rows=1500 loops=1)
                                      Buffers: shared hit=19166 read=8155
                                      ->  Function Scan on unnest anon_4  (cost=0.00..5.00 rows=500 width=4) (actual time=0.037..0.887 rows=500 loops=1)
                                      ->  Limit  (cost=0.72..381.46 rows=50 width=8) (actual time=0.063..35.982 rows=3 loops=500)
                                            Buffers: shared hit=19166 read=8155
                                            ->  Nested Loop  (cost=0.72..7478.44 rows=982 width=8) (actual time=0.060..35.970 rows=3 loops=500)
                                                  Join Filter: (((repos_1.default_branch IS NULL) OR ((test_runs_1.branch)::text = (repos_1.default_branch)::text)) AND (test_runs_1.project_id = projects_1.id))
                                                  Rows Removed by Join Filter: 3997
                                                  Buffers: shared hit=19166 read=8155
                                                  ->  Nested Loop  (cost=0.72..7406.34 rows=2000 width=21) (actual time=0.015..15.501 rows=2000 loops=500)
                                                        Buffers: shared hit=19164 read=8155
                                                        ->  Index Scan Backward using ix_exec_case_id on test_executions test_executions_2  (cost=0.43..6648.29 rows=2000 width=12) (actual time=0.008..3.457 rows=2000 loops=500)
                                                              Index Cond: (test_case_id = anon_4.test_case_id)
                                                              Buffers: shared hit=13164 read=8155
                                                        ->  Memoize  (cost=0.29..0.46 rows=1 width=17) (actual time=0.002..0.002 rows=1 loops=1000000)
                                                              Cache Key: test_executions_2.test_run_id
                                                              Cache Mode: logical
                                                              Hits: 998000  Misses: 2000  Evictions: 0  Overflows: 0  Memory Usage: 237kB
                                                              Buffers: shared hit=6000
                                                              ->  Index Scan using test_runs_pkey on test_runs test_runs_1  (cost=0.28..0.45 rows=1 width=17) (actual time=0.002..0.002 rows=1 loops=2000)
                                                                    Index Cond: (id = test_executions_2.test_run_id)
                                                                    Buffers: shared hit=6000
                                                  ->  Materialize  (cost=0.00..2.10 rows=2 width=9) (actual time=0.001..0.004 rows=2 loops=1000000)
                                                        Buffers: shared hit=2
                                                        ->  Nested Loop  (cost=0.00..2.09 rows=2 width=9) (actual time=0.038..0.068 rows=2 loops=1)
                                                              Join Filter: (projects_1.repo_id = repos_1.id)
                                                              Rows Removed by Join Filter: 1
                                                              Buffers: shared hit=2
                                                              ->  Seq Scan on projects projects_1  (cost=0.00..1.02 rows=2 width=8) (actual time=0.009..0.013 rows=2 loops=1)
                                                                    Buffers: shared hit=1
                                                              ->  Materialize  (cost=0.00..1.03 rows=2 width=9) (actual time=0.007..0.012 rows=2 loops=2)
                                                                    Buffers: shared hit=1
                                                                    ->  Seq Scan on repos repos_1  (cost=0.00..1.02 rows=2 width=9) (actual time=0.010..0.014 rows=2 loops=1)
                                                                          Buffers: shared hit=1
              ->  Index Scan using test_executions_pkey on test_executions  (cost=0.43..8.40 rows=1 width=33) (actual time=0.003..0.003 rows=1 loops=26000)
                    Index Cond: (id = test_executions_1.id)
                    Buffers: shared hit=104000
        ->  Hash  (cost=155.12..155.12 rows=4000 width=51) (actual time=40.985..41.013 rows=4000 loops=1)
              Buckets: 4096  Batches: 1  Memory Usage: 365kB
              Buffers: shared hit=60
              ->  Hash Join  (cost=2.12..155.12 rows=4000 width=51) (actual time=16.165..34.708 rows=4000 loops=1)
                    Hash Cond: (test_runs.project_id = projects.id)
                    Buffers: shared hit=60
                    ->  Seq Scan on test_runs  (cost=0.00..98.00 rows=4000 width=50) (actual time=0.008..5.946 rows=4000 loops=1)
                          Buffers: shared hit=58
                    ->  Hash  (cost=2.09..2.09 rows=2 width=9) (actual time=16.134..16.151 rows=2 loops=1)
                          Buckets: 1024  Batches: 1  Memory Usage: 9kB
                          Buffers: shared hit=2
                          ->  Nested Loop  (cost=0.00..2.09 rows=2 width=9) (actual time=16.099..16.132 rows=2 loops=1)
                                Join Filter: (projects.repo_id = repos.id)
                                Rows Removed by Join Filter: 1
                                Buffers: shared hit=2
                                ->  Seq Scan on projects  (cost=0.00..1.02 rows=2 width=8) (actual time=0.005..0.010 rows=2 loops=1)
                                      Buffers: shared hit=1
                                ->  Materialize  (cost=0.00..1.03 rows=2 width=9) (actual time=8.038..8.043 rows=2 loops=2)
                                      Buffers: shared hit=1
                                      ->  Seq Scan on repos  (cost=0.00..1.02 rows=2 width=9) (actual time=16.069..16.072 rows=2 loops=1)
                                            Buffers: shared hit=1
Planning:
  Buffers: shared hit=76
Planning Time: 0.789 ms
JIT:
  Functions: 74
  Options: Inlining false, Optimization false, Expressions true, Deforming true
  Timing: Generation 1.443 ms (Deform 0.597 ms), Inlining 0.000 ms, Optimization 0.777 ms, Emission 15.482 ms, Total 17.702 ms
Execution Time: 18945.762 ms
```

</details>

## Reproduce

Fixture, after `alembic upgrade head` on an empty database (`score_window` left at its
default of 50):

```sql
-- Seed for the #20 EXPLAIN measurement (recorded in this file's plans).
-- Two Repos at 500 Tests x 2,000 Executions each, on an alembic-head database:
--   repo 1 'acme/plentiful' - Default branch 'main', 1 in 50 Runs off-Default
--   repo 2 'acme/scarce'    - Default branch 'main', exactly 3 Default-branch Runs
INSERT INTO repos (id, name, default_branch, created_at) VALUES
  (1, 'acme/plentiful', 'main', now()),
  (2, 'acme/scarce', 'main', now());

INSERT INTO projects (id, repo_id, name, root, created_at) VALUES
  (1, 1, 'default', '', now()),
  (2, 2, 'default', '', now());

-- Tests 1..500 in the plentiful Repo, 501..1000 in the scarce one.
INSERT INTO test_cases (id, project_id, fingerprint, name, last_seen_at)
SELECT g, 1 + ((g - 1) / 500), md5('tc' || g), 'test_' || g, now()
FROM generate_series(1, 1000) g;

-- 2,000 Runs per Repo, ids interleaved across Repos like real ingest.
-- Repo 1: every 50th Run on a feature branch, the rest on main.
-- Repo 2: only every 666th Run on main (g = 666, 1332, 1998: 3 Default Runs).
INSERT INTO test_runs (id, project_id, commit_sha, branch, ci_run_id, ci_run_attempt, created_at)
SELECT 2 * (g - 1) + p, p, md5('r' || p || '-' || g),
       CASE WHEN p = 1 THEN CASE WHEN g % 50 = 0 THEN 'feat/x' ELSE 'main' END
            ELSE CASE WHEN g % 666 = 0 THEN 'main' ELSE 'feat/topic-' || (g % 7) END END,
       'run-' || p || '-' || g, 1,
       now() - ((2000 - g) || ' minutes')::interval
FROM generate_series(1, 2000) g, generate_series(1, 2) p;

-- Every Run executes every Test of its Repo: 2,000 Executions per Test,
-- ids ascending with the Run order, 1 in 16 failing.
INSERT INTO test_executions (id, test_case_id, test_run_id, status, attempt, failure_category, created_at)
SELECT 500 * (r.id - 1) + t, 500 * (r.project_id - 1) + t, r.id,
       CASE WHEN (500 * (r.id - 1) + t) % 16 = 0 THEN 'failed' ELSE 'passed' END, 0,
       CASE WHEN (500 * (r.id - 1) + t) % 16 = 0 THEN 'assertion' ELSE NULL END,
       r.created_at
FROM test_runs r, generate_series(1, 500) t;

SELECT setval('test_runs_id_seq', (SELECT max(id) FROM test_runs));
SELECT setval('test_executions_id_seq', (SELECT max(id) FROM test_executions));
SELECT setval('test_cases_id_seq', (SELECT max(id) FROM test_cases));
ANALYZE;
```

Then, from each tree (the base commit `49b36e7`, then this head), capture the statement the
app sends and plan it:

```python
event.listen(engine.sync_engine, "before_cursor_execute", listener)  # keeps the SQL + binds
async with factory() as db:
    await processing.rescore(db, test_ids_of_one_repo)  # one chunk, per shape
# EXPLAIN (ANALYZE, BUFFERS) <captured statement> run through asyncpg with the captured binds
```

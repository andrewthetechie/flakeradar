# Review of PR #135 at `f9330fd`

The findings tasks 16–31 fix. These are review records: `ledger.md` tracks their state, and the three findings files stay as written, except for the correction below.

| File | Holds |
|---|---|
| `ledger.md` | Every finding, its owning task and its state |
| `spec-findings.md` | Python parity, tested side by side. `S1`–`S13` are its numbered sections |
| `standards-findings.md` | Repo standards and `ms-rust`: `H0`–`H20`, `J1`–`J18` |
| `clean-code-findings.md` | Clean Code: `A`, `B`, `C` and `T` |
| `repro/` | The scripts behind the findings |
| `RE-REVIEW.md` | Written by task 31 |

**Correction.** `standards-findings.md` first called `J1` inherited from Python. `S2`'s run disproved that, so its text now says so. Treat it as must-fix.

**Conflict.** `A3` says closing the issue can stay on a store failure. `S13` and the reference say the issue stays open. Task 18's contract, which follows the reference, decides.

## The repro scripts

They are scratch tools, kept as evidence and starting points, and they were never in CI. `<S>` and `<repo>` stand for the scratch directory they ran in and the repo root. Adjust those paths, ports and databases to run them.

| Directory | What it is |
|---|---|
| `repro/proofs/` | A scratch crate depending on the repo's crates by path. Each test asserts the defect, so "pass" meant "reproduced" at `f9330fd`. `release_depth_main.rs` is the parser depth harness for the release profile |
| `repro/differential/` | Python scripts sending the same requests to both servers. `t6` row locks, `t9` Postgres restart, `t10` transport stop, `t3` processing dumps, `t8` ingest |
| `repro/fake-github/` | The fake GitHub, plus scripts that start and reset both servers |
| `repro/parser-compare/` | Runs one set of JUnit inputs through both parsers |

### Running both servers

1. Extract the reference: `git archive 86c98804 backend | tar -x -C "$TMP/python-base"`, and install its dependencies in a virtualenv.
2. Start Postgres 17 in Docker, with one database per server.
3. For GitHub runs, point both servers at the fake. Python: patch `API_BASE` at start-up (`fake-github/gh_main.py`). Rust: use a scratch copy of the source, with the API base read from an env var. That one line is the only source change, and it stays out of the repo.

Normalize timestamps before comparing dumps, and reset both databases between runs so ids stay aligned. `t8.out`'s ids had drifted, so its output was noise and was not reviewed.

# 23 — Differential sweep

Read `00-fixes-context.md` first. Ledger ids `D1`–`D8`, from "Not finished" in `spec-findings.md`.

## Goal

The surface the review never compared has been run through both servers, and every difference is fixed or logged.

## Method

Run the Python reference and the Rust binary side by side, each on its own Postgres database, and send both the same requests. `review-135/README.md` gives the setup. Compare status, JSON and resulting rows. Save the battery under `review-135/repro/differential/` so it can be rerun.

**Exhaust the parameters.** Generate requests from each route's real parameter list. The earlier battery used `limit` and `offset`, which are not real parameters, and so tested nothing on those routes.

## Surface

| Id | What to compare |
|---|---|
| `D1` | Every MCP tool, with valid, string-typed, missing and out-of-range arguments, and the write-tool rules including `replace=true` |
| `D2` | Every read route with `page`, `page_size`, `file` and each filter, alone and in combination. Then read `queries.py` and `reads.rs` side by side |
| `D3` | Failed-Report paths: retries, final `failed` state, and the rows left behind |
| `D4` | Concurrency: two instances, upload during a prune, admin delete during processing, deadlocks |
| `D5` | Ingest: size limits at and past each boundary, gzip, content types, the writers-app `curl`, and all of `t8.py`, freshly rerun with ids reset |
| `D6` | Retention and webhook delivery, which were only read before |
| `D7` | Acceptance criteria of tasks 01, 02, 03, 12 and 13, one by one |
| `D8` | Scope: anything built that no task asks for |

## Rules

A difference gets one of three outcomes: fixed to the reference with a red test first, logged in `differences.md` with a reason and any approval it needs, or stopped on as `BLOCKED`.

## Done when

- [ ] Each `D` row is closed in the ledger with the request count and the differences found, with the battery saved in the repo.
- [ ] A final rerun of the battery shows only differences that `differences.md` lists.
- [ ] The shared rules in `00-fixes-context.md` are met.

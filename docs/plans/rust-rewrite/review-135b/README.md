# Second review of PR #135 at `ac5e6f3`

The findings tasks 32–44 fix. `ledger.md` tracks their state, and the findings files are review records. Start a run with `RELAY-PROMPT.md`.

| File | Holds |
|---|---|
| `ledger.md` | Every finding, its owning task and its state |
| `spec-findings.md` | `SP1`–`SP15`, parity and the record |
| `standards-findings.md` | `ST1`–`ST34`, repo standards and `ms-rust` |
| `clean-code-findings.md` | `CC1`–`CC25` |
| `repro/spec/` | The scripts behind `SP1`–`SP4`, and in `second-pass/` the ones behind `SP8`–`SP15`. Outputs are not kept; rerun them |
| `repro/clean-code/clusters.py` | Groups `dups.py`'s windows into real clusters, for `CC24` and `CC25` |
| `RE-REVIEW.md` | Written by task 44 |

## What the reviewer re-ran at `ac5e6f3`

| Check | Result |
|---|---|
| CI | 18 pass, 1 skipping (`Merge the manifest list`), 0 fail |
| `battery/run.sh all` | 529 + 31 comparisons, 0 new differences. The report's "2733" sums cumulative phase counts |
| Full suite under ten CPU spinners, three runs | 1103/1103 each, no retries |
| `dups.py 8`, production | 59 pairs / 639 lines → 24 / 267 |
| `dups.py 8`, tests | 391 pairs / 3833 lines → 652 / 6193 |
| `dups.py 8`, task 27's four files | 7 / 77 reproduced at `53cbd17`, 5 / 58 at head |
| Browser, by hand, against the real binary and `samples/simulate_ci.py` data | Tests, Jobs, both details, the CI-run view and a quarantine write all work. No console errors, and every API call answered 200 |
| `<failure xmlns="">` through `POST /api/ingest` | Stored `passed`. `SP2` |
| Reference venv: numbers and numeric strings through Pydantic's `datetime` | `SP8` and `SP9` confirmed by the reviewer as well as the subagent |

## Limits of this review

- The `martin-clean-code` skill is user-invocation-only and was not applied. The clean-code axis used general clean-code principles and the first review's categories.
- The review ran two passes. The first stopped early at the owner's request, on budget. The second, also at the owner's request, finished each axis one at a time, with a capped subagent. Nits are listed, not written up. Task 44 lists what neither pass reached, and the third review starts there.
- `SP1` was measured at `parse_junit_xml`, not through a running server, because it would exhaust the machine. The abort is inferred from the measured growth.
- `SP6` and `ST16` are confirmed by reading, not by running. So are `ST24`, `ST25` and `ST28`–`ST30`. `ST31`, `ST33` and `ST34` are PLAUSIBLE, and their tasks test them first.

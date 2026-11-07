# 43 — The record and the missing controls

Read `00-fixes-2-context.md` first. Findings: `SP5`, `SP7`, `SP10`, `SP13`, `SP14`, `SP15`, `E5` (from the first review). No behaviour change.

## Contracts

**`E5`'s four controls.** Write the negative controls for the OpenAPI type and `required` comparison: a retyped field, a dropped `required` entry, a port-only `required` entry, and a field widened to nullable. Each is a test feeding a mutated document through the comparator. If one passes unnoticed, that is a bug in the check: fix it. `E5`'s row then cites function names.

**`differences.md` follows the proposed table.** Rows `:122`, `:123`, `:128`, `:129`, the naive timestamp row and the name-less part row cite `00-fixes-2-context.md`, and "Needs the owner's decision" comes out of `:123`, `RE-REVIEW.md` and the PR body. Rows `:108` and `:109` are as tasks 32 and 33 left them.

**Say one thing about reconciliation.** `differences.md:50-53` says `record.py --verify` "fails" in the present tense, `:87-95` says it is deleted, and `RE-REVIEW.md:216-219` says reconciliation is manual, while `tests/common/allowances.rs:173`'s `reconcile` and its test exist. Read that code, and state what it checks and what nothing checks. Correct `HANDOFF.md`'s matching risk row.

**The second pass's rows follow the proposed table.** `SP10` and `SP14` widen row `:124`. `SP13` corrects row `:115` and adds one row for its table. These change no code, so the row is the whole fix, and each row cites `00-fixes-2-context.md`.

**Small record fixes.**

- `SP15`: `tests/parity/README.md:6` names `97b933d` for every scenario, while `failed-report` and `github` were recorded at `86c98804`. Correct it, and say how those two were recorded, since `tools/parity-record` is gone.

- `RE-REVIEW.md:159-160` says the floor "stays 94".
- The battery report's first line sums cumulative phase counts ("2733 comparisons" for 529). Fix `harness`/`main.py` so it prints the total.
- Add a top note to `review-135/ledger.md` naming the rows the second review reopened, pointing at `review-135b/ledger.md`. The old findings files stay as written.

## Done when

- [ ] All four controls exist and failed against a deliberately weakened comparator. Say how in `STATUS.md`.
- [ ] A reader of `differences.md` finds no row awaiting a decision and no contradiction on reconciliation.
- [ ] The shared rules are met.

# 25 — Domain types

Read `00-fixes-context.md` first. Behaviour stays fixed. Findings: `H6`, `B4`, `B9`, `B12`, `J10`, `J15`, `H17`, `C3`.

## Goal

The compiler catches swapped and invalid domain values, and the wire stays byte-identical.

## Contracts

- **Newtypes (`H6`).** `Fingerprint`, `Score` and `CiRunId`, with fallible constructors and no infallible `From`. `Score` refuses NaN and values outside 0–1. `serde(transparent)` keeps the wire unchanged.
- **Enums (`B4`).** Report status and kind, Execution status and Failure category use enums across DTOs and processing. One `EntityKind` replaces the three Test-or-Job enums.
- **Named values (`B9`, `J15`).** Named structs replace positional tuples such as `test_counts`, `branch_scoped_score`'s history and the day counts. Fix the argument order mismatch at `reads.rs:1846`.
- **Tier rule (`B12`).** One tier type replaces the three booleans, and `Board` stops copying `ListOptions`.
- **Invariants (`J10`).** `Scope` keeps its fields private, and `mcp.rs` goes through its constructor.
- **Glossary (`H17`, `C3`).** Rust-only names follow `CONTEXT.md`. Wire and column names stay.

## Done when

- [ ] Swapping a commit SHA and a branch in the scoring history no longer compiles.
- [ ] Goldens replay with `allowed-differences.json` untouched, and a test proves `Score`'s refusals.
- [ ] A grep of `CONTEXT.md`'s avoided terms over Rust identifiers shows only wire or column names.
- [ ] The shared rules in `00-fixes-context.md` are met.

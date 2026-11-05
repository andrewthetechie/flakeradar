# 19 — Parser safety and parity

Read `00-fixes-context.md` first. Findings: `H0`, `S3`, `H3`, `H19` (non-finite time), `S9`, `S10`, `S13` (second bullet), `J18`.

## Goal

No JUnit input within the size limit can abort the process, and parsing matches Python, because a changed fingerprint means a different Test.

## Contracts

**Safety.**

- Suite nesting cannot overflow any stack: walk with an explicit stack, or refuse past a documented `const` depth. Check all recursion and all of `roxmltree`'s limits, including at the other parse sites.
- Allocation for any body within the ingest limit stays under a documented budget. Share repeated identity strings (`Arc<str>`) or refuse past a cumulative byte budget, and check that the copies in `processing.rs` (`J5`) do not bring the growth back.
- Correct the module doc's claim that sizes are capped here.

**Parity (the `S9` and `S10` tables).**

- Failure text merges across comments and processing instructions, as ElementTree does.
- Encoding labels follow Python's codecs: ISO-8859-1 is true Latin-1, and `US-ASCII` falls back as Python does. Declared UTF-16 with no BOM is accepted.
- Namespaced elements and attributes are not treated as JUnit names, and `<testsuite xmlns="urn:x">` is refused as in Python.
- `DOCTYPE` follows the approved table. Bound nodes with `nodes_limit`.
- Invalid character references (`&#xD800;`, `&#x110000;`) are refused.
- A `line` past range with no `file` no longer fails the Report (`processing.rs:582`), because Python writes `line` only when `file` is set.
- Non-finite `time` follows the approved table.
- The `Nd` digit table handles U+1D7CE..U+1D7FF (`J18`).

Run every input in both parsers, using `review-135/repro/parser-compare/`, and store Python's output as the expected value. New goldens follow the "What stays fixed" rule.

## Done when

- [ ] A 5,000-depth and a 200,000-depth upload through the real route each get the planned 4xx with the server still up, and with the repo's release profile on a 2 MiB stack (`repro/proofs/release_depth_main.rs`).
- [ ] A body at the ingest limit with a long name and many cases stays under the budget, by a test that asserts the bound rather than a time.
- [ ] Each `S9` and `S10` row has a test whose expected value came from the reference, and `differences.md` has a row for any exception, with the owner's approval.
- [ ] `parser_robustness.rs` covers deep nesting and entity bombs.
- [ ] The JUnit corpus goldens replay unchanged.
- [ ] The shared rules in `00-fixes-context.md` are met.

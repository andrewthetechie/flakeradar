# 32 — Parser budget that holds for every shape

Read `00-fixes-2-context.md` first. Finding: `SP1` (reopens `H3`). Must-fix.

## Goal

`00-fixes-context.md` requires that "an entity bomb gets a 4xx, never an abort". `differences.md:108` and `PARSE_BYTES_MAX` claim the budget holds "whatever shape it takes". Neither is true today:

- A 1 MiB body with one 512 KiB internal entity, referenced about 178,000 times in an attribute, passes 3 GiB of heap at `parse_junit_xml`.
- The same trick in `<failure>` text, and 60,000 nested namespace prefixes, ran past 120 s of CPU at 20 MiB.

roxmltree 0.21.2 allows unlimited references at depth zero. `nodes_limit` counts nodes, not bytes. `IDENTITY_BYTES_MAX` is checked only after parsing returns.

## Contracts

- Read `spec-findings.md` `SP1` for the exact bodies. Check the claim first with an allocation-counting test. Do **not** POST these bodies to a running server: that would exhaust the machine.
- Before calling `Document::parse_with_options`, bound the work. The likely shape is a linear pre-scan of the body:
  - Sum each internal entity's value length times its reference count, conservatively counting nested entities.
  - Count namespace declarations and nesting.
  - Refuse with the 422 a bad report gets when the bound passes the budget.
  - Another mechanism is fine if it meets the test below. Replacing the XML parser is out of scope and is `BLOCKED`.
- The pre-scan must not refuse a document the reference parses when its expansion is within budget. Keep DOCTYPE accepted, as the approved table says.
- Also bound CPU. Five million math digits in `time` took 19.5 s on one blocking thread in the reviewer's run. Decide whether a `time` length cap is the same 400 a non-numeric `time` gets, and test it.

## Done when

- [ ] A table-driven test runs at least these shapes under an allocation-counting allocator and asserts peak heap under `PARSE_BYTES_MAX`, with a bounded answer for each: `attr_1mib`, `attr_entity_quadratic`, `text_entity_quadratic`, `namespace_quadratic`, `time_math_digits`, `many_attrs`, the classic nested bomb, and a realistic 20 MiB failure-heavy report that must still parse.
- [ ] Each shape's red is in the `STATUS.md` note. A run that would exhaust memory stops at the counting allocator's limit, as the review's harness did.
- [ ] `repro/proofs/release_depth_main` still answers without an abort in the release profile.
- [ ] `differences.md:108`, `parsing.rs`'s module doc and `PARSE_BYTES_MAX`'s doc say only what the test proves.
- [ ] The shared rules are met.

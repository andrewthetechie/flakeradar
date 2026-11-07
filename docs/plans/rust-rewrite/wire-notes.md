# Where the wire rules came from

The code states the rules this API enforces and stops there: a doc comment says
what a value may be and what a refusal answers, not who measured it first. This
file is where that provenance lives, one row per area.

Every rule below was measured against the reference at `86c98804` while the Rust
port was written, and `tests/parity/` (goldens, scenarios, `allowed-differences.json`)
or a corpus under `crates/flakeradar-core/tests/fixtures/` pins it. A golden is
immutable: see `differences.md` for the rules that changed on purpose. The code
itself does not repeat this: a comment states the rule and what the answer is, and
who measured it first lives here, so a reader never needs the replaced tree.

| Area | Where the rule lives now | What pins it |
|---|---|---|
| The lax reading of a request body: which spellings a boolean, an integer, a timestamp and a string take, the order issues are reported in, and each issue's `type`, `msg` and `loc` | `flakeradar_core::lax`, `flakeradar_core::dto::lax_bool`, `flakeradar_core::dto::lax_int` | `crates/flakeradar/tests/validation.rs`, `crates/flakeradar-core/tests/lax.rs` and `dto.rs`, the `validation` scenario |
| The 422 shape: `detail` as a list of issues, `loc` naming the request part and the field, `input` echoing the raw query text | `flakeradar::extract`, `flakeradar::error` | `crates/flakeradar/tests/extractors.rs`, every scenario's 422 steps |
| The JUnit reading: identity, outcome, Location, retries, `time` parsing, encodings, nesting | `flakeradar_core::parsing` with `flakeradar_core::py` for the string rules | the JUnit corpus under `tests/parity/golden/junit/` and `crates/flakeradar-core/tests/parser_robustness.rs` |
| The encoding label a declaration names, and what each byte of that code page decodes to | `flakeradar_core::encodings`, whose tables and fixture are written by `docs/plans/rust-rewrite/review-135b/repro/spec/genencodings.py` from `CPython`'s own `codecs` module (measured label by label, byte by byte, at 86c98804) | `crates/flakeradar-core/tests/fixtures/encodings.tsv` replayed by `every_label_and_byte_decodes_as_its_table_says`, and `differences.md` for the stateful and EBCDIC labels this server refuses |
| The XML declaration, byte-order marks, `DOCTYPE`s, external subsets and `ATTLIST` defaults a report may carry | `flakeradar_core::parsing`, one rule per shape; `roxmltree` cannot express several of them, and `differences.md` holds one row each | `crates/flakeradar-core/tests/declarations.rs`, `parser_robustness.rs` and `parse_budget.rs`, each literal measured from the reference at 86c98804 |
| A Unix timestamp's magnitude rule (a number past `2e10` is milliseconds), and the sentence that refuses an instant outside the years a datetime holds | `flakeradar_core::lax::epoch_of` and `flakeradar_core::dto::ValidationIssue::datetime_parsing` | `the_pipeline_body_reads_a_timestamp_the_way_the_wire_does`, `a_millisecond_timestamp_reaches_the_stored_job_execution`, and the battery's `d7` phase, each row measured from the reference |
| The multipart framing the ingest route accepts, and what it answers for a body that is no form | `flakeradar::multipart` | `crates/flakeradar/tests/multipart.rs`, the `ingest-junit` multipart steps |
| The MCP wire: tool names, titles, descriptions, argument schemas, annotations, the `instructions` text, and the four `isError` results | `flakeradar::mcp` | the 46 `mcp` scenario steps, `crates/flakeradar/tests/mcp.rs` |
| The issue bodies and the fixed-note comment, byte for byte | `flakeradar::github::bodies` | `crates/flakeradar/tests/github/bodies.rs`, whose `RECORDED_BODY` is the body the second review's `S7` input produced on the reference at 86c98804 |
| The timestamp spelling this server emits, and the one field where it differs from the goldens | `flakeradar_core::dto::timestamp` | `crates/flakeradar/tests/parity.rs`, `differences.md` |
| The score, the Proven flake floor, the streaks and the trend arrow | `flakeradar_core::scoring`, `flakeradar_core::helpers` | the `flaky-tests` and `default-branch` dumps, `crates/flakeradar-core/src/scoring.rs` unit tests |
| The tier vocabulary, the Report and Job statuses, and the settings defaults | `flakeradar_core::model`, `flakeradar_core::defaults` | `crates/flakeradar-core/tests/model.rs`, `crates/flakeradar/src/settings.rs` unit tests |

Two rules the review of PR #135 found the two implementations disagreeing on are
deliberate, and `differences.md` carries them: an unparsable `<testcase time>`
answers 400 where the reference crashed, and the suffix of a parse-error detail
is this parser's own wording.

# Where the wire rules came from

The code states the rules this API enforces and stops there: a doc comment says
what a value may be and what a refusal answers, not who measured it first. This
file is where that provenance lives, one row per area.

Every rule below was measured against the reference at `86c98804` while the Rust
port was written, and `tests/parity/` (goldens, scenarios, `allowed-differences.json`)
or a corpus under `crates/flakeradar-core/tests/fixtures/` pins it. A golden is
immutable: see `differences.md` for the rules that changed on purpose.

| Area | Where the rule lives now | What pins it |
|---|---|---|
| The lax reading of a request body: which spellings a boolean, an integer, a timestamp and a string take, the order issues are reported in, and each issue's `type`, `msg` and `loc` | `flakeradar_core::lax`, `flakeradar_core::dto::lax_bool`, `flakeradar_core::dto::lax_int` | `crates/flakeradar/tests/validation.rs`, `crates/flakeradar-core/tests/lax.rs` and `dto.rs`, the `validation` scenario |
| The 422 shape: `detail` as a list of issues, `loc` naming the request part and the field, `input` echoing the raw query text | `flakeradar::extract`, `flakeradar::error` | `crates/flakeradar/tests/extractors.rs`, every scenario's 422 steps |
| The JUnit reading: identity, outcome, Location, retries, `time` parsing, encodings, nesting | `flakeradar_core::parsing` with `flakeradar_core::py` for the string rules | the JUnit corpus under `tests/parity/golden/junit/` and `crates/flakeradar-core/tests/parser_robustness.rs` |
| The multipart framing the ingest route accepts, and what it answers for a body that is no form | `flakeradar::multipart` | `crates/flakeradar/tests/multipart.rs`, the `ingest-junit` multipart steps |
| The MCP wire: tool names, titles, descriptions, argument schemas, annotations, the `instructions` text, and the four `isError` results | `flakeradar::mcp` | the 46 `mcp` scenario steps, `crates/flakeradar/tests/mcp.rs` |
| The issue bodies and the fixed-note comment, byte for byte | `flakeradar::github::bodies` | `crates/flakeradar/tests/github/bodies.rs` |
| The timestamp spelling this server emits, and the one field where it differs from the goldens | `flakeradar_core::dto::timestamp` | `crates/flakeradar/tests/parity.rs`, `differences.md` |
| The score, the Proven flake floor, the streaks and the trend arrow | `flakeradar_core::scoring`, `flakeradar_core::helpers` | the `flaky-tests` and `default-branch` dumps, `crates/flakeradar-core/src/scoring.rs` unit tests |
| The tier vocabulary, the Report and Job statuses, and the settings defaults | `flakeradar_core::model`, `flakeradar_core::defaults` | `crates/flakeradar-core/tests/model.rs`, `crates/flakeradar/src/settings.rs` unit tests |

Two rules the review of PR #135 found the two implementations disagreeing on are
deliberate, and `differences.md` carries them: an unparsable `<testcase time>`
answers 400 where the reference crashed, and the suffix of a parse-error detail
is this parser's own wording.

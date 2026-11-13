//! Four properties of this workspace's own source, checked from outside it.
//!
//! All are rules a compiler cannot see: that every log event carries a name and a
//! static message, that no comment or identifier in `crates/` points at the
//! implementation this one replaced, that every hand-written file carries the
//! compliance marker, and that the Report route module owns no database write.
//! Each rule is a function over the text of one file, so a test can feed it a
//! violation as well as the tree, and every violation shape this file is named
//! for has a case that runs through it.
//!
//! This file is the one the first two tree checks skip: it spells the words and the
//! macros it looks for.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a test fails loudly when the tree, or a line of its own fixture text, \
              is not what it expects"
)]

use std::path::{Path, PathBuf};

/// The workspace root, two directories above this crate's manifest.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|directory| {
            directory.join("Cargo.toml").is_file() && directory.join("crates").is_dir()
        })
        .expect("the workspace root holds Cargo.toml and crates/")
        .to_path_buf()
}

/// Every `.rs` file under `crates/`, with its text and its path from the root.
fn sources() -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut pending = vec![workspace_root().join("crates")];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("the source tree is readable") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|kind| kind == "rs") {
                let text = std::fs::read_to_string(&path).expect("a source file is readable");
                let name = path
                    .strip_prefix(workspace_root())
                    .expect("the file sits under the workspace root")
                    .to_string_lossy()
                    .into_owned();
                found.push((name, text));
            }
        }
    }
    found
}

/// The word each line must not carry: a name of the implementation this one
/// replaced, or of a step of the plan that ported it.
///
/// A task number goes stale the day the plan closes, and a reader of this code has
/// no way to look it up. The phrase forms name the replaced implementation rather
/// than stating the rule, which sends a reader to a tree that is not here.
const REPLACED: &[&str] = &[
    "python",
    "fastapi",
    "pydantic",
    "starlette",
    "asyncpg",
    "fastmcp",
    "backend/",
    "frozen",
    "the reference",
    ".py",
    "docstring",
    "elementtree",
    "sqlalchemy",
    "alembic",
];

/// The words that may still be carried, by file, with the reason.
///
/// Each entry is a file, the words it may carry, and why. Every one names an
/// input rather than the replaced implementation: a URL scheme this server accepts,
/// the version table an older install leaves behind, a field of a committed
/// fixture, a file of the sample Repo a report or an issue body carries, a key of
/// the MCP wire, and the script that generates a table.
const ALLOWED: &[(&str, &[&str], &str)] = &[
    (
        "crates/flakeradar-db/src/connect.rs",
        &["asyncpg"],
        "the postgresql+asyncpg:// scheme is one of the three URLs this server accepts",
    ),
    (
        "crates/flakeradar-db/src/error.rs",
        &["asyncpg", "alembic"],
        "the refusal names the accepted URL scheme and the version table it looks for, which are the text a caller reads",
    ),
    (
        "crates/flakeradar-db/src/migration.rs",
        &["alembic"],
        "an alembic_version table is what identifies the install this server refuses to migrate",
    ),
    (
        "crates/flakeradar-db/tests/migrations.rs",
        &["alembic"],
        "the test creates the version table whose refusal it pins",
    ),
    (
        "crates/flakeradar-db/tests/connect.rs",
        &["asyncpg"],
        "the test pins the accepted URL schemes",
    ),
    (
        "crates/flakeradar/tests/server.rs",
        &["alembic"],
        "the test creates the version table whose refusal it pins",
    ),
    (
        "crates/flakeradar-test-support/src/db.rs",
        &["asyncpg"],
        "the helper rewrites the accepted scheme, as the pool does",
    ),
    (
        "crates/flakeradar/src/settings.rs",
        &["asyncpg"],
        "the default URL and the refusal's text both name the accepted scheme",
    ),
    (
        "crates/flakeradar-core/src/encodings.rs",
        &["python"],
        "the tables are derived from CPython's own codecs",
    ),
    (
        "crates/flakeradar-core/tests/encodings.rs",
        &["python"],
        "the module doc names the CPython codecs the fixture was derived from",
    ),
    (
        "crates/flakeradar-core/tests/golden_junit.rs",
        &["pythonValue", "python_value"],
        "pythonValue is a field of tests/parity/allowed-differences.json, a committed fixture",
    ),
    (
        "crates/flakeradar/tests/common/allowances.rs",
        &["pythonValue", "python_value"],
        "the allowance reader names the fixture field it deserializes",
    ),
    (
        "crates/flakeradar/tests/parity.rs",
        &["python_value", ".py"],
        "the replay builds an allowance entry, whose field name is the fixture's, and its scenario bodies name the sample Repo's files",
    ),
    (
        "crates/flakeradar-core/tests/parsing.rs",
        &[".py"],
        "a sample report's own `file` attribute is the value the parser is asked about",
    ),
    (
        "crates/flakeradar-db/tests/processing.rs",
        &[".py"],
        "a sample report's own `file` attribute is the Location the processor stores",
    ),
    (
        "crates/flakeradar-db/tests/spike.rs",
        &[".py"],
        "a sample report's own `file` attribute is the value the query is asked about",
    ),
    (
        "crates/flakeradar/tests/consumers.rs",
        &[".py"],
        "the sample Repo's file path is what the CI-run failure listing reports",
    ),
    (
        "crates/flakeradar/tests/reads.rs",
        &[".py"],
        "the sample Repo's file path is what the leaderboard search and the permalink are asked about",
    ),
    (
        "crates/flakeradar/tests/mcp.rs",
        &[".py"],
        "the sample Repo's file path is what the MCP tools answer",
    ),
    (
        "crates/flakeradar/tests/github/bodies.rs",
        &[".py"],
        "the sample Repo's file path is inside the issue body and its permalink",
    ),
    (
        "crates/flakeradar/tests/github/filing.rs",
        &["backend/", ".py"],
        "the path is inside the sample Repo's own tree, which the issue body reports",
    ),
    (
        "crates/flakeradar/tests/github/support.rs",
        &["backend/", ".py"],
        "the fixture body names a file in the sample Repo",
    ),
    (
        "crates/flakeradar/src/mcp/mod.rs",
        &["fastmcp"],
        "x-fastmcp-wrap-result and _meta.fastmcp.tags are keys of the MCP wire",
    ),
    (
        "crates/flakeradar/src/mcp/tools/top_flaky_tests.rs",
        &[".py"],
        "the tool's instructions text names a sample file, and that text is a wire field",
    ),
];

/// The words [`ALLOWED`] lets through in `name`.
fn allowed_words(name: &str) -> Vec<&'static str> {
    ALLOWED
        .iter()
        .filter(|(file, _, _)| *file == name)
        .flat_map(|(_, words, _)| words.iter())
        .copied()
        .collect()
}

/// Whether `line` carries `word`, ignoring case except for the fixture's field.
///
/// The allowance fixture spells its field `pythonValue`, so its two spellings are
/// matched as written and are not also read as the bare language name.
fn carries(line: &str, word: &str) -> bool {
    match word {
        "pythonValue" | "python_value" => line.contains(word),
        _ => {
            let lowered = line.to_lowercase();
            carries_in(&lowered, word)
                && !lowered.contains("python_value")
                && !lowered.contains("pythonvalue")
        }
    }
}

/// Whether the lowercased `text` holds `word` as a word of its own.
///
/// A word that runs straight into more letters is not a match, which is what keeps
/// `the references in an attribute value` from reading as a comparison to the
/// replaced implementation. A word that spells a path or an extension is matched as
/// written, because that is how a deleted file is named.
fn carries_in(text: &str, word: &str) -> bool {
    if word.starts_with('.') || word.ends_with('/') {
        return text.contains(word);
    }
    text.match_indices(word)
        .any(|(at, _)| !continues_with_a_letter(text, at + word.len()))
}

/// Whether the text at `at` continues the word that ended just before it.
fn continues_with_a_letter(text: &str, at: usize) -> bool {
    text.as_bytes().get(at).is_some_and(u8::is_ascii_alphabetic)
}

/// Every way `text` names the implementation this one replaced, or a plan step.
///
/// Each entry names the file, the line and the word or pattern it carries.
fn replaced_implementation_offences(name: &str, text: &str) -> Vec<String> {
    let mut offences = Vec::new();
    let allowed = allowed_words(name);
    for (number, line) in text.lines().enumerate() {
        for word in REPLACED {
            if carries(line, word) && !allowed.contains(word) {
                offences.push(format!("{name}:{}: {word}: {}", number + 1, line.trim()));
            }
        }
        for at in task_number_matches(line) {
            offences.push(format!(
                "{name}:{}: task number at column {}: {}",
                number + 1,
                at + 1,
                line.trim()
            ));
        }
    }
    offences
}

/// Where `line` names a task number, as `task 12`, `tasks 16-31` or `Task-7`.
///
/// Every occurrence is its own match, so a second number cannot hide behind the
/// first.
fn task_number_matches(line: &str) -> Vec<usize> {
    let lowered = line.to_lowercase();
    let bytes = lowered.as_bytes();
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(offset) = lowered.get(from..).and_then(|rest| rest.find("task")) {
        let at = from + offset + "task".len();
        let mut after = at;
        if bytes.get(after).is_some_and(|ch| *ch == b's') {
            after += 1;
        }
        if matches!(bytes.get(after), Some(b' ' | b'-' | b'#' | b':')) {
            after += 1;
        }
        if bytes.get(after).is_some_and(u8::is_ascii_digit) {
            found.push(from + offset + 1);
        }
        from = at;
    }
    found
}

/// No comment or identifier in `crates/` names the replaced implementation.
#[test]
fn no_source_line_points_at_the_replaced_implementation() {
    let mut offences = Vec::new();
    for (name, text) in sources() {
        if name.ends_with("tests/source_hygiene.rs") {
            continue;
        }
        offences.extend(replaced_implementation_offences(&name, &text));
    }
    assert!(
        offences.is_empty(),
        "these lines name the replaced implementation or a plan step:\n{}",
        offences.join("\n")
    );
}

/// The marker `docs/agents/code-standards-rust.md` asks a compliant file to carry.
const COMPLIANCE_MARKER: &str = "// Rust guideline compliant 2026-07-21";

/// Whether `name` is a file that must carry the compliance marker but does not.
///
/// The SeaORM entities are generated, so there is no hand-written file to check.
fn lacks_the_marker(name: &str, text: &str) -> bool {
    !name.contains("/entities/") && !text.lines().any(|line| line == COMPLIANCE_MARKER)
}

/// Every hand-written source file carries the compliance marker.
#[test]
fn every_source_file_carries_the_compliance_marker() {
    let missing: Vec<String> = sources()
        .into_iter()
        .filter(|(name, text)| lacks_the_marker(name, text))
        .map(|(name, _)| name)
        .collect();
    assert!(
        missing.is_empty(),
        "these files lack `{COMPLIANCE_MARKER}`:\n{}",
        missing.join("\n")
    );
}

/// The marker check flags a bare file and passes a marked or generated one.
#[test]
fn the_marker_check_flags_only_a_hand_written_file_without_it() {
    let marked = format!("//! Docs.\n\n{COMPLIANCE_MARKER}\n\nuse std::fmt;\n");
    assert!(lacks_the_marker("crates/a/src/b.rs", "use std::fmt;\n"));
    assert!(!lacks_the_marker("crates/a/src/b.rs", &marked));
    assert!(!lacks_the_marker(
        "crates/flakeradar-db/src/entities/jobs.rs",
        "use sea_orm::entity::prelude::*;\n"
    ));
}

/// Every [`ALLOWED`] entry names a file that exists and a word it still carries.
///
/// An allowance nobody needs is a licence to write the word again, so a stale entry
/// fails here instead of quietly widening the rule.
#[test]
fn no_allowance_entry_is_stale() {
    let mut offences = Vec::new();
    for (file, words, _) in ALLOWED {
        let Ok(text) = std::fs::read_to_string(workspace_root().join(file)) else {
            offences.push(format!("{file}: the file no longer exists"));
            continue;
        };
        for word in words.iter().copied() {
            if !text.lines().any(|line| carries(line, word)) {
                offences.push(format!("{file}: nothing carries {word:?} any more"));
            }
        }
    }
    assert!(
        offences.is_empty(),
        "these allowances are no longer needed:\n{}",
        offences.join("\n")
    );
}

/// The violation shapes this file is named for, each fed through the check.
#[test]
fn each_replaced_implementation_violation_is_caught() {
    let text = "//! Mirrors `reports.py` and the reference's `_clear_if_closed` docstring.\n\
//! Set up in tasks 16-31, and see Task-12, plus task 7.\n\
fn an_alembic_table_an_elementtree_parser_and_sqlalchemy_are_named() {}\n";
    let found = replaced_implementation_offences("scratch.rs", text);
    for want in [
        "reports.py",
        "the reference",
        "docstring",
        "alembic",
        "elementtree",
        "sqlalchemy",
    ] {
        assert!(
            found.iter().any(|offence| offence.contains(want)),
            "{want:?} is not caught: {found:?}"
        );
    }
    // Each task number on the line is its own offence, not the first one only.
    assert_eq!(
        task_number_matches("//! Set up in tasks 16-31, and see Task-12, plus task 7.").len(),
        3,
        "every spelling of a task number on a line is matched"
    );
    assert_eq!(
        replaced_implementation_offences("scratch.rs", "//! See task 4, then task 9.\n").len(),
        2,
        "both numbers are reported"
    );
    // A line that names none of them passes, so the check is not a blanket refusal.
    let quiet =
        replaced_implementation_offences("scratch.rs", "/// The page size the listing allows.\n");
    assert!(
        quiet.is_empty(),
        "a line naming nothing is reported: {quiet:?}"
    );
    // An XML reference is not the replaced implementation.
    let xml = replaced_implementation_offences(
        "scratch.rs",
        "/// The references in an attribute value.\n",
    );
    assert!(xml.is_empty(), "an XML reference is reported: {xml:?}");
    // An allowance is per file: the same word elsewhere is an offence.
    let scheme = "// the postgresql+asyncpg:// scheme is refused\n";
    assert!(
        replaced_implementation_offences(ALLOWED[0].0, scheme).is_empty(),
        "the file whose allowance names the scheme is not reported"
    );
    assert!(
        !replaced_implementation_offences("crates/elsewhere.rs", scheme).is_empty(),
        "the same word outside the allowed file is reported"
    );
}

/// How one logging call breaks the rule.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum LoggingOffence {
    /// The event carries no `name:`.
    Unnamed,
    /// The message formats a value into its text instead of naming it as a field.
    Interpolated,
}

/// The macros that write a log event, whatever path they are reached through.
const LOG_MACROS: &[&str] = &["info!", "warn!", "error!", "debug!", "trace!", "event!"];

/// Every log event in `text` that breaks the rule, by line number.
///
/// A call is recognized with or without a path, and even when the path sits on the
/// line above, because either spelling hides an event from a search for one literal.
fn logging_offences(text: &str) -> Vec<(usize, LoggingOffence)> {
    let mut offences = Vec::new();
    for name in LOG_MACROS {
        for at in macro_calls(text, name) {
            let Some(call) = call_body(text, at) else {
                continue;
            };
            let line = line_of(text, at);
            if !call.contains("name:") {
                offences.push((line, LoggingOffence::Unnamed));
            }
            if interpolated_message(call) {
                offences.push((line, LoggingOffence::Interpolated));
            }
        }
    }
    offences.sort();
    offences
}

/// Where each call of the macro `name` starts in `text`.
fn macro_calls(text: &str, name: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut from = 0;
    while let Some(offset) = text.get(from..).and_then(|rest| rest.find(name)) {
        let at = from + offset;
        let before = text.get(..at).unwrap_or_default();
        // Reached through any path, or written plain: either is an event. Only text
        // that merely ends in the macro's letters, such as `my_info!`, is not.
        let plain_or_path = before
            .chars()
            .next_back()
            .is_none_or(|last| !(last.is_ascii_alphanumeric() || last == '_' || last == ':'))
            || before.ends_with("::");
        if plain_or_path {
            starts.push(at);
        }
        from = at + name.len();
    }
    starts
}

/// The text of the call whose macro starts at `at`, through its matching `)`.
fn call_body(text: &str, at: usize) -> Option<&str> {
    let rest = text.get(at..)?;
    let open = rest.find('(')?;
    let mut depth = 0usize;
    let mut in_text = false;
    let mut escaped = false;
    for (offset, ch) in rest.char_indices().skip(open) {
        if in_text {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_text = false;
            }
            continue;
        }
        match ch {
            '"' => in_text = true,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return rest.get(..offset + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// The one-based line `at` sits on.
fn line_of(text: &str, at: usize) -> usize {
    text.get(..at).unwrap_or_default().lines().count()
}

/// Whether the call's message formats a value into its text.
///
/// Both shapes count: the interpolation written into the message, and the message
/// built by `format!` and passed in, which a search inside the call cannot see.
fn interpolated_message(call: &str) -> bool {
    let mut in_text = false;
    let mut escaped = false;
    let mut text = String::new();
    for ch in call.chars() {
        if in_text {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => {
                    if text.contains('{') {
                        return true;
                    }
                    text.clear();
                    in_text = false;
                }
                _ => text.push(ch),
            }
        } else if ch == '"' {
            in_text = true;
        }
    }
    false
}

/// Every `tracing` event carries a `name:` and a message that formats nothing.
#[test]
fn every_log_event_is_named_and_unformatted() {
    let mut unnamed = Vec::new();
    let mut interpolated = Vec::new();
    for (name, text) in sources() {
        if name.ends_with("tests/source_hygiene.rs") {
            continue;
        }
        for (line, offence) in logging_offences(&text) {
            match offence {
                LoggingOffence::Unnamed => unnamed.push(format!("{name}:{line}")),
                LoggingOffence::Interpolated => interpolated.push(format!("{name}:{line}")),
            }
        }
    }
    assert!(
        unnamed.is_empty(),
        "these events carry no name:\n{}",
        unnamed.join("\n")
    );
    assert!(
        interpolated.is_empty(),
        "these events format a value into their message instead of naming it as a field; \
         M-LOG-STRUCTURED wants the values beside a static template:\n{}",
        interpolated.join("\n")
    );
}

/// The violation shapes the logging check is named for, each fed through it.
#[test]
fn each_logging_violation_is_caught() {
    let text = r#"
use tracing::info;
    let message = format!("value {x}");
    tracing::error!(name: "a.b.c", "{message}");
    info!("unnamed through an import {x}");
    warn!("unnamed through an import");
    tracing::event!(tracing::Level::INFO, "unnamed event macro");
    tracing::info!(name: "a.b.d", "inline format {x}");
    tracing::
        warn!("split across a line");
    tracing::info!(name: "a.b.e", count = 2, "sent a batch");
"#;
    let found = logging_offences(text);
    for needle in [
        "info!(\"unnamed through an import",
        "warn!(\"unnamed through an import\")",
        "tracing::event!(tracing::Level::INFO",
        "warn!(\"split across a line\")",
    ] {
        let line = line_holding(text, needle);
        assert!(
            found.contains(&(line, LoggingOffence::Unnamed)),
            "{needle} is not caught as an unnamed event: {found:?}"
        );
    }
    for needle in ["\"{message}\"", "inline format {x}"] {
        let line = line_holding(text, needle);
        assert!(
            found.contains(&(line, LoggingOffence::Interpolated)),
            "{needle} is not caught as a formatted message: {found:?}"
        );
    }
    // The named event with a static message is the control on the control.
    let quiet = line_holding(text, "count = 2");
    assert!(
        !found.iter().any(|(line, _)| *line == quiet),
        "a named event with a static message is reported: {found:?}"
    );
    // The pre-built message is reported on the event's own line, not the `format!`'s.
    assert_eq!(
        found
            .iter()
            .filter(|(_, offence)| matches!(offence, LoggingOffence::Interpolated))
            .count(),
        3,
        "the three events that format a value are reported once each: {found:?}"
    );
}

/// The one-based line of `needle` in `text`.
fn line_holding(text: &str, needle: &str) -> usize {
    text.lines()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("the control holds {needle:?}"))
        + 1
}

/// The shapes by which a route module would own a database write of its own.
///
/// The Report route used to hold all of them; the queue writes are
/// `flakeradar_db::report_intake`'s now, so one of these shapes in that module
/// means the split its own doc states has slipped.
const ROUTE_WRITE_SHAPES: &[&str] = &[
    "Entity::insert",
    "Entity::update",
    "Entity::delete",
    ".begin()",
    "TransactionTrait",
];

/// The Report route module: the one route whose queue writes now live in
/// `flakeradar-db`.
const REPORT_ROUTE: &str = "crates/flakeradar/src/routes/reports.rs";

/// Every line of `text` that owns a database write, one entry per shape it holds.
fn route_write_offences(text: &str) -> Vec<String> {
    let mut offences = Vec::new();
    for (number, line) in text.lines().enumerate() {
        for shape in ROUTE_WRITE_SHAPES.iter().copied() {
            if line.contains(shape) {
                offences.push(format!("{}: {shape}: {}", number + 1, line.trim()));
            }
        }
    }
    offences
}

/// The Report route module owns no entity write, no transaction open and no
/// `TransactionTrait`.
#[test]
fn the_report_route_owns_no_database_write() {
    let text = std::fs::read_to_string(workspace_root().join(REPORT_ROUTE))
        .expect("the Report route module is readable");
    let offences = route_write_offences(&text);
    assert!(
        offences.is_empty(),
        "{REPORT_ROUTE} owns a database write, which belongs in `flakeradar-db`:\n{}",
        offences.join("\n")
    );
}

/// Each write shape the route check is named for, each fed through it.
///
/// This file is the one the first two tree checks skip because it spells what it
/// looks for, so writing the shapes out here is not an offence against them.
#[test]
fn each_route_write_shape_is_reported() {
    let text = "reports::Entity::insert(new_report).exec_with_returning(&txn).await?;\n\
                reports::Entity::update(active).exec(db).await?;\n\
                reports::Entity::delete_by_id(id).exec(db).await?;\n\
                let txn = db.begin().await?;\n\
                use sea_orm::TransactionTrait;\n\
                let summary = reads::report_summary(id);\n";
    let found = route_write_offences(text);
    for shape in ROUTE_WRITE_SHAPES.iter().copied() {
        assert!(
            found.iter().any(|offence| offence.contains(shape)),
            "{shape} is not caught: {found:?}"
        );
    }
    // One offence per shape: the line that owns neither a write nor a transaction
    // is the control, and is reported not at all.
    assert_eq!(
        found.len(),
        ROUTE_WRITE_SHAPES.len(),
        "every shape is reported once: {found:?}"
    );
    let quiet = line_holding(text, "report_summary");
    assert!(
        !found
            .iter()
            .any(|offence| offence.starts_with(&format!("{quiet}: "))),
        "a line owning no write is reported: {found:?}"
    );
}

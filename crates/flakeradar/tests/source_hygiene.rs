//! Two properties of this workspace's own source, checked from outside it.
//!
//! Both are rules a compiler cannot see: that every log event carries a name, and
//! that no comment or identifier in `crates/` points at the implementation this
//! one replaced. Each test reads the source files and names the line that breaks
//! the rule, so a regression is reported where it happened.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the tree it reads is not what it expects"
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

/// Every `.rs` file under `crates/`, with its path relative to the workspace root.
fn sources() -> Vec<(PathBuf, String)> {
    let mut found = Vec::new();
    let mut pending = vec![workspace_root().join("crates")];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("the source tree is readable") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|kind| kind == "rs") {
                let text = std::fs::read_to_string(&path).expect("a source file is readable");
                found.push((path, text));
            }
        }
    }
    found
}

/// The words that name the implementation this one replaced, or a plan step.
///
/// `task N` is here because a task number in a comment goes stale the day the plan
/// closes, and a reader of the code has no way to look it up.
const REPLACED: &[&str] = &[
    "python",
    "fastapi",
    "pydantic",
    "starlette",
    "asyncpg",
    "fastmcp",
    "backend/",
    "frozen",
];

/// The lines that may still carry one of [`REPLACED`]'s words, and why.
///
/// Each entry is a file name, the words it may carry, and the reason. They are
/// inputs rather than references to the old tree: a URL scheme this server accepts,
/// a field of a committed fixture, a path inside the sample repository, and the
/// `_meta` keys of the MCP wire.
const ALLOWED: &[(&str, &[&str], &str)] = &[
    (
        "crates/flakeradar-db/src/connect.rs",
        &["asyncpg"],
        "the postgresql+asyncpg:// scheme is one of the three URLs this server accepts",
    ),
    (
        "crates/flakeradar-db/src/error.rs",
        &["asyncpg"],
        "the refusal names the accepted URL scheme, which is the text a caller reads",
    ),
    (
        "crates/flakeradar-db/tests/connect.rs",
        &["asyncpg"],
        "the test pins the accepted URL schemes",
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
        &["python_value"],
        "the replay builds an allowance entry, whose field name is the fixture's",
    ),
    (
        "crates/flakeradar/tests/github/filing.rs",
        &["backend/"],
        "the path is inside the sample Repo's own tree, which the issue body reports",
    ),
    (
        "crates/flakeradar/tests/github/support.rs",
        &["backend/"],
        "the fixture body names a file in the sample Repo",
    ),
    (
        "crates/flakeradar/src/mcp/mod.rs",
        &["fastmcp"],
        "x-fastmcp-wrap-result and _meta.fastmcp.tags are keys of the MCP wire",
    ),
];

/// Whether `line` carries `word`, ignoring case except for the fixture's field.
///
/// The allowance fixture spells its field `pythonValue`, so the two spellings of
/// it are matched as written; every other word is compared in lowercase.
fn carries(line: &str, word: &str) -> bool {
    match word {
        "pythonValue" | "python_value" => line.contains(word),
        _ => {
            line.to_lowercase().contains(word)
                && !line.contains("python_value")
                && !line.contains("pythonValue")
        }
    }
}

/// No comment or identifier in `crates/` names the replaced implementation.
#[test]
fn no_source_line_points_at_the_replaced_implementation() {
    let mut offences = Vec::new();
    for (path, text) in sources() {
        if path.ends_with("tests/source_hygiene.rs") {
            continue;
        }
        let name = path
            .strip_prefix(workspace_root())
            .expect("the file sits under the workspace root")
            .to_string_lossy()
            .into_owned();
        let allowed: Vec<&str> = ALLOWED
            .iter()
            .filter(|(file, _, _)| *file == name)
            .flat_map(|(_, words, _)| words.iter().copied())
            .collect();
        for (number, line) in text.lines().enumerate() {
            for word in REPLACED {
                if carries(line, word) && !allowed.contains(word) {
                    offences.push(format!("{name}:{}: {word}: {line}", number + 1));
                }
            }
            if line.to_lowercase().contains("task ") && mentions_a_task_number(line) {
                offences.push(format!("{name}:{}: task number: {line}", number + 1));
            }
        }
    }
    assert!(
        offences.is_empty(),
        "these lines name the replaced implementation or a plan step:\n{}",
        offences.join("\n")
    );
}

/// Whether `line` names a task number, as `task 12` or `tasks 16–31`.
fn mentions_a_task_number(line: &str) -> bool {
    let lowered = line.to_lowercase();
    let Some(at) = lowered.find("task") else {
        return false;
    };
    lowered.get(at + 4..).is_some_and(|rest| {
        rest.trim_start()
            .starts_with(|ch: char| ch.is_ascii_digit())
    })
}

/// Every `tracing` event carries a `name:`, and none formats its message first.
#[test]
fn every_log_event_is_named_and_unformatted() {
    let mut offences = Vec::new();
    for (path, text) in sources() {
        for (offset, _) in text.match_indices("tracing::") {
            let rest = text.get(offset..).unwrap_or_default();
            let event = ["info!", "warn!", "error!", "debug!", "trace!"]
                .iter()
                .find(|macro_name| rest.starts_with(&format!("tracing::{macro_name}")));
            let Some(_) = event else {
                continue;
            };
            let Some(open) = rest.find('(') else {
                continue;
            };
            let call = rest.get(open..).unwrap_or_default();
            let end = call.find(");").unwrap_or(call.len());
            let call = call.get(..end).unwrap_or_default();
            let line = text.get(..offset).unwrap_or_default().lines().count() + 1;
            if !call.contains("name:") {
                offences.push(format!("{}:{line}: no name", path.display()));
            }
            if call.contains("format!(") {
                offences.push(format!(
                    "{}:{line}: a message built before it is logged",
                    path.display()
                ));
            }
        }
    }
    assert!(
        offences.is_empty(),
        "these events break the logging rule:\n{}",
        offences.join("\n")
    );
}

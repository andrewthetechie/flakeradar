//! Replays every recorded JUnit golden file through the Rust parser.
//!
//! The goldens are what the frozen Python server produced for each corpus file
//! (task 02), so this is the parity check for parsing, fingerprints and Failure
//! categories. A difference is accepted only through an entry in
//! `tests/parity/allowed-differences.json`, read here exactly as the parity
//! rules describe it, so an intended difference can never hide in this test.

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a test fails loudly when a golden file or the parser is wrong"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use flakeradar_core::{
    ExecutionStatus, ParseError, ParsedCase, classify_failure, fingerprint, parse_junit_xml,
};
use serde_json::Value;

/// The repository's `tests/parity` directory.
fn parity_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity")
}

/// One allowed difference, as `allowed-differences.json` spells it.
struct Allowance {
    /// Golden file the allowance covers, relative to `tests/parity`.
    file: String,
    /// Dotted path of the differing field within that golden.
    field: String,
    /// The exact Python value, when the allowance names one.
    python_value: Option<Value>,
    /// The exact Rust value, when the allowance names one.
    rust_value: Option<Value>,
    /// The prefix the Rust value must start with, when the allowance requires it.
    prefix: Option<String>,
}

/// Every allowance the repository records, loaded once.
static ALLOWANCES: LazyLock<Vec<Allowance>> = LazyLock::new(|| {
    let path = parity_dir().join("allowed-differences.json");
    let raw =
        fs::read(&path).unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let entries: Value = serde_json::from_slice(&raw)
        .unwrap_or_else(|error| panic!("{} is not JSON: {error}", path.display()));
    let mut allowances = Vec::new();
    for entry in entries.as_array().expect("the file is an array") {
        for allow in entry["allows"]
            .as_array()
            .expect("an entry lists allowances")
        {
            allowances.push(Allowance {
                file: allow["file"]
                    .as_str()
                    .expect("an allowance names a file")
                    .to_owned(),
                field: allow["field"]
                    .as_str()
                    .expect("an allowance names a field")
                    .to_owned(),
                python_value: allow.get("pythonValue").cloned(),
                rust_value: allow.get("rustValue").cloned(),
                prefix: allow
                    .get("match")
                    .filter(|matcher| matcher["kind"] == "prefix")
                    .and_then(|matcher| matcher["value"].as_str())
                    .map(ToOwned::to_owned),
            });
        }
    }
    allowances
});

/// Whether `allowed-differences.json` covers this exact value difference.
fn is_allowed(golden: &str, field: &str, python: &Value, rust: &Value) -> bool {
    let file = format!("golden/junit/{golden}");
    ALLOWANCES.iter().any(|allowance| {
        allowance.file == file
            && allowance.field == field
            && allowance
                .python_value
                .as_ref()
                .is_none_or(|value| value == python)
            && allowance
                .rust_value
                .as_ref()
                .is_none_or(|value| value == rust)
            && allowance
                .prefix
                .as_ref()
                .is_none_or(|prefix| rust.as_str().is_some_and(|text| text.starts_with(prefix)))
    })
}

/// Asserts one golden field, unless an allowance covers exactly this difference.
#[expect(
    clippy::needless_pass_by_value,
    reason = "the helper owns the produced value so a call site stays one expression"
)]
fn check_field(golden: &str, field: &str, want: &Value, got: Value) {
    if want == &got {
        return;
    }
    assert!(
        is_allowed(golden, field, want, &got),
        "{golden}: {field} is {got}, Python recorded {want}"
    );
}

/// The recorded result of one corpus file at one `line_base`.
struct Golden {
    /// File name, for failure messages and allowances.
    name: String,
    /// The corpus file the golden was recorded from.
    source: PathBuf,
    /// The `line_base` the record used.
    line_base: i64,
    /// The recorded body.
    body: Value,
}

/// Loads every `golden/junit/*.json` record.
fn goldens() -> Vec<Golden> {
    let dir = parity_dir().join("golden/junit");
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("cannot list {}: {error}", dir.display()))
        .map(|entry| entry.expect("the directory is readable").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no golden files under {}", dir.display());
    paths
        .into_iter()
        .map(|path| {
            let raw = fs::read(&path)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
            let body: Value = serde_json::from_slice(&raw)
                .unwrap_or_else(|error| panic!("{} is not JSON: {error}", path.display()));
            let name = path
                .file_name()
                .expect("a golden file has a name")
                .to_string_lossy()
                .into_owned();
            let source = body
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("{name} has no source"))
                .to_owned();
            let line_base = body
                .get("line_base")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| panic!("{name} has no line_base"));
            Golden {
                name,
                source: parity_dir().join(source),
                line_base,
                body,
            }
        })
        .collect()
}

#[test]
fn every_recorded_junit_golden_replays_identically() {
    let goldens = goldens();
    // Both line_base values for every corpus file, so a parser that ignored the
    // parameter would fail half of them.
    assert!(
        goldens.len() >= 60,
        "expected the whole corpus, got {}",
        goldens.len()
    );
    for golden in &goldens {
        let bytes = fs::read(&golden.source)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", golden.source.display()));
        let parsed = parse_junit_xml(&bytes, golden.line_base);
        match golden.body.get("error").and_then(Value::as_str) {
            Some(recorded) => check_error(golden, recorded, parsed),
            None => check_cases(golden, &parsed),
        }
    }
}

/// Checks one recorded parse error against what Rust reports.
fn check_error(golden: &Golden, recorded: &str, parsed: Result<Vec<ParsedCase>, ParseError>) {
    let error = parsed.err().unwrap_or_else(|| {
        panic!(
            "{}: Python recorded an error but Rust parsed the report",
            golden.name
        )
    });
    if recorded.starts_with("unhandled ") {
        // Python raised out of the parser here; the Rust parser must name the
        // bad value so the ingest route can answer 400 (differences.md).
        assert!(
            matches!(error, ParseError::InvalidTime { .. }),
            "{}: expected a typed time error, got {error}",
            golden.name
        );
    }
    check_field(
        &golden.name,
        "error",
        &Value::String(recorded.to_owned()),
        Value::String(error.to_string()),
    );
}

/// Checks every recorded case field against what Rust produced.
fn check_cases(golden: &Golden, parsed: &Result<Vec<ParsedCase>, ParseError>) {
    let cases = parsed.as_ref().unwrap_or_else(|error| {
        panic!(
            "{}: Python parsed the report but Rust failed: {error}",
            golden.name
        )
    });
    let recorded = golden
        .body
        .get("cases")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{} has no cases", golden.name));
    assert_eq!(
        cases.len(),
        recorded.len(),
        "{}: case count differs",
        golden.name
    );
    for (index, (case, want)) in cases.iter().zip(recorded).enumerate() {
        let name = &golden.name;
        let field = |name: &str| format!("cases.{index}.{name}");
        check_field(
            name,
            &field("suite"),
            &want["suite"],
            Value::from(case.suite.clone()),
        );
        check_field(
            name,
            &field("classname"),
            &want["classname"],
            Value::from(case.classname.clone()),
        );
        check_field(
            name,
            &field("name"),
            &want["name"],
            Value::from(case.name.clone()),
        );
        check_field(
            name,
            &field("status"),
            &want["status"],
            Value::from(case.status.as_str()),
        );
        check_field(
            name,
            &field("duration"),
            &want["duration"],
            Value::from(case.duration),
        );
        check_field(
            name,
            &field("message"),
            &want["message"],
            Value::from(case.message.clone()),
        );
        check_field(
            name,
            &field("details"),
            &want["details"],
            Value::from(case.details.clone()),
        );
        check_field(
            name,
            &field("file"),
            &want["file"],
            case.file.clone().map_or(Value::Null, Value::from),
        );
        check_field(
            name,
            &field("line"),
            &want["line"],
            case.line.map_or(Value::Null, Value::from),
        );
        check_field(
            name,
            &field("fingerprint"),
            &want["fingerprint"],
            Value::from(fingerprint(&case.suite, &case.classname, &case.name)),
        );
        let category = matches!(
            case.status,
            ExecutionStatus::Failed | ExecutionStatus::Error
        )
        .then(|| classify_failure(&case.message, &case.details));
        check_field(
            name,
            &field("category"),
            &want["category"],
            category.map_or(Value::Null, |category| Value::from(category.as_str())),
        );
    }
}

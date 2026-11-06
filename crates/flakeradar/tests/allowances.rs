//! The parity allowances, reconciled with the record of intended differences.
//!
//! `tests/parity/allowed-differences.json` lets a replay accept a mismatch, and
//! `docs/plans/rust-rewrite/differences.md` explains why each one is intended.
//! The deleted recorder checked the two against each other; these tests are that
//! check, so an allowance cannot say it accepts "whatever the server answers"
//! and cannot cite a difference the record does not hold.

#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "a test fails loudly when the fixtures it reconciles are wrong"
)]

mod common;

use std::path::Path;

use common::allowances::{Allowance, Allowances, differences, reconcile, root};

/// The committed allowances, reconciled with the committed record.
#[test]
fn every_allowance_names_its_values_and_cites_a_recorded_difference() {
    let allowances = Allowances::load();
    assert!(
        !allowances.entries().is_empty(),
        "the fixture records at least one intended difference"
    );
    let problems = reconcile(allowances.entries(), &differences(), &root());
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

/// The allowances of one JSON document, as the fixture spells them.
fn entries(document: &str) -> Vec<Allowance> {
    serde_json::from_str(document).expect("the control document is the fixture's shape")
}

/// A golden the controls name, which records a value a control can contradict.
const HEALTH: &str = "golden/health/01.json";

/// The first problem the reconciliation reports, or a panic when it reports none.
fn only_problem(document: &str, differences: &str) -> String {
    let problems = reconcile(&entries(document), differences, &root());
    assert_eq!(problems.len(), 1, "{problems:?}");
    problems[0].clone()
}

/// The reconciliation reports each way an allowance can lie.
#[test]
fn an_allowance_that_mismatches_the_record_is_reported() {
    let record = differences();
    let row = "An allowance a test invents";

    // A cited row the record does not hold: this is the check that fails when a
    // `differences.md` row is removed, which is what the recorder used to catch.
    let unrecorded = format!(
        r#"[{{"id": "invented", "differenceRow": "{row}", "reason": "control", "allows": [
            {{"file": "{HEALTH}", "field": "body.status", "pythonValue": "ok", "rustValue": "healthy"}}]}}]"#
    );
    assert!(
        only_problem(&unrecorded, &record).contains("is not a row of differences.md"),
        "a row no longer in the record is reported"
    );

    // A row that is in the record reconciles, so the control above fails for the
    // reason it names and not because the document is unacceptable.
    let recorded_row = record
        .lines()
        .find(|line| line.starts_with("| `line` attribute |"))
        .expect("the record holds that row");
    let mut cells = recorded_row.split('|').map(str::trim);
    let _ = cells.next();
    let area = cells.next().expect("the row names its area");
    let recorded = format!(
        r#"[{{"id": "recorded", "differenceRow": "{area}", "reason": "control", "allows": [
            {{"file": "{HEALTH}", "field": "body.status", "pythonValue": "ok", "rustValue": "healthy"}}]}}]"#
    );
    assert_eq!(
        reconcile(&entries(&recorded), &record, &root()),
        Vec::<String>::new()
    );

    // An allowance with no expected values accepts any mismatch at its field.
    let open_ended = format!(
        r#"[{{"id": "open", "differenceRow": "{area}", "reason": "control", "allows": [
            {{"file": "{HEALTH}", "field": "body.status"}}]}}]"#
    );
    assert!(
        only_problem(&open_ended, &record).contains("without the values it may show"),
        "an open-ended allowance is reported"
    );

    // A `pythonValue` the golden does not hold is a stale allowance.
    let stale = format!(
        r#"[{{"id": "stale", "differenceRow": "{area}", "reason": "control", "allows": [
            {{"file": "{HEALTH}", "field": "body.status", "pythonValue": "fine", "rustValue": "healthy"}}]}}]"#
    );
    assert!(
        only_problem(&stale, &record).contains("recorded"),
        "a pythonValue the golden does not hold is reported"
    );

    // A golden that does not exist, a field that does not exist, and an entry
    // whose contract also names values are each reported by name.
    let missing_file = format!(
        r#"[{{"id": "absent", "differenceRow": "{area}", "reason": "control", "allows": [
            {{"file": "golden/nowhere/01.json", "field": "body.status", "pythonValue": "ok", "rustValue": "healthy"}}]}}]"#
    );
    assert!(
        only_problem(&missing_file, &record).contains("not a readable golden"),
        "a golden that is not there is reported"
    );
    let missing_field = format!(
        r#"[{{"id": "absent", "differenceRow": "{area}", "reason": "control", "allows": [
            {{"file": "{HEALTH}", "field": "body.health", "pythonValue": "ok", "rustValue": "healthy"}}]}}]"#
    );
    assert!(
        only_problem(&missing_field, &record).contains("recorded null"),
        "a field the golden does not hold is reported"
    );
    let contract_and_values = format!(
        r#"[{{"id": "both", "differenceRow": "{area}", "reason": "control", "allows": [
            {{"file": "{HEALTH}", "field": "body", "pythonValue": {{}}, "rustValue": {{}},
              "contract": {{"kind": "openapi"}}}}]}}]"#
    );
    assert!(
        only_problem(&contract_and_values, &record).contains("must not name values"),
        "a contract compared by value is reported"
    );

    // An entry that gives no reason, and one whose id another entry already uses,
    // are reported too: a reader of the fixture needs both.
    let no_reason = format!(
        r#"[{{"id": "bare", "differenceRow": "{area}", "allows": [
            {{"file": "{HEALTH}", "field": "body.status", "pythonValue": "ok", "rustValue": "healthy"}}]}}]"#
    );
    assert!(
        only_problem(&no_reason, &record).contains("gives no reason"),
        "an unexplained allowance is reported"
    );
    let twice = format!(
        r#"[{{"id": "twice", "differenceRow": "{area}", "reason": "control", "allows": [
            {{"file": "{HEALTH}", "field": "body.status", "pythonValue": "ok", "rustValue": "healthy"}}]}},
           {{"id": "twice", "differenceRow": "{area}", "reason": "control", "allows": [
            {{"file": "{HEALTH}", "field": "body.status", "pythonValue": "ok", "rustValue": "healthy"}}]}}]"#
    );
    assert!(
        only_problem(&twice, &record).contains("used twice"),
        "a repeated entry id is reported"
    );
}

/// The record the reconciliation reads is the plan's own file.
#[test]
fn the_record_is_the_plans_differences_file() {
    let record = differences();
    assert!(
        record.contains("# Intentional behaviour differences"),
        "the reconciliation reads differences.md"
    );
    assert!(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/plans/rust-rewrite/differences.md")
            .is_file(),
        "the record is a committed file"
    );
}

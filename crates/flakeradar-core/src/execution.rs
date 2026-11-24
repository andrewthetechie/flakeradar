//! The rules that turn a Report's cases into numbered Executions.
//!
//! [`build_executions`] answers what a Report implies: one [`PlannedExecution`]
//! per case with its per-Test attempt, each Test's newest state as a
//! [`PlannedLatestTest`], the six [`PlanCounts`], and what the day added to each
//! Test's Score history. Nothing here stores a row or reads a setting: the
//! database layer maps the plan onto its own active models and owns every
//! stored timestamp, so the rules stay testable with plain values. The only
//! instant the plan answers is the one the caller supplied, echoed so every row
//! the mapping builds carries it.

// Rust guideline compliant 2026-07-21

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

use chrono::{DateTime, FixedOffset};

use crate::model::ExecutionStatus;
use crate::parsing::{ClassifiedCase, ClassifiedReport};
use crate::types::Fingerprint;

/// One Execution a case becomes, before any row is built.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedExecution {
    /// The Test this attempt belongs to, as the caller's id map answers it.
    pub test_case_id: i32,
    /// Which try of the Test this is, counting from 0 within the Report.
    pub attempt: i32,
    /// What this attempt did.
    pub status: ExecutionStatus,
    /// The Failure message, `""` unless the attempt failed.
    pub message: String,
    /// The Failure details, `""` unless the attempt failed or errored.
    pub details: String,
    /// The reported duration in seconds; 0.0 when the runner gave none.
    pub duration: f64,
    /// The Location the rule accepted: the reported file, with the line only
    /// when the Report gave a file and the line fitted the column.
    pub location: Option<(String, i32)>,
}

/// A Test's newest occurrence in the Report: the state and Location to keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedLatestTest {
    /// The Test this row describes.
    pub test_case_id: i32,
    /// What the Test did on its last attempt.
    pub last_status: ExecutionStatus,
    /// The Location the last occurrence with a file reported.
    pub file: Option<String>,
    /// The line of that Location, when it fitted the column.
    pub line: Option<i32>,
}

/// The counts a processed Report answers, one per status plus the two the
/// Report as a whole carries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlanCounts {
    /// Attempts that passed.
    pub passed: i64,
    /// Attempts that failed an assertion.
    pub failed: i64,
    /// Attempts that raised an error.
    pub error: i64,
    /// Attempts that were skipped.
    pub skipped: i64,
    /// Tests the Report attempted more than once.
    pub retried: i64,
    /// Cases the deduplicating path dropped; a JUnit Report has none.
    pub duplicate: i64,
}

impl PlanCounts {
    /// Counts one Execution's status.
    fn add(&mut self, status: ExecutionStatus) {
        match status {
            ExecutionStatus::Passed => self.passed += 1,
            ExecutionStatus::Failed => self.failed += 1,
            ExecutionStatus::Error => self.error += 1,
            ExecutionStatus::Skipped => self.skipped += 1,
        }
    }
}

/// What a Report's cases imply, in Report order.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionPlan {
    /// The Run these Executions belong to, answered back so the caller writes
    /// the same value into every row.
    pub run_id: i32,
    /// The instant the caller built the plan for, answered back so the mapping
    /// stamps every row it writes with the one instant the Run already uses.
    pub now: DateTime<FixedOffset>,
    /// One row per case, in Report order.
    pub executions: Vec<PlannedExecution>,
    /// Each Test's newest occurrence, in order of first appearance.
    pub latest: Vec<PlannedLatestTest>,
    /// What the Report's attempts add up to.
    pub counts: PlanCounts,
    /// What this Report added to each Test's day, as `(executions, failures)`.
    pub day_counts: BTreeMap<i32, (i32, i32)>,
}

/// Why a Report cannot become an Execution plan.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecutionError {
    /// The caller's id map has no id for a Test the Report names.
    #[error("DataError: Test {fingerprint} vanished")]
    Vanished {
        /// The Test that has no id.
        fingerprint: Fingerprint,
    },
    /// A reported line does not fit the 32-bit column the Location uses.
    #[error("DataError: line {line} is out of range")]
    LineOutOfRange {
        /// The line the Report reported.
        line: i64,
    },
}

/// Plans the Executions, latest states and counts a Report's cases imply.
///
/// The attempt number counts per Test from 0, so a Report that retries a Test
/// numbers its attempts in the order the cases appear. The last occurrence of a
/// Test wins its `last_status`, and only an occurrence that gave a file replaces
/// the stored Location, so a runner that omits the Location on a retry never
/// erases one. A Location is planned only when the Report gave a file, so a line
/// without one is never planned and never has to fit the column.
///
/// The cases are borrowed: the caller keeps them, along with the Failure
/// category of each, which no column of the plan carries.
///
/// `now` is the instant the caller stamps the Run's rows with. No rule reads it:
/// the plan answers it back unchanged, so the mapping that builds the rows
/// writes one instant into all of them.
///
/// # Errors
///
/// Returns [`ExecutionError::Vanished`] when `ids` holds no id for a Test the
/// Report names, and [`ExecutionError::LineOutOfRange`] when a case that gave a
/// file reported a line too large for an `i32`.
///
/// # Examples
///
/// ```
/// use std::collections::BTreeMap;
///
/// use flakeradar_core::execution::build_executions;
/// use flakeradar_core::model::ExecutionStatus;
/// use flakeradar_core::parsing::classify_report;
///
/// let xml = br#"<testsuite name="unit"><testcase classname="c" name="t"><failure message="boom">trace</failure></testcase><testcase classname="c" name="t"/></testsuite>"#;
/// let report = classify_report(xml, 1).unwrap();
/// let ids: BTreeMap<_, _> = report
///     .first
///     .iter()
///     .enumerate()
///     .map(|(seen, index)| (report.cases[*index].fingerprint.clone(), (seen + 1) as i32))
///     .collect();
/// let now = chrono::DateTime::parse_from_rfc3339("2026-07-21T12:00:00+00:00").expect("an instant");
/// let plan = build_executions(&report, &ids, 7, now).unwrap();
/// let attempts: Vec<i32> = plan.executions.iter().map(|row| row.attempt).collect();
/// assert_eq!(attempts, [0, 1]);
/// assert_eq!(plan.counts.retried, 1);
/// assert_eq!(plan.latest[0].last_status, ExecutionStatus::Passed);
/// assert_eq!(plan.now, now);
/// ```
pub fn build_executions(
    report: &ClassifiedReport,
    ids: &BTreeMap<Fingerprint, i32>,
    run_id: i32,
    now: DateTime<FixedOffset>,
) -> Result<ExecutionPlan, ExecutionError> {
    let mut counts = PlanCounts::default();
    let mut attempts: BTreeMap<i32, i32> = BTreeMap::new();
    let mut executions: Vec<PlannedExecution> = Vec::with_capacity(report.cases.len());
    let mut latest: BTreeMap<i32, PlannedLatestTest> = BTreeMap::new();
    let mut order: Vec<i32> = Vec::new();
    let mut day_counts: BTreeMap<i32, (i32, i32)> = BTreeMap::new();
    for ClassifiedCase {
        fingerprint, case, ..
    } in &report.cases
    {
        let test_case_id =
            ids.get(fingerprint)
                .copied()
                .ok_or_else(|| ExecutionError::Vanished {
                    fingerprint: fingerprint.clone(),
                })?;
        let attempt = attempts.get(&test_case_id).copied().unwrap_or(0);
        attempts.insert(test_case_id, attempt + 1);
        counts.add(case.status);
        let day = day_counts.entry(test_case_id).or_default();
        day.0 += 1;
        if case.status.is_failing() {
            day.1 += 1;
        }
        let line = if case.file.is_some() {
            case.line
                .map(|line| {
                    i32::try_from(line)
                        .map_err(|_out_of_range| ExecutionError::LineOutOfRange { line })
                })
                .transpose()?
        } else {
            None
        };
        executions.push(PlannedExecution {
            test_case_id,
            attempt,
            status: case.status,
            message: case.message.clone(),
            details: case.details.clone(),
            duration: case.duration,
            location: case.file.clone().zip(line),
        });
        match latest.entry(test_case_id) {
            Entry::Occupied(row) => {
                let row = row.into_mut();
                row.last_status = case.status;
                // A report without a Location never erases one.
                if case.file.is_some() {
                    row.file.clone_from(&case.file);
                    row.line = line;
                }
            }
            Entry::Vacant(slot) => {
                order.push(test_case_id);
                slot.insert(PlannedLatestTest {
                    test_case_id,
                    last_status: case.status,
                    file: case.file.clone(),
                    line,
                });
            }
        }
    }
    counts.retried =
        i64::try_from(attempts.values().filter(|seen| **seen > 1).count()).unwrap_or(i64::MAX);
    let latest = order
        .into_iter()
        .filter_map(|id| latest.remove(&id))
        .collect();
    Ok(ExecutionPlan {
        run_id,
        now,
        executions,
        latest,
        counts,
        day_counts,
    })
}

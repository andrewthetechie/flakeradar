//! The clock the processor and the Score history read.
//!
//! Every rule that depends on "today" takes its time from here, so a test can
//! put the processor on a fixed day without touching the wall clock. Nothing in
//! this crate reads the wall clock directly: every rule takes its instant from a
//! [`Clock`], so a test states the day instead of waiting for one.

// Rust guideline compliant 2026-07-21

use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use chrono::{NaiveDate, Utc};
use sea_orm::entity::prelude::DateTimeWithTimeZone;

/// Where the processor learns the current instant.
pub trait Clock: fmt::Debug + Send + Sync {
    /// The current instant, in UTC.
    fn now(&self) -> DateTimeWithTimeZone;

    /// The current UTC day, which is what Score history is keyed on.
    fn today(&self) -> NaiveDate {
        self.now().date_naive()
    }
}

/// The real clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTimeWithTimeZone {
        Utc::now().fixed_offset()
    }
}

/// The clock a [`ProcessingSettings`](crate::processing::ProcessingSettings) carries.
pub type SharedClock = Arc<dyn Clock>;

/// Where a background run learns how much time it has spent.
///
/// Retention takes each reading from this seam, so a test can stop the clock and
/// decide when a run has spent its budget. Only differences between two readings
/// are meaningful, and the origin is an arbitrary instant, so nothing here
/// reports a wall-clock time.
pub trait MonotonicClock: fmt::Debug + Send + Sync {
    /// Seconds from this clock's origin.
    fn now(&self) -> f64;
}

/// The real monotonic clock, measured from the moment it was built.
#[derive(Debug, Clone, Copy)]
pub struct SystemMonotonic {
    start: Instant,
}

impl SystemMonotonic {
    /// A clock whose origin is now.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for SystemMonotonic {
    fn default() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl MonotonicClock for SystemMonotonic {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }
}

/// The monotonic clock a retention run or the Report worker carries.
pub type SharedMonotonic = Arc<dyn MonotonicClock>;

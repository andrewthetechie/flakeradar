//! The clock the processor and the Score history read.
//!
//! Every rule that depends on "today" takes its time from here, so a test can
//! put the processor on a fixed day without touching the wall clock. The frozen
//! Python server read `utcnow()` directly; the rules are the same, only the
//! source of the instant moved.

// Rust guideline compliant 2026-07-21

use std::sync::Arc;

use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use sea_orm::entity::prelude::DateTimeWithTimeZone;

/// Where the processor learns the current instant.
pub trait Clock: Send + Sync {
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

/// A clock stopped at one instant, for tests that must control the day.
#[derive(Debug, Clone)]
pub struct FixedClock {
    now: DateTimeWithTimeZone,
}

impl FixedClock {
    /// A clock reading `now`.
    #[must_use]
    pub fn new(now: DateTime<FixedOffset>) -> Self {
        Self { now }
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTimeWithTimeZone {
        self.now
    }
}

/// The clock a [`ProcessingSettings`](crate::ProcessingSettings) carries.
pub type SharedClock = Arc<dyn Clock>;

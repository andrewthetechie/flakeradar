//! The clocks a test hands a seam that asks for one.

// Rust guideline compliant 2026-07-21

use std::sync::Arc;

use chrono::{DateTime, FixedOffset};
use flakeradar_db::{clock::Clock, clock::SharedClock, clock::SystemClock};

/// The system clock, boxed the way a seam takes one.
///
/// A test that does not care which instant the code under it read passes this,
/// so the seam is filled and the assertion is about something else.
#[must_use]
pub fn system_clock() -> SharedClock {
    Arc::new(SystemClock)
}

/// A clock stopped at one instant, for a test that asserts what was written.
#[derive(Debug, Clone)]
pub struct FixedClock {
    now: DateTime<FixedOffset>,
}

impl FixedClock {
    /// A clock reading `now`.
    #[must_use]
    pub fn new(now: DateTime<FixedOffset>) -> Self {
        Self { now }
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTime<FixedOffset> {
        self.now
    }
}

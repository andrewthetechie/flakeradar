//! A monotonic clock a test scripts, so a time-budgeted run never waits.

// Rust guideline compliant 2026-07-21

use std::collections::VecDeque;
use std::sync::Mutex;

use flakeradar_db::clock::MonotonicClock;

/// A monotonic clock that answers each reading from a list the test wrote.
///
/// The last reading it scripts is past the budget, so the run stops without
/// spending one. Once the list is empty, the last value repeats, because a run may
/// read the clock more often than the test predicted.
#[derive(Debug)]
pub struct ScriptedMonotonic {
    readings: Mutex<(VecDeque<f64>, f64)>,
}

impl ScriptedMonotonic {
    /// A clock that answers the `readings` in order, from `readings` seconds.
    #[must_use]
    pub fn new(readings: impl IntoIterator<Item = f64>) -> Self {
        Self {
            readings: Mutex::new((readings.into_iter().collect(), 0.0)),
        }
    }
}

impl MonotonicClock for ScriptedMonotonic {
    fn now(&self) -> f64 {
        let mut guard = self
            .readings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match guard.0.pop_front() {
            Some(reading) => {
                guard.1 = reading;
                reading
            }
            None => guard.1,
        }
    }
}

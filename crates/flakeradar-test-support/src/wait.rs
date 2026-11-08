//! The queue facts a test asserts, and the one bounded wait the suite allows.
//!
//! A worker's own queue reports what it did with the work it was given, so a
//! test reads the queue rather than sleeping and hoping the task ran. The wait
//! below is the exception the standard records: a real socket answer and a real
//! background task have no seam to signal on, so a test polls for one and fails
//! on a deadline. The deadline bounds a failure; it is never the thing asserted.
//! [`within`] is the second exception, and the standard gives the same reason: a
//! cancellation has no observable other than the time it did not take.

// Rust guideline compliant 2026-07-21

use std::time::{Duration, Instant};

use flakeradar::queue::BatchQueue;

/// The longest a [`wait_until`] call may take before the test fails.
///
/// Far longer than any of these conditions takes on a loaded machine, so a
/// passing run never reaches it and a broken one says what it waited for.
pub const WAIT_DEADLINE: Duration = Duration::from_secs(30);

/// How long one poll waits before looking again.
pub const WAIT_INTERVAL: Duration = Duration::from_millis(2);

/// How many batches are waiting in `queue`.
#[must_use]
pub fn queued<T>(queue: &BatchQueue<T>) -> usize {
    queue.len()
}

/// How many batches the queue's bound has cost.
#[must_use]
pub fn dropped<T>(queue: &BatchQueue<T>) -> usize {
    queue.dropped()
}

/// Waits until `queue` has finished `expected` batches.
///
/// This is the proof a negative assertion needs: after it returns, the task has
/// taken that many batches and completed the work of each, so "nothing was sent"
/// is a claim about a task that ran rather than about a test that slept.
pub async fn until_finished<T>(queue: &BatchQueue<T>, expected: usize) {
    wait_until("the task finishes the batches it took", || async {
        queue.finished() >= expected
    })
    .await;
}

/// Runs `future`, failing when it takes longer than `bound`.
///
/// The one place the suite reads the real clock, and the standard names why: a
/// cancellation has no other observable. A stop either ends a delivery or a wait
/// at once, or sits through the seconds that delivery or wait asked for, so a
/// coarse bound separates the two. It is far above the cost of a wake and far
/// below the wait that must not happen, so a loaded machine cannot move it.
///
/// # Panics
///
/// Panics when `future` takes longer than `bound`, naming `what`.
pub async fn within<Fut, T>(what: &str, bound: Duration, future: Fut) -> T
where
    Fut: Future<Output = T>,
{
    let started = Instant::now();
    let output = future.await;
    let waited = started.elapsed();
    assert!(waited < bound, "{what}, but it waited {waited:?}");
    output
}

/// Waits for `condition` to hold, failing rather than hanging.
///
/// The deadline is a bound on a failure, not an assertion about time: every
/// condition the suite waits on this way is a row, a counter or a recorded
/// request that the code under test writes. Where the code has a seam that
/// signals the same fact, the seam is used instead (`until_finished`, a
/// `Notify`, a paused clock).
///
/// # Panics
///
/// Panics when `condition` has not held after [`WAIT_DEADLINE`], naming `what`.
pub async fn wait_until<F, Fut>(what: &str, mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + WAIT_DEADLINE;
    while !condition().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(WAIT_INTERVAL).await;
    }
}

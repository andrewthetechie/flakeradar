//! The bounded queue a leader task drains.
//!
//! Both leader tasks take their work from a queue of their own: the GitHub task
//! from the outcomes the processor hands it, the Notification task from the
//! Tier changes of those outcomes. Neither may grow without limit during an
//! outage, so both drop the oldest batch when full, and both report what they
//! did with the work: how deep the queue is, how many batches the bound cost,
//! and how many have been handled.

// Rust guideline compliant 2026-07-21

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

/// A mutex whose poisoning is recovered from, because a task that panicked
/// elsewhere must not stop the leader from working.
fn guard<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The queue one leader task drains, holding at most `capacity` batches.
///
/// A full queue drops the oldest batch rather than refusing the newest: during
/// an outage the newest news is the news worth filing, and the count of what was
/// lost is what an operator reads.
#[derive(Debug)]
pub struct BatchQueue<T> {
    /// The batches waiting, oldest first.
    batches: Mutex<VecDeque<T>>,
    /// Wakes the consuming loop when a batch arrives.
    notify: tokio::sync::Notify,
    /// How many batches the bound has cost.
    dropped: AtomicUsize,
    /// How many taken batches the task has finished handling.
    finished: AtomicUsize,
    /// How many batches the queue holds before it drops the oldest.
    capacity: usize,
}

impl<T> BatchQueue<T> {
    /// An empty queue that holds `capacity` batches.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            batches: Mutex::new(VecDeque::new()),
            notify: tokio::sync::Notify::new(),
            dropped: AtomicUsize::new(0),
            finished: AtomicUsize::new(0),
            capacity,
        }
    }

    /// Queues `batch`, answering the oldest one when the queue was full.
    ///
    /// The caller logs what it dropped, because only it knows what its batches
    /// are; the queue keeps the count.
    pub fn push(&self, batch: T) -> Option<T> {
        let dropped = {
            let mut batches = guard(&self.batches);
            let dropped = if batches.len() >= self.capacity {
                let dropped = batches.pop_front();
                self.dropped.fetch_add(1, Ordering::SeqCst);
                dropped
            } else {
                None
            };
            batches.push_back(batch);
            dropped
        };
        self.notify.notify_one();
        dropped
    }

    /// The next batch, when one is waiting.
    pub fn pop(&self) -> Option<T> {
        guard(&self.batches).pop_front()
    }

    /// How many batches are waiting.
    #[must_use]
    pub fn len(&self) -> usize {
        guard(&self.batches).len()
    }

    /// Whether no batch is waiting.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many batches the bound has cost.
    #[must_use]
    pub fn dropped(&self) -> usize {
        self.dropped.load(Ordering::SeqCst)
    }

    /// The handle the consuming loop selects on when the queue is empty.
    ///
    /// It is taken before the queue is read, so a batch pushed between the two
    /// cannot be missed.
    pub(crate) fn notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.notify.notified()
    }

    /// Records that one taken batch has been handled, answering how many have.
    pub fn finish(&self) -> usize {
        self.finished.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// How many taken batches the task has finished handling.
    ///
    /// A batch counts once its filing, delivery or sync has returned, so this is
    /// the number a reader waits on to know the task has dealt with what it took.
    #[must_use]
    pub fn finished(&self) -> usize {
        self.finished.load(Ordering::SeqCst)
    }
}

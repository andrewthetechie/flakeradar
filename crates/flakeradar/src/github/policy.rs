//! What GitHub answered, and what each operation does about it.
//!
//! One classifier turns an answer into a [`Verdict`], and one table — [`action`] —
//! states what each [`Operation`] does with each verdict. A batch reports what it
//! did as a [`BatchReport`], or as a [`RemovalReport`] for the post-delete close,
//! so a caller asserts the outcome instead of a log line.

use std::collections::{BTreeMap, BTreeSet};

use super::client::{Answer, Success, TransportError};

/// What GitHub said, as one verdict every operation reads the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The 2xx the call asked for.
    Accepted,
    /// GitHub is throttling: a `429`, or a `403` that rides a rate limit.
    RateLimited,
    /// `401`: bad credentials.
    Unauthorized,
    /// A `403` that is not a rate limit: permission, or issues disabled.
    RepoDenied,
    /// `404`.
    NotFound,
    /// `410`.
    Gone,
    /// Any other answer.
    Other,
    /// No answer at all.
    ///
    /// [`Answer::classify`](super::client::Answer::classify) never returns this: a
    /// caller turns a [`TransportError`] into it, so a
    /// batch's policy covers it too.
    Unreachable,
}

/// One of the four GitHub operations a verdict is judged for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// Creating an issue for a Test or a Job.
    File,
    /// Reading one linked issue's state in the sync.
    Sync,
    /// Commenting that a fix appears to hold.
    Note,
    /// Closing the issue that comment landed on.
    CloseNote,
    /// Commenting on and closing the issues an admin delete stranded.
    CloseDeleted,
}

/// What one operation does when GitHub answers with one verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Act on the answer: file the issue, or read the issue's state.
    Use,
    /// The issue itself is gone: clear the link and re-arm filing.
    Rearm,
    /// Cost this row only, and go on with the next.
    SkipRow,
    /// Skip this row's Repo for the rest of the batch.
    SkipRepo,
    /// End the batch.
    Stop,
}

/// Why a batch ended before its last row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchStop {
    /// GitHub is throttling, which ends every batch.
    RateLimited,
    /// Nothing answered, so the rest of the batch is not worth trying.
    Unreachable,
    /// The leader lost leadership, so the batch gave up.
    Stopped,
    /// A filed issue's number could not be stored, so the batch ended with it open.
    StoreFailed,
}

/// What one batch did, as a value a caller can assert on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchReport {
    /// The entity ids the batch stored a link for.
    pub linked: BTreeSet<i32>,
    /// The Repos the batch skipped for the rest of its run.
    pub skipped: BTreeSet<String>,
    /// Why the batch ended before its last row, when it did.
    pub stopped: Option<BatchStop>,
}

impl BatchReport {
    /// Logs this batch's summary once, at the edge that owns it.
    pub(super) fn log(&self, operation: Operation) {
        tracing::info!(
            name: "github.batch.finished",
            operation = ?operation,
            linked = self.linked.len(),
            skipped_repos = self.skipped.len(),
            stopped = ?self.stopped,
            "GitHub batch finished"
        );
    }
}

/// What the post-delete close did, as a value a caller can assert on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemovalReport {
    /// The issue numbers whose comment landed and which were then closed.
    pub closed: BTreeSet<i32>,
    /// The issue numbers nothing was closed for, with GitHub's answer.
    ///
    /// A [`Verdict::NotFound`] or [`Verdict::Gone`] here is the issue already
    /// being gone, which is the outcome this batch wanted; anything else left the
    /// issue open.
    pub refused: BTreeMap<i32, Verdict>,
    /// Why the batch ended before its last number, when it did.
    pub stopped: Option<BatchStop>,
}

impl RemovalReport {
    /// Logs this batch's summary once, at the edge that owns it.
    pub(super) fn log(&self) {
        tracing::info!(
            name: "github.batch.finished",
            operation = ?Operation::CloseDeleted,
            closed = self.closed.len(),
            refused = self.refused.len(),
            stopped = ?self.stopped,
            "GitHub batch finished"
        );
    }
}

/// What every operation does with every verdict: the module's one status policy.
///
/// A row is an operation and a column a verdict, and every cell is what the
/// reference does with that pair:
///
/// | verdict | `File` | `Sync` | `Note` | `CloseNote` | `CloseDeleted` |
/// |---|---|---|---|---|---|
/// | `Accepted` | use | use | use | use | use |
/// | `RateLimited` | stop | stop | stop | stop | stop |
/// | `Unauthorized` | skip Repo | skip Repo | skip row | skip row | skip row |
/// | `RepoDenied` | skip Repo | skip Repo | skip Repo | skip Repo | skip row |
/// | `NotFound` | skip Repo | re-arm | skip Repo | skip Repo | skip row |
/// | `Gone` | skip Repo | skip Repo | skip Repo | skip Repo | skip row |
/// | `Other` | skip row | skip row | skip row | skip row | skip row |
/// | `Unreachable` | stop | stop | stop | stop | skip row |
///
/// Three cells break the pattern, each for the operation's own reason:
/// a `404` in the sync is the issue itself being gone, so the link re-arms rather
/// than the Repo being skipped; a `401` is a per-item failure on the note paths,
/// because bad credentials are not Repo-wide there; and the post-delete close has
/// no Repo-skip cell at all, because it works through one Repo's numbers and
/// skipping the Repo would end the batch.
#[must_use]
pub fn action(operation: Operation, verdict: Verdict) -> Action {
    use Action::{Rearm, SkipRepo, SkipRow, Stop, Use};
    use Operation::{CloseDeleted, CloseNote, File, Note, Sync};
    use Verdict::{
        Accepted, Gone, NotFound, Other, RateLimited, RepoDenied, Unauthorized, Unreachable,
    };
    match operation {
        File => match verdict {
            Accepted => Use,
            RateLimited | Unreachable => Stop,
            Unauthorized | RepoDenied | NotFound | Gone => SkipRepo,
            Other => SkipRow,
        },
        Sync => match verdict {
            Accepted => Use,
            NotFound => Rearm,
            RateLimited | Unreachable => Stop,
            Unauthorized | RepoDenied | Gone => SkipRepo,
            Other => SkipRow,
        },
        Note | CloseNote => match verdict {
            Accepted => Use,
            RateLimited | Unreachable => Stop,
            RepoDenied | NotFound | Gone => SkipRepo,
            Unauthorized | Other => SkipRow,
        },
        CloseDeleted => match verdict {
            Accepted => Use,
            RateLimited => Stop,
            Unauthorized | RepoDenied | NotFound | Gone | Other | Unreachable => SkipRow,
        },
    }
}

/// Why GitHub's verdict ends a batch.
pub(super) fn batch_stop(verdict: Verdict) -> BatchStop {
    if verdict == Verdict::Unreachable {
        BatchStop::Unreachable
    } else {
        BatchStop::RateLimited
    }
}

/// One GitHub call, sent and judged.
#[derive(Debug)]
pub(super) enum Call {
    /// GitHub answered, and the table says what this operation does with it.
    Answered {
        /// What GitHub sent back.
        answer: Answer,
        /// The verdict the answer got.
        verdict: Verdict,
        /// What the operation does with the answer.
        action: Action,
    },
    /// Nothing answered. The same table judged that, so an outage costs what the
    /// operation's `Unreachable` cell says rather than what a call site assumed.
    Unreachable {
        /// What the operation does with no answer at all.
        action: Action,
    },
}

impl Call {
    /// The verdict this call got: the one GitHub's answer carried, or the one an
    /// outage always carries.
    #[must_use]
    pub(super) fn verdict(&self) -> Verdict {
        match self {
            Self::Answered { verdict, .. } => *verdict,
            Self::Unreachable { .. } => Verdict::Unreachable,
        }
    }

    /// Why this call ends its batch, when the table says it does.
    #[must_use]
    pub(super) fn stop(&self) -> BatchStop {
        batch_stop(self.verdict())
    }
}

/// Runs one GitHub call to the end of its policy: send, classify, judge.
///
/// A transport failure is [`Verdict::Unreachable`] here, so the table's own
/// `Unreachable` column decides what an outage costs the batch. The line it is
/// logged as stays the call site's words, because a tracing event name must be a
/// literal — `log` runs only on that path, with the failure to name.
pub(super) async fn step(
    operation: Operation,
    success: Success,
    call: impl Future<Output = Result<Answer, TransportError>>,
    log: impl FnOnce(&TransportError),
) -> Call {
    match call.await {
        Ok(answer) => {
            let verdict = answer.classify(success);
            Call::Answered {
                action: action(operation, verdict),
                answer,
                verdict,
            }
        }
        Err(error) => {
            log(&error);
            Call::Unreachable {
                action: action(operation, Verdict::Unreachable),
            }
        }
    }
}

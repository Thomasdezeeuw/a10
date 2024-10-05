//! Submission Queue.

use std::fmt;
use std::sync::Arc;

use crate::SharedState;

/// Queue of completion events.
pub(crate) struct Queue<S: Submissions, CE> {
    shared: Arc<SharedState<S, CE>>,
}

impl<S: Submissions, CE> Queue<S, CE> {
    pub(crate) const fn new(shared: Arc<SharedState<S, CE>>) -> Queue<S, CE> {
        Queue { shared }
    }

    pub(crate) fn add<F>(&self, submit: F) -> Result<(), QueueFull>
    where
        F: FnOnce(&mut S::Submission),
    {
        _ = submit;
        todo!();
    }
}

impl<S: Submissions, CE> Clone for Queue<S, CE> {
    fn clone(&self) -> Self {
        Queue {
            shared: self.shared.clone(),
        }
    }

    fn clone_from(&mut self, source: &Self) {
        self.shared.clone_from(&source.shared)
    }
}

impl<S: Submissions + fmt::Debug, CE: fmt::Debug> fmt::Debug for Queue<S, CE> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO.
        f.write_str("sq::Queue")
    }
}

/// Submit operations.
pub(crate) trait Submissions: fmt::Debug {
    /// Type of the submission.
    type Submission: Submission;

    /// Try to add a new submission.
    fn add<F>(&self, submit: F) -> Result<(), QueueFull>
    where
        F: FnOnce(&mut Self::Submission);
}

/// Submission queue is full.
pub(crate) struct QueueFull;

/// Submission event.
pub(crate) trait Submission: fmt::Debug {
    /// Set the identifier (index) of the completion events related to this
    /// submission.
    fn set_id(&mut self, id: usize);
}

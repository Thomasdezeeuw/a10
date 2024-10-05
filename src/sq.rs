//! Submission Queue.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::{fmt, io, mem};

use crate::{QueuedOperation, SharedState};

/// Queue of completion events.
pub(crate) struct Queue<S: Submissions, CE> {
    shared: Arc<SharedState<S, CE>>,
}

impl<S: Submissions, CE: Default> Queue<S, CE> {
    pub(crate) const fn new(shared: Arc<SharedState<S, CE>>) -> Queue<S, CE> {
        Queue { shared }
    }

    /// Add a new submission, returns an id.
    pub(crate) fn add<F>(&self, submit: F) -> Result<usize, QueueFull>
    where
        F: FnOnce(&mut S::Submission),
    {
        // Get an id (index) to the queued operation list.
        let shared = &*self.shared;
        let Some(id) = shared.op_indices.next_available() else {
            return Err(QueueFull);
        };

        let queued_op = QueuedOperation::new();
        // SAFETY: the `AtomicBitMap` always returns valid indices for
        // `op_queue` (it's the whole point of it).
        let mut op = shared.queued_ops[id].lock().unwrap();
        let old_queued_op = mem::replace(&mut *op, Some(queued_op));
        debug_assert!(old_queued_op.is_none());

        // TODO: not great naming `data.add`. Maybe rename the method?
        shared.data.add(|submission| {
            submit(submission);
            submission.set_id(id);
        })?;

        Ok(id)
    }

    pub(crate) fn wake(&self) {
        if !self.shared.is_polling.load(Ordering::Acquire) {
            // Not polling, no need to wake up.
            return;
        }

        if let Err(err) = self.shared.data.wake() {
            log::error!("failed to wake a10::Ring: {err}");
        }
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

    /// Wake a polling thread.
    fn wake(&self) -> io::Result<()>;
}

/// Submission queue is full.
pub(crate) struct QueueFull;

/// Submission event.
pub(crate) trait Submission: fmt::Debug {
    /// Set the identifier (index) of the completion events related to this
    /// submission.
    fn set_id(&mut self, id: usize);
}

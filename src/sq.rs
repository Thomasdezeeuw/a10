//! Submission Queue.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::{fmt, io, mem};

use crate::{Implementation, OperationId, QueuedOperation, SharedState};

/// Queue of completion events.
pub(crate) struct Queue<I: Implementation> {
    shared: Arc<SharedState<I>>,
}

impl<I: Implementation> Queue<I> {
    pub(crate) const fn new(shared: Arc<SharedState<I>>) -> Queue<I> {
        Queue { shared }
    }

    /// Add a new submission, returns the id (index).
    pub(crate) fn add<F>(&self, submit: F) -> Result<usize, QueueFull>
    where
        F: FnOnce(&mut <I::Submissions as Submissions>::Submission),
    {
        // Get an id (index) to the queued operation list.
        let shared = &*self.shared;
        let Some(id) = shared.op_ids.next_available() else {
            return Err(QueueFull);
        };

        let queued_op = QueuedOperation::new();
        // SAFETY: the `AtomicBitMap` always returns valid indices for
        // `op_queue` (it's the whole point of it).
        let mut op = shared.queued_ops[id].lock().unwrap();
        let old_queued_op = mem::replace(&mut *op, Some(queued_op));
        debug_assert!(old_queued_op.is_none());

        let result = shared.submissions.add(&shared.data, |submission| {
            submit(submission);
            submission.set_id(id);
        });
        if let Err(QueueFull) = result {
            // Release operation slot.
            {
                *shared.queued_ops[id].lock().unwrap() = None;
            }
            shared.op_ids.make_available(id);

            return Err(QueueFull);
        }

        Ok(id)
    }

    pub(crate) fn wake(&self) {
        if !self.shared.is_polling.load(Ordering::Acquire) {
            // Not polling, no need to wake up.
            return;
        }

        if let Err(err) = self.shared.submissions.wake(&self.shared.data) {
            log::error!("failed to wake a10::Ring: {err}");
        }
    }
}

impl<I: Implementation> Clone for Queue<I> {
    fn clone(&self) -> Self {
        Queue {
            shared: self.shared.clone(),
        }
    }

    fn clone_from(&mut self, source: &Self) {
        self.shared.clone_from(&source.shared)
    }
}

impl<I: Implementation> fmt::Debug for Queue<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("sq::Queue")
            .field("shared", &self.shared)
            .finish()
    }
}

/// Submit operations.
pub(crate) trait Submissions: fmt::Debug {
    /// Data shared between the submission and completion queues.
    type Shared: fmt::Debug + Sized;

    /// Type of the submission.
    type Submission: Submission;

    /// Try to add a new submission.
    fn add<F>(&self, shared: &Self::Shared, submit: F) -> Result<(), QueueFull>
    where
        F: FnOnce(&mut Self::Submission);

    /// Wake a polling thread.
    fn wake(&self, shared: &Self::Shared) -> io::Result<()>;
}

/// Submission event.
pub(crate) trait Submission: fmt::Debug {
    /// Set the identifier of operation.
    ///
    /// This must cause the relevant [`cq::Event::id`] of the completion event
    /// to return `id`.
    fn set_id(&mut self, id: OperationId);
}

/// Submission queue is full.
pub(crate) struct QueueFull;

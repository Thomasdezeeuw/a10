//! Submission Queue.

use std::sync::atomic::Ordering;
use std::sync::{Arc, MutexGuard};
use std::{fmt, io, mem, task};

use crate::{cq, Implementation, OperationId, QueuedOperation, SharedState};

/// Queue of completion events.
pub(crate) struct Queue<I: Implementation> {
    shared: Arc<SharedState<I>>,
}

impl<I: Implementation> Queue<I> {
    pub(crate) const fn new(shared: Arc<SharedState<I>>) -> Queue<I> {
        Queue { shared }
    }

    /// Add a new submission, returns the id (index).
    ///
    /// If this returns `QueueFull` it will use the `waker` to wait for a
    /// submission.
    pub(crate) fn submit<F>(&self, fill: F, waker: task::Waker) -> Result<OperationId, QueueFull>
    where
        F: FnOnce(&mut <I::Submissions as Submissions>::Submission),
    {
        // Get an `OperationId` to the queued operation list.
        let shared = &*self.shared;
        let Some(op_id) = shared.op_ids.next_available() else {
            return Err(QueueFull);
        };

        let queued_op = QueuedOperation::new(waker);
        // SAFETY: the `AtomicBitMap` always returns valid indices for
        // `op_queue` (it's the whole point of it).
        {
            let mut op = shared.queued_ops[op_id].lock().unwrap();
            debug_assert!(op.is_none());
            *op = Some(queued_op);
        }

        match self.submit_with_id(op_id, fill) {
            Ok(()) => Ok(op_id),
            Err(QueueFull) => {
                // Release operation slot.
                // SAFETY: `unwrap`s are safe as we set the operation above.
                let queued_op = { shared.queued_ops[op_id].lock().unwrap().take() };
                debug_assert!(queued_op.is_some());
                shared.op_ids.make_available(op_id);
                Err(QueueFull)
            }
        }
    }

    /// Re-adds a submission, reusing `op_id`.
    ///
    /// If this returns `QueueFull` `op_id` becomes invalid.
    pub(crate) fn resubmit<F>(&self, op_id: OperationId, fill: F) -> Result<(), QueueFull>
    where
        F: FnOnce(&mut <I::Submissions as Submissions>::Submission),
    {
        let shared = &*self.shared;
        match self.submit_with_id(op_id, fill) {
            Ok(()) => Ok(()),
            Err(QueueFull) => {
                // Release operation slot.
                // SAFETY: `unwrap`s are safe as we set the operation above.
                let queued_op = { shared.queued_ops[op_id].lock().unwrap().take() };
                debug_assert!(queued_op.is_some());
                shared.op_ids.make_available(op_id);
                Err(QueueFull)
            }
        }
    }

    /// Add a new submission using an existing operation `id`.
    ///
    /// If this returns `QueueFull` it will use the `waker` in
    /// `queued_ops[op_id]` to wait for a submission.
    fn submit_with_id<F>(&self, op_id: OperationId, fill: F) -> Result<(), QueueFull>
    where
        F: FnOnce(&mut <I::Submissions as Submissions>::Submission),
    {
        let shared = &*self.shared;
        let result = shared.submissions.add(&shared.data, |submission| {
            fill(submission);
            submission.set_id(op_id);
        });
        match result {
            Ok(()) => Ok(()),
            Err(QueueFull) => {
                if let Some(op) = shared.queued_ops[op_id].lock().unwrap().as_ref() {
                    self.wait_for_submission(op.waker.clone());
                }
                Err(QueueFull)
            }
        }
    }

    /// Wait for a submission slot, waking `waker` once one is available.
    pub(crate) fn wait_for_submission(&self, waker: task::Waker) {
        log::trace!(waker:? = waker; "adding blocked future");
        self.shared.blocked_futures.lock().unwrap().push(waker)
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

    /// # Safety
    ///
    /// The `id` must come from [`Queue::submit`] and must not be invalid, e.g.
    /// by using [`Queue::resubmit`].
    pub(crate) unsafe fn get_op(
        &self,
        id: OperationId,
    ) -> MutexGuard<
        Option<QueuedOperation<<<I::Completions as cq::Completions>::Event as cq::Event>::State>>,
    > {
        // SAFETY: we don't poison locks.
        self.shared.queued_ops[id].lock().unwrap()
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

//! Submission Queue.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, MutexGuard};
use std::{fmt, io, mem, task};

use crate::drop_waker::{drop_task_waker, DropWake};
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
        self.submit2(
            <<<I::Completions as cq::Completions>::Event as cq::Event>::State as cq::OperationState>::new,
            fill,
            waker,
        )
    }

    /// Add a new multishot submission, returns the id (index).
    ///
    /// If this returns `QueueFull` it will use the `waker` to wait for a
    /// submission.
    pub(crate) fn submit_multishot<F>(
        &self,
        fill: F,
        waker: task::Waker,
    ) -> Result<OperationId, QueueFull>
    where
        F: FnOnce(&mut <I::Submissions as Submissions>::Submission),
    {
        self.submit2(
            <<<I::Completions as cq::Completions>::Event as cq::Event>::State as cq::OperationState>::new_multishot,
            fill,
            waker,
        )
    }

    /// Add a new submission, returns the id (index).
    ///
    /// If this returns `QueueFull` it will use the `waker` to wait for a
    /// submission.
    fn submit2<F, S>(
        &self,
        new_state: S,
        fill: F,
        waker: task::Waker,
    ) -> Result<OperationId, QueueFull>
    where
        F: FnOnce(&mut <I::Submissions as Submissions>::Submission),
        S: FnOnce() -> <<I::Completions as cq::Completions>::Event as cq::Event>::State,
    {
        let op_id = self.queue2(new_state, waker)?;

        // SAFETY: we just got the `op_id` above so we own it. Furthermore we
        // don't use it in case an error is returned.
        unsafe { self.submit_with_id(op_id, fill)? };

        Ok(op_id)
    }

    /// Queue a new multishot operation, without submitting an operation,
    /// returns the operation id.
    pub(crate) fn queue_multishot(&self) -> Result<OperationId, QueueFull> {
        self.queue2(
            <<<I::Completions as cq::Completions>::Event as cq::Event>::State as cq::OperationState>::new_multishot,
            task::Waker::noop().clone(),
        )
    }

    fn queue2<S>(&self, new_state: S, waker: task::Waker) -> Result<OperationId, QueueFull>
    where
        S: FnOnce() -> <<I::Completions as cq::Completions>::Event as cq::Event>::State,
    {
        // Get an `OperationId` to the queued operation list.
        let shared = &*self.shared;
        let Some(op_id) = shared.op_ids.next_available() else {
            self.wait_for_submission(waker);
            return Err(QueueFull);
        };

        let queued_op = QueuedOperation::new(new_state(), waker);
        // SAFETY: the `AtomicBitMap` always returns valid indices for
        // `op_queue` (it's the whole point of it).
        {
            let mut op = shared.queued_ops[op_id].lock().unwrap();
            debug_assert!(op.is_none());
            *op = Some(queued_op);
        }

        Ok(op_id)
    }

    /// Re-adds a submission, reusing `op_id`.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `op_id` is valid and owned by them.
    ///
    /// If this returns `QueueFull` `op_id` becomes invalid.
    pub(crate) unsafe fn resubmit<F>(&self, op_id: OperationId, fill: F) -> Result<(), QueueFull>
    where
        F: FnOnce(&mut <I::Submissions as Submissions>::Submission),
    {
        unsafe { self.submit_with_id(op_id, fill) }
    }

    /// Add a new submission using an existing operation `id`.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `op_id` is valid and owned by them.
    ///
    /// If this returns `QueueFull`it will use `op_id` to remove the queued
    /// operation, invalidating `op_id`, and use it's waker to wait for a
    /// submission slot.
    unsafe fn submit_with_id<F>(&self, op_id: OperationId, fill: F) -> Result<(), QueueFull>
    where
        F: FnOnce(&mut <I::Submissions as Submissions>::Submission),
    {
        let shared = &*self.shared;
        let result = shared
            .submissions
            .add(&shared.data, &shared.is_polling, |submission| {
                fill(submission);
                submission.set_id(op_id);
            });
        match result {
            Ok(()) => Ok(()),
            Err(QueueFull) => {
                // Release operation slot.
                // SAFETY: `unwrap`s are safe as the caller must ensure it's
                // valid.
                let queued_op = { shared.queued_ops[op_id].lock().unwrap().take().unwrap() };
                shared.op_ids.make_available(op_id);
                self.wait_for_submission(queued_op.waker);
                Err(QueueFull)
            }
        }
    }

    /// Add a new submission, without waiting for a result.
    ///
    /// This marks the submission to not generate a completion event (as it will
    /// be discarded any way).
    pub(crate) fn submit_no_completion<F>(&self, fill: F) -> Result<(), QueueFull>
    where
        F: FnOnce(&mut <I::Submissions as Submissions>::Submission),
    {
        let shared = &*self.shared;
        shared
            .submissions
            .add(&shared.data, &shared.is_polling, |submission| {
                submission.set_id(crate::NO_COMPLETION_ID);
                submission.no_completion_event();
                fill(submission);
            })
    }

    /// Cancel an operation with `op_id`.
    ///
    /// # Safety
    ///
    /// After this function is called the queued operation with `op_id` may no
    /// longer be accessed.
    pub(crate) unsafe fn cancel<T: DropWake>(&self, op_id: OperationId, resources: T) {
        let shared = &*self.shared;
        let mut queued_op_slot = { shared.queued_ops[op_id].lock().unwrap() };
        let queued_op = { queued_op_slot.as_mut().unwrap() };

        if queued_op.done {
            // Easy path, the operation has already been completed.
            *queued_op_slot = None;
            // Unlock defore dropping `resources`, which might take a while.
            drop(queued_op_slot);
            shared.op_ids.make_available(op_id);

            // We can safely drop the resources.
            drop(resources);
            return;
        }

        // Harder path, the operation is not done, but the Future holding the
        // resource is about to be dropped, so we need to cancel the operation.
        match shared
            .submissions
            .cancel(&shared.data, &shared.is_polling, op_id)
        {
            Cancelled::Immediate => {
                // Operation has been cancelled, we can drop the resources and
                // make the slot available.
                *queued_op_slot = None;
                // Unlock defore dropping `resources`, which might take a while.
                drop(queued_op_slot);
                shared.op_ids.make_available(op_id);

                // We can safely drop the resources.
                drop(resources);
            }
            Cancelled::Async => {
                // Hardest path, the operation is cancelled asynchronously,
                // which means the kernel still has access to the resources and
                // we can't drop them yet.
                //
                // We need to do two things:
                // 1. Delay the dropping of `resources` until the kernel is done
                //    with the operation.
                // 2. Delay the available making of the queued operation slot
                //    until the kernel is done with the operation.
                //
                // We achieve 1 by creating a special waker that just drops the
                // resources (created by `drop_task_waker`).
                // 2. is achieved by `cq::Queue::poll`, which makes the slot
                // available if the operation is dropped and expects no more
                // events.
                queued_op.dropped = true;
                if mem::needs_drop::<T>() {
                    // SAFETY: not cloning the waker.
                    queued_op.waker = unsafe { drop_task_waker(resources) };
                }
            }
        }
    }

    /// Wait for a submission slot, waking `waker` once one is available.
    pub(crate) fn wait_for_submission(&self, waker: task::Waker) {
        log::trace!(waker:? = waker; "adding blocked future");
        self.shared.blocked_futures.lock().unwrap().push(waker);
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

    /// Get the queued operation with `id`.
    ///
    /// # Safety
    ///
    /// The `id` must come from [`Queue::submit`] and must not be invalid, e.g.
    /// by using [`Queue::resubmit`].
    #[allow(clippy::type_complexity)]
    pub(crate) unsafe fn get_op(
        &self,
        op_id: OperationId,
    ) -> MutexGuard<
        Option<QueuedOperation<<<I::Completions as cq::Completions>::Event as cq::Event>::State>>,
    > {
        // SAFETY: we don't poison locks.
        self.shared.queued_ops[op_id].lock().unwrap()
    }

    /// Make operation with `id` available.
    ///
    /// # Safety
    ///
    /// The `id` must come from [`Queue::submit`] and must not be invalid, e.g.
    /// by using [`Queue::resubmit`].
    ///
    /// After this call `id` is invalid.
    #[allow(clippy::type_complexity)]
    pub(crate) unsafe fn make_op_available(
        &self,
        op_id: OperationId,
        mut op: MutexGuard<
            Option<
                QueuedOperation<<<I::Completions as cq::Completions>::Event as cq::Event>::State>,
            >,
        >,
    ) {
        // SAFETY: we don't poison locks.
        *op = None;
        drop(op);
        self.shared.op_ids.make_available(op_id);
    }

    /// Returns the implementation specific shared data.
    pub(crate) fn shared_data(&self) -> &I::Shared {
        &self.shared.data
    }
}

impl<I: Implementation> Clone for Queue<I> {
    fn clone(&self) -> Self {
        Queue {
            shared: self.shared.clone(),
        }
    }

    fn clone_from(&mut self, source: &Self) {
        self.shared.clone_from(&source.shared);
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
    fn add<F>(
        &self,
        shared: &Self::Shared,
        is_polling: &AtomicBool,
        submit: F,
    ) -> Result<(), QueueFull>
    where
        F: FnOnce(&mut Self::Submission);

    /// Try to cancel an operation.
    fn cancel(
        &self,
        shared: &Self::Shared,
        is_polling: &AtomicBool,
        op_id: OperationId,
    ) -> Cancelled;

    /// Wake a polling thread.
    fn wake(&self, shared: &Self::Shared) -> io::Result<()>;
}

/// Result of [cancelling] an operation.
///
/// [cancelling]: Submissions::cancel
pub(crate) enum Cancelled {
    /// Operation is cancelled synchronously, operation is already cancelled.
    Immediate,
    /// Operation is cancelled asynchronously, operation is still in progress.
    Async,
}

/// Submission event.
pub(crate) trait Submission: fmt::Debug {
    /// Set the identifier of operation.
    ///
    /// This must cause the relevant [`cq::Event::id`] of the completion event
    /// to return `id`.
    fn set_id(&mut self, id: OperationId);

    /// Don't return a completion event for this submission.
    fn no_completion_event(&mut self);
}

/// Submission queue is full.
pub(crate) struct QueueFull;

impl From<QueueFull> for io::Error {
    fn from(_: QueueFull) -> io::Error {
        #[cfg(not(feature = "nightly"))]
        let kind = io::ErrorKind::Other;
        #[cfg(feature = "nightly")]
        let kind = io::ErrorKind::ResourceBusy;
        io::Error::new(kind, "submission queue is full")
    }
}

impl fmt::Debug for QueueFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueueFull").finish()
    }
}

impl fmt::Display for QueueFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("`a10::Ring` submission queue is full")
    }
}

//! Completion Queue.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use std::{fmt, io, mem};

use crate::{Implementation, OperationId, SharedState, NO_COMPLETION_ID, WAKE_ID};

/// Queue of completion events.
pub(crate) struct Queue<I: Implementation> {
    completions: I::Completions,
    shared: Arc<SharedState<I>>,
}

impl<I: Implementation> Queue<I> {
    pub(crate) const fn new(completions: I::Completions, shared: Arc<SharedState<I>>) -> Queue<I> {
        Queue {
            completions,
            shared,
        }
    }

    pub(crate) fn poll(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        self.shared.is_polling.store(true, Ordering::Release);
        let completions = match self.completions.poll(&self.shared.data, timeout) {
            Ok(completions) => completions,
            Err(err) => {
                self.shared.is_polling.store(false, Ordering::Release);
                return Err(err);
            }
        };

        for completion in completions {
            log::trace!(completion:? = completion; "dequeued completion event");
            let id = completion.id();
            let Some(queued_op) = self.shared.queued_ops.get(id) else {
                if id == WAKE_ID {
                    /* Wake up only. */
                } else if id == NO_COMPLETION_ID {
                    log::warn!(completion:? = completion; "operation without completion failed");
                } else {
                    log::trace!(completion:? = completion; "got completion for unknown operation");
                }
                continue;
            };

            let mut queued_op = queued_op.lock().unwrap();
            let Some(op) = &mut *queued_op else {
                log::debug!(completion:? = completion; "operation gone, but got completion event");
                continue;
            };

            log::trace!(completion:? = completion; "updating operation");
            let more_events = completion.update_state(&mut op.state);
            op.done = !more_events;
            if op.dropped && !more_events {
                // The Future was previously dropped so no one is waiting on the
                // result. We can make the slot avaiable again.
                *queued_op = None;
                drop(queued_op);
                log::trace!(id = id; "marking slot as available");
                self.shared.op_ids.make_available(id);
            } else {
                log::trace!(completion:? = completion; "waking future");
                op.waker.wake_by_ref();
            }
        }

        self.wake_blocked_futures();
        self.shared.is_polling.store(false, Ordering::Release);
        Ok(())
    }

    /// Wake any futures that were blocked on a submission slot.
    // Work around <https://github.com/rust-lang/rust-clippy/issues/8539>.
    #[allow(clippy::iter_with_drain, clippy::needless_pass_by_ref_mut)]
    fn wake_blocked_futures(&mut self) {
        // TODO: check the actual amount of submissions slot available and limit
        // the amount of
        let mut blocked_futures = self.shared.blocked_futures.lock().unwrap();
        if !blocked_futures.is_empty() {
            let mut wakers = mem::take(&mut *blocked_futures);
            drop(blocked_futures); // Unblock other threads.
            for waker in wakers.drain(..) {
                waker.wake();
            }

            // Reuse allocation.
            let mut blocked_futures = self.shared.blocked_futures.lock().unwrap();
            mem::swap(&mut *blocked_futures, &mut wakers);
            drop(blocked_futures);
            // In case any wakers where added wake those as well.
            for waker in wakers {
                waker.wake();
            }
        }
    }

    pub(crate) fn shared(&self) -> &SharedState<I> {
        &self.shared
    }
}

impl<I: Implementation> fmt::Debug for Queue<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("cq::Queue")
            .field("completions", &self.completions)
            .field("shared", &self.shared)
            .finish()
    }
}

/// Poll for completition events.
pub(crate) trait Completions: fmt::Debug {
    /// Data shared between the submission and completion queues.
    type Shared: fmt::Debug + Sized;

    /// Completiton [`Event`] (ce).
    type Event: Event + Sized;

    /// Poll for new completion events.
    fn poll<'a>(
        &'a mut self,
        shared: &Self::Shared,
        timeout: Option<Duration>,
    ) -> io::Result<impl Iterator<Item = &'a Self::Event>>;
}

/// Completition event.
pub(crate) trait Event: fmt::Debug {
    /// State of an operation.
    type State: OperationState;

    /// Identifier of the operation.
    fn id(&self) -> OperationId;

    /// Update the state of the operation.
    ///
    /// Returns a boolean indicating if more events are expected for the same
    /// operation id.
    fn update_state(&self, state: &mut Self::State) -> bool;
}

/// State of an operation.
pub(crate) trait OperationState: fmt::Debug {
    /// Create a queued operation.
    fn new() -> Self;

    /// Create a queued multishot operation.
    fn new_multishot() -> Self;
}

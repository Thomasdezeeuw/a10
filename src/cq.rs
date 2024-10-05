//! Completion Queue.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use std::{fmt, io};

use crate::SharedState;

/// Queue of completion events.
pub(crate) struct Queue<C: Completions> {
    completions: C,
    shared: Arc<SharedState<C::Shared, <C::Event as Event>::State>>,
}

impl<C: Completions> Queue<C> {
    pub(crate) const fn new(
        completions: C,
        shared: Arc<SharedState<C::Shared, <C::Event as Event>::State>>,
    ) -> Queue<C> {
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
                log::trace!(completion:? = completion; "invalid id: {id}");
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
                self.shared.op_indices.make_available(id);
            } else if let Some(waker) = op.waker.take() {
                log::trace!(completion:? = completion; "waking future");
                waker.wake();
            }
        }
        self.shared.is_polling.store(false, Ordering::Release);
        Ok(())
    }
}

impl<C: Completions> fmt::Debug for Queue<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO.
        f.write_str("cq::Queue")
    }
}

/// Poll for completition events.
pub(crate) trait Completions: fmt::Debug {
    /// Data shared between the submission and completion queues.
    type Shared: Sized;

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
    type State: Default;

    /// Identifier (index) of the event.
    fn id(&self) -> usize;

    /// Update the state of the operation.
    ///
    /// Returns a boolean indicating if more events are expected for the same
    /// operation id.
    fn update_state(&self, state: &mut Self::State) -> bool;
}

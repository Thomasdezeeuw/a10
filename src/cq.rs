//! Completition Queue.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{fmt, io, task};

use crate::bitmap::AtomicBitMap;
use crate::{Poll, SharedState};

/// Queue of completion events.
#[derive(Debug)]
pub(crate) struct CompletionQueue<P: Poll> {
    poll: P,
    shared: Arc<SharedState<P>>,
}

impl<P: Poll> CompletionQueue<P> {
    pub(crate) fn poll(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        for completion in self.poll.poll(&self.shared.data, timeout)? {
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
        Ok(())
    }
}

/// Completition event.
///
/// NOTE: the actual implementation is platform dependent.
pub(crate) trait Event: fmt::Debug {
    /// State of an operation.
    type State;

    /// Identifier (index) of the event.
    fn id(&self) -> usize;

    /// Update the state of the operation.
    ///
    /// Returns a boolean indicating if more events are expected for the same
    /// operation id.
    fn update_state(&self, state: &mut Self::State) -> bool;
}

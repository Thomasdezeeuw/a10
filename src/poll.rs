//! Poll for file descriptor events.
//!
//! To wait for events on a file descriptor use:
//!  * [`SubmissionQueue::oneshot_poll`] a [`Future`] returning a single
//!    [`PollEvent`].
//!  * [`SubmissionQueue::multishot_poll`] an [`AsyncIterator`] returning
//!    multiple [`PollEvent`]s.
//!
//! [`AsyncIterator`]: std::async_iter::AsyncIterator

use std::future::Future;
use std::os::fd::RawFd;
use std::pin::Pin;
use std::task::{self, Poll};
use std::{fmt, io};

use crate::cancel::{Cancel, CancelOp, CancelResult};
use crate::op::{poll_state, OpState};
use crate::{QueueFull, SubmissionQueue};

/// [`Future`] behind [`SubmissionQueue::oneshot_poll`].
#[derive(Debug)]
#[must_use = "`Future`s do nothing unless polled"]
#[allow(clippy::module_name_repetitions)]
pub struct OneshotPoll<'a> {
    sq: &'a SubmissionQueue,
    state: OpState<(RawFd, u32)>,
}

impl<'a> OneshotPoll<'a> {
    /// Create a new `OneshotPoll`.
    pub(crate) const fn new(sq: &'a SubmissionQueue, fd: RawFd, mask: u32) -> OneshotPoll {
        OneshotPoll {
            sq,
            state: OpState::NotStarted((fd, mask)),
        }
    }
}

impl<'a> Future for OneshotPoll<'a> {
    type Output = io::Result<PollEvent>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let op_index = poll_state!(
            OneshotPoll,
            self.state,
            self.sq,
            ctx,
            |submission, (fd, mask)| unsafe {
                submission.poll(fd, mask);
            }
        );

        match self.sq.poll_op(ctx, op_index) {
            Poll::Ready(result) => {
                self.state = OpState::Done;
                match result {
                    Ok((_, events)) => Poll::Ready(Ok(PollEvent { events })),
                    Err(err) => Poll::Ready(Err(err)),
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<'a> Cancel for OneshotPoll<'a> {
    fn try_cancel(&mut self) -> CancelResult {
        self.state.try_cancel(self.sq)
    }

    fn cancel(&mut self) -> CancelOp {
        self.state.cancel(self.sq)
    }
}

impl<'a> Drop for OneshotPoll<'a> {
    fn drop(&mut self) {
        if let OpState::Running(op_index) = self.state {
            let result = self.sq.cancel_op(op_index, (), |submission| unsafe {
                submission.remove_poll(op_index);
                // We'll get a canceled completion event if we succeeded, which
                // is sufficient to cleanup the operation.
                submission.no_completion_event();
            });
            if let Err(err) = result {
                log::error!("error submitting poll removal operation for a10::OneshotPoll: {err}");
            }
        }
    }
}

/// [`AsyncIterator`] behind [`SubmissionQueue::multishot_poll`].
///
/// [`AsyncIterator`]: std::async_iter::AsyncIterator
#[derive(Debug)]
#[must_use = "`Future`s do nothing unless polled"]
#[allow(clippy::module_name_repetitions)]
pub struct MultishotPoll<'a> {
    sq: &'a SubmissionQueue,
    state: OpState<(RawFd, u32)>,
}

impl<'a> MultishotPoll<'a> {
    /// Create a new `MultishotPoll`.
    pub(crate) const fn new(sq: &'a SubmissionQueue, fd: RawFd, mask: u32) -> MultishotPoll {
        MultishotPoll {
            sq,
            state: OpState::NotStarted((fd, mask)),
        }
    }

    /// This is the same as the `AsyncIterator::poll_next` function, but then
    /// available on stable Rust.
    pub fn poll_next(
        mut self: Pin<&mut Self>,
        ctx: &mut task::Context<'_>,
    ) -> Poll<Option<io::Result<PollEvent>>> {
        let op_index = match self.state {
            OpState::Running(op_index) => op_index,
            OpState::NotStarted((fd, mask)) => {
                let result = self.sq.add_multishot(|submission| unsafe {
                    submission.multishot_poll(fd, mask);
                });
                match result {
                    Ok(op_index) => {
                        self.state = OpState::Running(op_index);
                        op_index
                    }
                    Err(QueueFull(())) => {
                        self.sq.wait_for_submission(ctx.waker().clone());
                        return Poll::Pending;
                    }
                }
            }
            OpState::Done => return Poll::Ready(None),
        };

        match self.sq.poll_multishot_op(ctx, op_index) {
            Poll::Ready(Some(Result::Ok((_, events)))) => {
                Poll::Ready(Some(Result::Ok(PollEvent { events })))
            }
            Poll::Ready(Some(Result::Err(err))) => {
                // After an error we also don't expect any more results.
                self.state = OpState::Done;
                if let Some(libc::ECANCELED) = err.raw_os_error() {
                    // Operation was canceled, so we expect no more
                    // results.
                    Poll::Ready(None)
                } else {
                    Poll::Ready(Some(Result::Err(err)))
                }
            }
            Poll::Ready(None) => {
                self.state = OpState::Done;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(feature = "nightly")]
impl<'a> std::async_iter::AsyncIterator for MultishotPoll<'a> {
    type Item = io::Result<PollEvent>;

    fn poll_next(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Option<Self::Item>> {
        self.poll_next(ctx)
    }
}

impl<'a> Cancel for MultishotPoll<'a> {
    fn try_cancel(&mut self) -> CancelResult {
        self.state.try_cancel(self.sq)
    }

    fn cancel(&mut self) -> CancelOp {
        self.state.cancel(self.sq)
    }
}

/// Event returned by [`OneshotPoll`].
#[derive(Copy, Clone)]
#[allow(clippy::module_name_repetitions)]
pub struct PollEvent {
    events: libc::c_int,
}

impl PollEvent {
    /// There is data to read.
    #[doc(alias = "POLLIN")]
    pub const fn is_readable(&self) -> bool {
        (self.events & libc::POLLIN as libc::c_int) != 0
    }

    /// There is some exceptional condition on the file descriptor.
    #[doc(alias = "POLLPRI")]
    pub const fn is_priority(&self) -> bool {
        (self.events & libc::POLLPRI as libc::c_int) != 0
    }

    /// Writing is now possible.
    #[doc(alias = "POLLOUT")]
    pub const fn is_writable(&self) -> bool {
        (self.events & libc::POLLOUT as libc::c_int) != 0
    }

    /// Stream socket peer closed connection, or shut down writing half of
    /// connection.
    #[doc(alias = "POLLRDHUP")]
    pub const fn is_read_hup(&self) -> bool {
        (self.events & libc::POLLRDHUP as libc::c_int) != 0
    }

    /// Error condition.
    #[doc(alias = "POLLERR")]
    pub const fn is_error(&self) -> bool {
        (self.events & libc::POLLERR as libc::c_int) != 0
    }

    /// Hang up.
    #[doc(alias = "POLLHUP")]
    pub const fn is_hup(&self) -> bool {
        (self.events & libc::POLLHUP as libc::c_int) != 0
    }

    /// Returns a bitmask indicating which events occured, see the `poll(2)`
    /// system call manual and `libc::POLL*` constants, e.g. `libc::POLLIN`.
    pub const fn events_mask(&self) -> libc::c_int {
        self.events
    }
}

/// Known poll events supported by Linux as of v6.3.
const KNOWN_EVENTS: [(libc::c_short, &str); 11] = [
    (libc::POLLIN, "POLLIN"),
    (libc::POLLPRI, "POLLPRI"),
    (libc::POLLOUT, "POLLOUT"),
    (libc::POLLERR, "POLLERR"),
    (libc::POLLHUP, "POLLHUP"),
    (libc::POLLNVAL, "POLLNVAL"),
    (libc::POLLRDNORM, "POLLRDNORM"),
    (libc::POLLRDBAND, "POLLRDBAND"),
    (libc::POLLWRNORM, "POLLWRNORM"),
    (libc::POLLWRBAND, "POLLWRBAND"),
    (libc::POLLRDHUP, "POLLRDHUP"),
];

impl fmt::Debug for PollEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let events = KNOWN_EVENTS.into_iter().filter_map(|(event, name)| {
            (self.events & libc::c_int::from(event) != 0).then_some(name)
        });
        f.debug_list().entries(events).finish()
    }
}

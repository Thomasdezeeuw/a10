//! Poll for file descriptor events.

use std::future::Future;
use std::os::fd::RawFd;
use std::pin::Pin;
use std::task::{self, Poll};
use std::{fmt, io};

use crate::cancel::{Cancel, CancelOp, CancelResult};
use crate::op::{poll_state, OpState};
use crate::SubmissionQueue;

/// [`Future`] behind [`SubmissionQueue::oneshot_poll`].
#[derive(Debug)]
#[must_use = "`Future`s do nothing unless polled"]
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

/// Event returned by [`OneshotPoll`].
#[derive(Copy, Clone)]
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

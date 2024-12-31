//! Poll for file descriptor events.
//!
//! To wait for events on a file descriptor use:
//!  * [`oneshot_poll`] a [`Future`] returning a single [`PollEvent`].
//!  * [`multishot_poll`] an [`AsyncIterator`] returning multiple
//!    [`PollEvent`]s.
//!
//! Note that module only supports regular file descriptors, not direct
//! descriptors as it doesn't make much sense to poll a direct descriptor,
//! instead start the I/O operation you want to perform.
//!
//! [`AsyncIterator`]: std::async_iter::AsyncIterator

use std::os::fd::{AsRawFd, BorrowedFd};
use std::{fmt, io};

use crate::op::{operation, Operation};
use crate::{man_link, sys, SubmissionQueue};

/// Wait for an event specified in `mask` on the file descriptor `fd`.
///
/// Ths is similar to calling `poll(2)` on the file descriptor.
///
/// # Notes
///
/// In general it's more efficient to perform the I/O operation you want to
/// perform instead of polling for it to be ready.
#[doc = man_link!(poll(2))]
#[doc(alias = "poll")]
#[doc(alias = "epoll")]
#[doc(alias = "select")]
#[allow(clippy::module_name_repetitions)]
pub fn oneshot_poll(sq: SubmissionQueue, fd: BorrowedFd, mask: libc::c_int) -> OneshotPoll {
    OneshotPoll(Operation::new(sq, (), (fd.as_raw_fd(), mask)))
}

operation!(
    /// [`Future`] behind [`oneshot_poll`].
    pub struct OneshotPoll(sys::poll::OneshotPollOp) -> io::Result<PollEvent>;
);

/// Event returned by [`OneshotPoll`].
#[derive(Copy, Clone)]
#[allow(clippy::module_name_repetitions)]
pub struct PollEvent(pub(crate) libc::c_int);

impl PollEvent {
    /// There is data to read.
    #[doc(alias = "POLLIN")]
    pub const fn is_readable(&self) -> bool {
        (self.0 & libc::POLLIN as libc::c_int) != 0
    }

    /// There is some exceptional condition on the file descriptor.
    #[doc(alias = "POLLPRI")]
    pub const fn is_priority(&self) -> bool {
        (self.0 & libc::POLLPRI as libc::c_int) != 0
    }

    /// Writing is now possible.
    #[doc(alias = "POLLOUT")]
    pub const fn is_writable(&self) -> bool {
        (self.0 & libc::POLLOUT as libc::c_int) != 0
    }

    /// Stream socket peer closed connection, or shut down writing half of
    /// connection.
    #[doc(alias = "POLLRDHUP")]
    pub const fn is_read_hup(&self) -> bool {
        (self.0 & libc::POLLRDHUP as libc::c_int) != 0
    }

    /// Error condition.
    #[doc(alias = "POLLERR")]
    pub const fn is_error(&self) -> bool {
        (self.0 & libc::POLLERR as libc::c_int) != 0
    }

    /// Hang up.
    #[doc(alias = "POLLHUP")]
    pub const fn is_hup(&self) -> bool {
        (self.0 & libc::POLLHUP as libc::c_int) != 0
    }

    /// Returns a bitmask indicating which events occured, see the `poll(2)`
    /// system call manual and `libc::POLL*` constants, e.g. `libc::POLLIN`.
    pub const fn events_mask(&self) -> libc::c_int {
        self.0
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
        let events = KNOWN_EVENTS
            .into_iter()
            .filter_map(|(event, name)| (self.0 & libc::c_int::from(event) != 0).then_some(name));
        f.debug_list().entries(events).finish()
    }
}

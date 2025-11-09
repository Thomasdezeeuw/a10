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
//! [`Future`]: std::future::Future
//! [`AsyncIterator`]: std::async_iter::AsyncIterator

use std::os::fd::{AsRawFd, BorrowedFd};
use std::{fmt, io};

use crate::op::{iter_operation, operation, Operation};
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

/// Returns an [`AsyncIterator`] that returns multiple events as specified
/// in `mask` on the file descriptor `fd`.
///
/// This is not the same as calling [`oneshot_poll`] in a loop as this uses a
/// multishot operation, which means only a single operation is created kernel
/// side, making this more efficient.
///
/// [`AsyncIterator`]: std::async_iter::AsyncIterator
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
pub fn multishot_poll(sq: SubmissionQueue, fd: BorrowedFd, mask: libc::c_int) -> MultishotPoll {
    MultishotPoll(Operation::new(sq, (), (fd.as_raw_fd(), mask)))
}

iter_operation!(
    /// [`AsyncIterator`] behind [`multishot_poll`].
    pub struct MultishotPoll(sys::poll::MultishotPollOp) -> io::Result<PollEvent>;
);

/// Event returned by [`OneshotPoll`].
#[derive(Copy, Clone)]
#[allow(clippy::module_name_repetitions)]
pub struct PollEvent(pub(crate) libc::c_int);

impl PollEvent {
    /// There is data to read.
    #[doc(alias = "POLLIN")]
    #[doc(alias = "EPOLLIN")]
    pub const fn is_readable(&self) -> bool {
        (self.0 & libc::EPOLLIN) != 0
    }

    /// There is some exceptional condition on the file descriptor.
    #[doc(alias = "POLLPRI")]
    #[doc(alias = "EPOLLPRI")]
    pub const fn is_priority(&self) -> bool {
        (self.0 & libc::EPOLLPRI) != 0
    }

    /// Writing is now possible.
    #[doc(alias = "POLLOUT")]
    #[doc(alias = "EPOLLOUT")]
    pub const fn is_writable(&self) -> bool {
        (self.0 & libc::EPOLLOUT) != 0
    }

    /// Error condition.
    #[doc(alias = "POLLERR")]
    #[doc(alias = "EPOLLERR")]
    pub const fn is_error(&self) -> bool {
        (self.0 & libc::EPOLLERR) != 0
    }

    /// Hang up.
    #[doc(alias = "POLLHUP")]
    #[doc(alias = "EPOLLHUP")]
    pub const fn is_hup(&self) -> bool {
        (self.0 & libc::EPOLLHUP) != 0
    }

    /// Stream socket peer closed connection, or shut down writing half of
    /// connection.
    #[doc(alias = "POLLRDHUP")]
    #[doc(alias = "EPOLLRDHUP")]
    pub const fn is_read_hup(&self) -> bool {
        (self.0 & libc::EPOLLRDHUP) != 0
    }
}

/// Known epoll events supported by Linux as of v6.17.
const KNOWN_EVENTS: [(libc::c_int, &str); 10] = [
    (libc::EPOLLIN, "EPOLLIN"),
    (libc::EPOLLPRI, "EPOLLPRI"),
    (libc::EPOLLOUT, "EPOLLOUT"),
    (libc::EPOLLERR, "EPOLLERR"),
    (libc::EPOLLHUP, "EPOLLHUP"),
    (libc::EPOLLRDNORM, "EPOLLRDNORM"),
    (libc::EPOLLRDBAND, "EPOLLRDBAND"),
    (libc::EPOLLWRNORM, "EPOLLWRNORM"),
    (libc::EPOLLWRBAND, "EPOLLWRBAND"),
    (libc::EPOLLRDHUP, "EPOLLRDHUP"),
];

impl fmt::Debug for PollEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let events = KNOWN_EVENTS
            .into_iter()
            .filter_map(|(event, name)| (self.0 & event != 0).then_some(name));
        f.debug_list().entries(events).finish()
    }
}

use std::io;

use crate::op::iter_operation;
use crate::{SubmissionQueue, sys};

/// State of [`Pollable`].
///
/// Basically an [`AsyncFd`], without the actual file descriptor as we use the
/// file descriptor of the submission queue.
#[derive(Debug)]
pub(crate) struct PollableState {
    #[cfg(any(
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "ios",
        target_os = "macos",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "tvos",
        target_os = "visionos",
        target_os = "watchos",
    ))]
    pub(crate) state: crate::sys::fd::State,
    pub(crate) sq: SubmissionQueue,
}

impl PollableState {
    pub(super) fn new(sq: SubmissionQueue) -> PollableState {
        PollableState {
            #[cfg(any(
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "ios",
                target_os = "macos",
                target_os = "netbsd",
                target_os = "openbsd",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos",
            ))]
            state: crate::sys::fd::State::new(),
            sq,
        }
    }
}

iter_operation!(
    /// [`AsyncIterator`] behind [`Ring::pollable`].
    ///
    /// [`Ring::pollable`]: crate::Ring::pollable
    pub struct Pollable(sys::poll::PollableOp) -> io::Result<()>;
);

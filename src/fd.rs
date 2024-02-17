//! Asynchronous file descriptor (fd).

use std::mem::ManuallyDrop;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::{fmt, io};

use crate::SubmissionQueue;

/// An open file descriptor.
///
/// All functions on `AsyncFd` are asynchronous and return a [`Future`].
///
/// [`Future`]: std::future::Future
pub struct AsyncFd {
    /// # Notes
    ///
    /// We use `ManuallyDrop` because we drop the fd using io_uring, not a
    /// blocking `close(2)` system call.
    fd: ManuallyDrop<OwnedFd>,
    pub(crate) sq: SubmissionQueue,
}

// NOTE: the implementations are split over the modules to give the `Future`
// implementation types a reasonable place in the docs.

impl AsyncFd {
    /// Create a new `AsyncFd`.
    pub const fn new(fd: OwnedFd, sq: SubmissionQueue) -> AsyncFd {
        AsyncFd {
            fd: ManuallyDrop::new(fd),
            sq,
        }
    }

    /// Create a new `AsyncFd` from a `RawFd`.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `fd` is valid and that it's no longer used
    /// by anything other than the returned `AsyncFd`.
    pub unsafe fn from_raw_fd(fd: RawFd, sq: SubmissionQueue) -> AsyncFd {
        AsyncFd::new(OwnedFd::from_raw_fd(fd), sq)
    }

    /// Creates a new independently owned `AsyncFd` that shares the same
    /// underlying file descriptor as the existing `AsyncFd`.
    #[doc(alias = "dup")]
    #[doc(alias = "dup2")]
    #[doc(alias = "F_DUPFD")]
    #[doc(alias = "F_DUPFD_CLOEXEC")]
    pub fn try_clone(&self) -> io::Result<AsyncFd> {
        let fd = self.fd.try_clone()?;
        Ok(AsyncFd::new(fd, self.sq.clone()))
    }

    /// Returns the `RawFd` of this `AsyncFd`.
    pub(crate) fn fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl AsFd for AsyncFd {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl fmt::Debug for AsyncFd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct AsyncFdSubmissionQueue<'a>(&'a SubmissionQueue);

        impl fmt::Debug for AsyncFdSubmissionQueue<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct("SubmissionQueue")
                    .field("ring_fd", &self.0.shared.ring_fd.as_raw_fd())
                    .finish()
            }
        }

        f.debug_struct("AsyncFd")
            .field("fd", &self.fd())
            .field("sq", &AsyncFdSubmissionQueue(&self.sq))
            .finish()
    }
}

impl Drop for AsyncFd {
    fn drop(&mut self) {
        let result = self.sq.add_no_result(|submission| unsafe {
            submission.close(self.fd());
            submission.no_completion_event();
        });
        if let Err(err) = result {
            log::error!("error submitting close operation for a10::AsyncFd: {err}");
        }
    }
}

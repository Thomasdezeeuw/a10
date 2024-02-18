//! Asynchronous file descriptor (fd).
//!
//! See [`AsyncFd`].

use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::{fmt, io};

use crate::op::Submission;
use crate::SubmissionQueue;

/// An open file descriptor.
///
/// All functions on `AsyncFd` are asynchronous and return a [`Future`].
///
/// `AsyncFd` comes on in of two kinds:
///  * `AsyncFd<`[`File`]`>`: regular file descriptor which can be used as a
///    regular file descriptor outside of io_uring.
///  * `AsyncFd<`[`Direct`]`>`: direct descriptor which can be only be used with
///    io_uring.
///
/// Direct descriptors can be faster, but their usage is limited to them being
/// limited to io_uring operations.
///
/// [`Future`]: std::future::Future
pub struct AsyncFd<D: Descriptor = File> {
    /// # Notes
    ///
    /// We use `ManuallyDrop` because we drop the fd using io_uring, not a
    /// blocking `close(2)` system call.
    fd: ManuallyDrop<OwnedFd>,
    pub(crate) sq: SubmissionQueue,
    kind: PhantomData<D>,
}

// NOTE: the implementations are split over the modules to give the `Future`
// implementation types a reasonable place in the docs.

impl AsyncFd<File> {
    /// Create a new `AsyncFd`.
    pub const fn new(fd: OwnedFd, sq: SubmissionQueue) -> AsyncFd {
        AsyncFd {
            fd: ManuallyDrop::new(fd),
            sq,
            kind: PhantomData,
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
}

impl<D: Descriptor> AsyncFd<D> {
    /// Returns the `RawFd` of this `AsyncFd`.
    pub(crate) fn fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl AsFd for AsyncFd<File> {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl<D: Descriptor> fmt::Debug for AsyncFd<D> {
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
            .field("kind", &D::fmt_dbg())
            .finish()
    }
}

impl<D: Descriptor> Drop for AsyncFd<D> {
    fn drop(&mut self) {
        let result = self.sq.add_no_result(|submission| unsafe {
            submission.close(self.fd());
            submission.no_completion_event();
            D::set_flags(submission);
        });
        if let Err(err) = result {
            log::error!("error submitting close operation for a10::AsyncFd: {err}");
        }
    }
}

/// What kind of descriptor is used [`File`] or [`Direct`].
#[allow(private_bounds)] // That's the point of the private module.
pub trait Descriptor: private::Descriptor {}

pub(crate) mod private {
    use crate::op::Submission;

    pub(crate) trait Descriptor {
        /// Set any additional flags in `submission`.
        fn set_flags(submission: &mut Submission);

        /// Debug representation of the descriptor.
        fn fmt_dbg() -> &'static str;
    }
}

/// Regular Unix file descriptors.
#[derive(Copy, Clone, Debug)]
pub enum File {}

impl Descriptor for File {}

impl private::Descriptor for File {
    fn set_flags(_: &mut Submission) {
        // No flags needed.
    }

    fn fmt_dbg() -> &'static str {
        "file descriptor"
    }
}

/// Direct descriptors are io_uring private file descriptors.
///
/// They avoid some of the overhead associated with thread shared file tables
/// and can be used in any io_uring request that takes a file descriptor.
/// However they cannot be used outside of io_uring.
#[derive(Copy, Clone, Debug)]
pub enum Direct {}

impl Descriptor for Direct {}

impl private::Descriptor for Direct {
    fn set_flags(submission: &mut Submission) {
        submission.direct_fd();
    }

    fn fmt_dbg() -> &'static str {
        "direct descriptor"
    }
}

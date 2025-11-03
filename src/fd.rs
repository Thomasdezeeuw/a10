//! Asynchronous file descriptor (fd).
//!
//! See [`AsyncFd`].

use std::marker::PhantomData;
use std::os::fd::{AsFd, BorrowedFd, IntoRawFd, OwnedFd, RawFd};
use std::{fmt, io};

use crate::{syscall, SubmissionQueue};

#[cfg(any(target_os = "android", target_os = "linux"))]
pub use crate::sys::fd::{Direct, ToDirect, ToFd};

/// An open file descriptor.
///
/// All functions on `AsyncFd` are asynchronous and return a [`Future`].
///
/// `AsyncFd` comes on in of two kinds:
///  * <code>AsyncFd<[File]></code>: regular file descriptor which can be used as a
///    regular file descriptor outside of io_uring.
///  * <code>AsyncFd<[Direct]></code>: direct descriptor which can be only be used with
///    io_uring.
///
/// Direct descriptors can be faster, but their usage is limited to them being
/// limited to io_uring operations.
///
/// An `AsyncFd` can be created using some of the following methods:
///  * Sockets can be opened using [`socket`].
///  * Sockets can also be accepted using [`AsyncFd::accept`].
///  * Files can be opened using [`open_file`] or [`fs::OpenOptions`].
///  * Finally they can be created from any valid file descriptor using
///    [`AsyncFd::new`].
///
/// [`Future`]: std::future::Future
/// [`socket`]: crate::net::socket
/// [`open_file`]: crate::fs::open_file
/// [`fs::OpenOptions`]: crate::fs::OpenOptions
#[allow(clippy::module_name_repetitions)]
pub struct AsyncFd<D: Descriptor = File> {
    fd: RawFd,
    // NOTE: public because it's used by the crate::io::Std{in,out,error}.
    pub(crate) sq: SubmissionQueue,
    kind: PhantomData<D>,
}

// NOTE: the implementations are split over the modules to give the `Future`
// implementation types a reasonable place in the docs.

/// Operations only available on regular file descriptors.
impl AsyncFd<File> {
    /// Create a new `AsyncFd`.
    pub fn new(fd: OwnedFd, sq: SubmissionQueue) -> AsyncFd {
        // SAFETY: OwnedFd ensure that `fd` is valid.
        unsafe { AsyncFd::from_raw_fd(fd.into_raw_fd(), sq) }
    }

    /// Creates a new independently owned `AsyncFd` that shares the same
    /// underlying file descriptor as the existing `AsyncFd`.
    #[doc(alias = "dup")]
    #[doc(alias = "dup2")]
    #[doc(alias = "F_DUPFD")]
    #[doc(alias = "F_DUPFD_CLOEXEC")]
    pub fn try_clone(&self) -> io::Result<AsyncFd> {
        let fd = self.as_fd().try_clone_to_owned()?;
        Ok(AsyncFd::new(fd, self.sq.clone()))
    }
}

impl<D: Descriptor> AsyncFd<D> {
    /// Create a new `AsyncFd` from a `RawFd`.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `fd` is valid and that it's no longer used
    /// by anything other than the returned `AsyncFd`.
    ///
    /// Furthermore the caller must ensure that `fd` is either a [`File`]
    /// descriptor or a [`Direct`] descriptor.
    pub unsafe fn from_raw_fd(fd: RawFd, sq: SubmissionQueue) -> AsyncFd<D> {
        // SAFETY: caller must ensure that `fd` is valid.
        AsyncFd {
            fd,
            sq,
            kind: PhantomData,
        }
    }

    /// Returns the `RawFd` of this `AsyncFd`.
    ///
    /// The file descriptor can be a regular or direct descriptor.
    pub(crate) fn fd(&self) -> RawFd {
        self.fd
    }

    /// Returns the `SubmissionQueue` of this `AsyncFd`.
    pub(crate) const fn sq(&self) -> &SubmissionQueue {
        &self.sq
    }

    fn is_direct(&self) -> bool {
        D::is_direct()
    }
}

impl AsFd for AsyncFd<File> {
    fn as_fd(&self) -> BorrowedFd<'_> {
        // SAFETY: we're ensured that `fd` is valid.
        unsafe { BorrowedFd::borrow_raw(self.fd()) }
    }
}

impl<D: Descriptor> Unpin for AsyncFd<D> {}

impl<D: Descriptor> fmt::Debug for AsyncFd<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncFd")
            .field("fd", &self.fd())
            .field("sq", &"SubmissionQueue")
            .field("kind", &D::fmt_dbg())
            .finish()
    }
}

impl<D: Descriptor> Drop for AsyncFd<D> {
    fn drop(&mut self) {
        // Try to asynchronously close the desctiptor (if the OS supports it).
        #[cfg(any(target_os = "android", target_os = "linux"))]
        {
            let result = self.sq.inner.submit_no_completion(|submission| {
                D::close_flags(self.fd(), submission);
            });
            match result {
                Ok(()) => return,
                Err(crate::sq::QueueFull) => {
                    log::warn!("error submitting close operation for a10::AsyncFd, queue is full");
                }
            }
        }

        // Fall back to synchronously closing the descriptor.
        if let Err(err) = D::close(self.fd(), &self.sq) {
            log::warn!("error closing a10::AsyncFd: {err}");
        }
    }
}

/// What kind of descriptor is used [`File`] or [`Direct`].
#[allow(private_bounds)] // That's the point of the private module.
pub trait Descriptor: private::Descriptor {}

pub(crate) mod private {
    use std::io;
    use std::os::fd::RawFd;

    use crate::SubmissionQueue;

    pub(crate) trait Descriptor {
        fn is_direct() -> bool {
            false
        }

        /// Set any additional flags in `submission` when using the descriptor.
        fn use_flags(submission: &mut crate::sys::Submission);

        /// Set any additional flags in `submission` when creating the descriptor.
        fn create_flags(submission: &mut crate::sys::Submission);

        /// Return the equivalant of `O_CLOEXEC` for the descripor.
        fn cloexec_flag() -> libc::c_int;

        /// Return the equivalant of `IORING_ASYNC_CANCEL_FD_FIXED` for the
        /// descriptor.
        fn cancel_flag() -> u32;

        /// Debug representation of the descriptor.
        fn fmt_dbg() -> &'static str;

        /// Set flags in `submission` to close the descriptor.
        fn close_flags(fd: RawFd, submission: &mut crate::sys::Submission);

        /// Synchronously close the file descriptor.
        fn close(fd: RawFd, sq: &SubmissionQueue) -> io::Result<()>;
    }
}

/// Regular Unix file descriptors.
#[derive(Copy, Clone, Debug)]
pub enum File {}

impl Descriptor for File {}

impl private::Descriptor for File {
    fn use_flags(_: &mut crate::sys::Submission) {
        // No additional flags needed.
    }

    fn create_flags(_: &mut crate::sys::Submission) {
        // No additional flags needed.
    }

    fn cloexec_flag() -> libc::c_int {
        libc::O_CLOEXEC
    }

    fn cancel_flag() -> u32 {
        0
    }

    fn fmt_dbg() -> &'static str {
        "file descriptor"
    }

    fn close_flags(fd: RawFd, submission: &mut crate::sys::Submission) {
        crate::sys::io::close_file_fd(fd, submission);
    }

    fn close(fd: RawFd, _: &SubmissionQueue) -> io::Result<()> {
        syscall!(close(fd))?;
        Ok(())
    }
}

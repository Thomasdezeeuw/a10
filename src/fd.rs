//! Asynchronous file descriptor (fd).
//!
//! See [`AsyncFd`].

use std::marker::PhantomData;
use std::os::fd::{AsFd, BorrowedFd, IntoRawFd, OwnedFd, RawFd};
use std::{fmt, io};

use crate::{syscall, Submission, SubmissionQueue};

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

    /// Create a new `AsyncFd` from a `RawFd`.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `fd` is valid and that it's no longer used
    /// by anything other than the returned `AsyncFd`.
    pub unsafe fn from_raw_fd(fd: RawFd, sq: SubmissionQueue) -> AsyncFd {
        AsyncFd::from_raw(fd, Kind::File, sq)
    }

    /// Creates a new independently owned `AsyncFd` that shares the same
    /// underlying file descriptor as the existing `AsyncFd`.
    ///
    /// # Notes
    ///
    /// Direct descriptors can not be cloned.
    #[doc(alias = "dup")]
    #[doc(alias = "dup2")]
    #[doc(alias = "F_DUPFD")]
    #[doc(alias = "F_DUPFD_CLOEXEC")]
    pub fn try_clone(&self) -> io::Result<AsyncFd> {
        if let Kind::Direct = self.kind() {
            return Err(io::ErrorKind::Unsupported.into());
        }

        let fd = self.as_fd().try_clone_to_owned()?;
        Ok(AsyncFd::new(fd, self.sq.clone()))
    }
}

impl<D: Descriptor> AsyncFd<D> {
    pub(crate) unsafe fn from_raw(fd: RawFd, kind: Kind, sq: SubmissionQueue) -> AsyncFd<D> {
        #[cfg(any(target_os = "android", target_os = "linux"))]
        let fd = if let Kind::Direct = kind {
            fd | (1 << 31)
        } else {
            fd
        };
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
        // We use the sign bit to indicate direct descriptors, so we unset it.
        self.fd & !(1 << 31)
    }

    /// Returns the `SubmissionQueue` of this `AsyncFd`.
    pub(crate) const fn sq(&self) -> &SubmissionQueue {
        &self.sq
    }

    pub(crate) fn use_flags(&self, submission: &mut Submission) {
        #[cfg(any(target_os = "android", target_os = "linux"))]
        if let Kind::Direct = self.kind() {
            crate::sys::fd::use_direct_flags(submission)
        }
    }

    pub(crate) fn create_flags(&self, submission: &mut Submission) {
        #[cfg(any(target_os = "android", target_os = "linux"))]
        if let Kind::Direct = self.kind() {
            crate::sys::fd::create_direct_flags(submission)
        }
    }

    /// Returns the kind of descriptor.
    pub fn kind(&self) -> Kind {
        #[cfg(any(target_os = "android", target_os = "linux"))]
        if self.fd.is_negative() {
            return Kind::Direct;
        }
        Kind::File
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
            .field("kind", &self.kind())
            .finish()
    }
}

impl<D: Descriptor> Drop for AsyncFd<D> {
    fn drop(&mut self) {
        // Try to asynchronously close the desctiptor (if the OS supports it).
        #[cfg(any(target_os = "android", target_os = "linux"))]
        {
            let result = self.sq.inner.submit_no_completion(|submission| {
                crate::sys::io::close_file_fd(self.fd(), self.kind(), submission);
            });
            match result {
                Ok(()) => return,
                Err(crate::sq::QueueFull) => {
                    log::warn!("error submitting close operation for a10::AsyncFd, queue is full");
                }
            }
        }

        // Fall back to synchronously closing the descriptor.
        let result = match self.kind() {
            Kind::Direct => crate::sys::io::close_direct_fd(self.fd(), self.sq()),
            Kind::File => syscall!(close(self.fd())).map(|_| ()),
        };
        if let Err(err) = result {
            log::warn!("error closing a10::AsyncFd: {err}");
        }
    }
}

/// Kind of descriptor.
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum Kind {
    /// Regular Unix file descriptor.
    File,
    /// Direct descriptor are io_uring private file descriptor.
    ///
    /// They avoid some of the overhead associated with thread shared file
    /// tables and can be used in any io_uring request that takes a file
    /// descriptor. However they cannot be used outside of io_uring.
    Direct,
}

impl Kind {
    pub(crate) fn cloexec_flag(self) -> libc::c_int {
        if let Kind::Direct = self {
            0 // Direct descriptor always have (the equivalant of) `O_CLOEXEC` set.
        } else {
            libc::O_CLOEXEC
        }
    }
}

/// What kind of descriptor is used [`File`] or [`Direct`].
#[allow(private_bounds)] // That's the point of the private module.
pub trait Descriptor: private::Descriptor {}

pub(crate) mod private {
    pub(crate) trait Descriptor {}
}

/// Regular Unix file descriptors.
#[derive(Copy, Clone, Debug)]
pub enum File {}

impl Descriptor for File {}

impl private::Descriptor for File {}

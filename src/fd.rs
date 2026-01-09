//! Asynchronous file descriptor (fd).
//!
//! See [`AsyncFd`].

use std::os::fd::{BorrowedFd, IntoRawFd, OwnedFd, RawFd};
use std::{fmt, io};

use crate::{SubmissionQueue, syscall};

/// An open file descriptor.
///
/// All functions on `AsyncFd` are asynchronous and return a [`Future`].
///
/// `AsyncFd` comes on in of two kinds:
///  * regular file descriptor which can be used outside of io_uring.
///  * direct descriptor which can be only be used with io_uring.
///
/// Direct descriptors can be faster, but their usage is limited to them being
/// limited to io_uring operations. See [`AsyncFd::kind`] and [`Kind`] for more
/// information.
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
pub struct AsyncFd {
    fd: RawFd,
    #[cfg(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "tvos",
        target_os = "visionos",
        target_os = "watchos",
    ))]
    state: crate::sys::fd::State,
    // NOTE: public because it's used by the crate::io::Std{in,out,error}.
    pub(crate) sq: SubmissionQueue,
}

// NOTE: the implementations are split over the modules to give the `Future`
// implementation types a reasonable place in the docs.

impl AsyncFd {
    /// Create a new `AsyncFd` from an owned file descriptor.
    ///
    /// # Notes
    ///
    /// `fd` is expected to be a regular file descriptor.
    pub fn new(fd: OwnedFd, sq: SubmissionQueue) -> AsyncFd {
        // SAFETY: OwnedFd ensure that `fd` is valid.
        unsafe { AsyncFd::from_raw_fd(fd.into_raw_fd(), sq) }
    }

    /// Create a new `AsyncFd` from a raw file descriptor.
    ///
    /// # Notes
    ///
    /// `fd` is expected to be a regular file descriptor.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `fd` is valid and that it's no longer used
    /// by anything other than the returned `AsyncFd`.
    pub unsafe fn from_raw_fd(fd: RawFd, sq: SubmissionQueue) -> AsyncFd {
        // SAFETY: caller must ensure that `fd` is correct.
        unsafe { AsyncFd::from_raw(fd, Kind::File, sq) }
    }

    pub(crate) unsafe fn from_raw(fd: RawFd, kind: Kind, sq: SubmissionQueue) -> AsyncFd {
        #[cfg(any(target_os = "android", target_os = "linux"))]
        let fd = if let Kind::Direct = kind {
            fd | (1 << 31)
        } else {
            fd
        };
        AsyncFd {
            fd,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos",
            ))]
            state: crate::sys::fd::State::new(),
            sq,
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

    /// Attempts to borrow the file descriptor.
    ///
    /// If this is a direct descriptor this returns `None`. The direct
    /// descriptor can be cloned into a regular file descriptor using
    /// [`AsyncFd::to_file_descriptor`].
    pub fn as_fd(&self) -> Option<BorrowedFd<'_>> {
        #[cfg(any(target_os = "android", target_os = "linux"))]
        if let Kind::Direct = self.kind() {
            return None;
        }

        // SAFETY: we're ensured that `fd` is valid.
        unsafe { Some(BorrowedFd::borrow_raw(self.fd())) }
    }

    /// Creates a new independently owned `AsyncFd` that shares the same
    /// underlying file descriptor as the existing `AsyncFd`.
    ///
    /// # Notes
    ///
    /// Direct descriptors can not be cloned and will always return an
    /// unsupported error.
    #[doc(alias = "dup")]
    #[doc(alias = "dup2")]
    #[doc(alias = "F_DUPFD")]
    #[doc(alias = "F_DUPFD_CLOEXEC")]
    pub fn try_clone(&self) -> io::Result<AsyncFd> {
        let fd = self.as_fd().ok_or(io::ErrorKind::Unsupported)?;
        let fd = fd.try_clone_to_owned()?;
        Ok(AsyncFd::new(fd, self.sq.clone()))
    }

    /// Returns the `RawFd` of this `AsyncFd`.
    ///
    /// The file descriptor can be a regular or direct descriptor.
    pub(crate) fn fd(&self) -> RawFd {
        // The sign bit is used to indicate direct descriptors, so unset it.
        self.fd & !(1 << 31)
    }

    /// Returns the internal state of the fd.
    #[cfg(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "tvos",
        target_os = "visionos",
        target_os = "watchos",
    ))]
    pub(crate) const fn state(&self) -> &crate::sys::fd::State {
        &self.state
    }

    /// Returns the `SubmissionQueue` of this `AsyncFd`.
    pub(crate) const fn sq(&self) -> &SubmissionQueue {
        &self.sq
    }
}

impl Unpin for AsyncFd {}

impl fmt::Debug for AsyncFd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut f = f.debug_struct("AsyncFd");
        f.field("fd", &self.fd()).field("kind", &self.kind());
        #[cfg(any(
            target_os = "ios",
            target_os = "macos",
            target_os = "tvos",
            target_os = "visionos",
            target_os = "watchos",
        ))]
        f.field("state", &self.state);
        f.finish()
    }
}

impl Drop for AsyncFd {
    fn drop(&mut self) {
        // Try to asynchronously close the desctiptor (if the OS supports it).
        #[cfg(any(target_os = "android", target_os = "linux"))]
        {
            let result = self.sq.submissions().add(|submission| {
                crate::sys::io::close_file_fd(self.fd(), self.kind(), submission);
            });
            if let Ok(()) = result {
                return;
            }
        }

        // Fall back to synchronously closing the descriptor.
        let result = match self.kind() {
            #[cfg(any(target_os = "android", target_os = "linux"))]
            Kind::Direct => crate::sys::io::close_direct_fd(self.fd(), &self.sq),
            Kind::File => {
                let res = syscall!(close(self.fd())).map(|_| ());
                res
            }
        };
        if let Err(err) = result {
            log::warn!("error closing a10::AsyncFd: {err}");
        }
        #[cfg(any(
            target_os = "ios",
            target_os = "macos",
            target_os = "tvos",
            target_os = "visionos",
            target_os = "watchos",
        ))]
        // SAFETY: we're in the drop implementation.
        unsafe {
            self.state.drop(&self.sq)
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
    #[cfg(any(target_os = "android", target_os = "linux"))]
    Direct,
}

impl Kind {
    pub(crate) fn cloexec_flag(self) -> libc::c_int {
        #[cfg(any(target_os = "android", target_os = "linux"))]
        if let Kind::Direct = self {
            return 0; // Direct descriptor always have (the equivalant of) `O_CLOEXEC` set.
        }
        // We also use `O_CLOEXEC` when we technically should use
        // `SOCK_CLOEXEC`, so ensure the value is the same so it works as
        // expected.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        #[allow(clippy::items_after_statements)]
        const _: () = assert!(libc::SOCK_CLOEXEC == libc::O_CLOEXEC);
        libc::O_CLOEXEC
    }
}

//! Asynchronous file descriptor (fd).
//!
//! See [`AsyncFd`].

use std::fmt;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};

use crate::SubmissionQueue;

#[cfg(target_os = "linux")]
pub use crate::sys::fd::Direct;

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
    /// # Notes
    ///
    /// We use `ManuallyDrop` because we drop the fd using an asynchronous
    /// operation, not a blocking `close(2)` system call.
    fd: ManuallyDrop<OwnedFd>,
    sq: SubmissionQueue,
    kind: PhantomData<D>,
}

// NOTE: the implementations are split over the modules to give the `Future`
// implementation types a reasonable place in the docs.

/// Operations only available on regular file descriptors.
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
}

impl<D: Descriptor> AsyncFd<D> {
    /// Create a new `AsyncFd` from a direct descriptor.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `fd` is valid and that it's no longer
    /// used by anything other than the returned `AsyncFd`. Furthermore the
    /// caller must ensure the descriptor is file or direct descriptor depending
    /// on `D`.
    pub(crate) unsafe fn from_raw(fd: RawFd, sq: SubmissionQueue) -> AsyncFd<D> {
        AsyncFd {
            fd: ManuallyDrop::new(OwnedFd::from_raw_fd(fd)),
            sq,
            kind: PhantomData,
        }
    }

    /// Returns the `RawFd` of this `AsyncFd`.
    ///
    /// The file descriptor can be a regular or direct descriptor.
    pub(crate) fn fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// Returns the `SubmissionQueue` of this `AsyncFd`.
    pub(crate) fn sq(&self) -> &SubmissionQueue {
        &self.sq
    }
}

impl AsFd for AsyncFd<File> {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl<D: Descriptor> fmt::Debug for AsyncFd<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncFd")
            .field("fd", &self.fd())
            .field("sq", &self.sq())
            .field("kind", &D::fmt_dbg())
            .finish()
    }
}

impl<D: Descriptor> Drop for AsyncFd<D> {
    fn drop(&mut self) {
        // TODO(port).
        todo!("AsyncFd::drop")
        /*
        let result = self.sq.add_no_result(|submission| unsafe {
            submission.close(self.fd());
            submission.no_completion_event();
            D::use_flags(submission);
        });
        if let Err(err) = result {
            log::warn!("error submitting close operation for a10::AsyncFd: {err}");
            if let Err(err) = D::sync_close(self.fd()) {
                log::warn!("error closing a10::AsyncFd: {err}");
            }
        }
        */
    }
}

/// What kind of descriptor is used [`File`] or [`Direct`].
#[allow(private_bounds)] // That's the point of the private module.
pub trait Descriptor: private::Descriptor {}

pub(crate) mod private {
    /* TODO(port).
    use std::io;
    use std::os::fd::RawFd;

    use crate::io_uring::op::Submission;
    */

    pub(crate) trait Descriptor {
        /* TODO(port).
        /// Set any additional flags in `submission` when using the descriptor.
        fn use_flags(submission: &mut Submission);

        /// Set any additional flags in `submission` when creating the descriptor.
        fn create_flags(submission: &mut Submission);

        /// Return the equivalant of `O_CLOEXEC` for the descripor.
        fn cloexec_flag() -> libc::c_int;

        /// Return the equivalant of `IORING_ASYNC_CANCEL_FD_FIXED` for the
        /// descriptor.
        fn cancel_flag() -> u32;
        */

        /// Debug representation of the descriptor.
        fn fmt_dbg() -> &'static str;

        /* TODO(port).
        fn sync_close(fd: RawFd) -> io::Result<()>;
        */
    }
}

/// Regular Unix file descriptors.
#[derive(Copy, Clone, Debug)]
pub enum File {}

impl Descriptor for File {}

impl private::Descriptor for File {
    /* TODO(port).
    fn use_flags(_: &mut Submission) {
        // No additional flags needed.
    }

    fn create_flags(_: &mut Submission) {
        // No additional flags needed.
    }

    fn cloexec_flag() -> libc::c_int {
        libc::O_CLOEXEC
    }

    fn cancel_flag() -> u32 {
        0
    }
    */

    fn fmt_dbg() -> &'static str {
        "file descriptor"
    }

    /* TODO(port).
    fn sync_close(fd: RawFd) -> io::Result<()> {
        syscall!(close(fd))?;
        Ok(())
    }
    */
}

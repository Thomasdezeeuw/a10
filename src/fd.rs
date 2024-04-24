//! Asynchronous file descriptor (fd).
//!
//! See [`AsyncFd`].

use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::{fmt, io};

use crate::op::{op_future, Submission};
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
    /// We use `ManuallyDrop` because we drop the fd using io_uring, not a
    /// blocking `close(2)` system call.
    fd: ManuallyDrop<OwnedFd>,
    pub(crate) sq: SubmissionQueue,
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

    /// Convert a regular file descriptor into a direct descriptor.
    ///
    /// The file descriptor can continued to be used and the lifetimes of the
    /// file descriptor and the newly returned direct descriptor are not
    /// connected.
    ///
    /// # Notes
    ///
    /// The [`Ring`] must be configured [`with_direct_descriptors`] enabled,
    /// otherwise this will return `ENXIO`.
    ///
    /// [`Ring`]: crate::Ring
    /// [`with_direct_descriptors`]: crate::Config::with_direct_descriptors
    #[doc(alias = "IORING_OP_FILES_UPDATE")]
    #[doc(alias = "IORING_FILE_INDEX_ALLOC")]
    pub fn to_direct_descriptor<'fd>(&'fd self) -> ToDirect<'fd, File> {
        ToDirect::new(self, Box::new(self.fd()), ())
    }
}

/// Operations only available on direct descriptors.
impl AsyncFd<Direct> {
    /// Create a new `AsyncFd` from a direct descriptor.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `direct_fd` is valid and that it's no longer
    /// used by anything other than the returned `AsyncFd`. Furthermore the
    /// caller must ensure the direct descriptor is actually a direct
    /// descriptor.
    pub(crate) unsafe fn from_direct_fd(direct_fd: RawFd, sq: SubmissionQueue) -> AsyncFd<Direct> {
        AsyncFd::from_raw(direct_fd, sq)
    }

    /// Convert a direct descriptor into a regular file descriptor.
    ///
    /// The direct descriptor can continued to be used and the lifetimes of the
    /// direct descriptor and the newly returned file descriptor are not
    /// connected.
    ///
    /// # Notes
    ///
    /// Requires Linux 6.8.
    #[doc(alias = "IORING_OP_FIXED_FD_INSTALL")]
    pub const fn to_file_descriptor<'fd>(&'fd self) -> ToFd<'fd, Direct> {
        ToFd::new(self, ())
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
            D::use_flags(submission);
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
        /// Set any additional flags in `submission` when using the descriptor.
        fn use_flags(submission: &mut Submission);

        /// Set any additional flags in `submission` when creating the descriptor.
        fn create_flags(submission: &mut Submission);

        /// Return the equivalant of `O_CLOEXEC` for the descripor.
        fn cloexec_flag() -> libc::c_int;

        /// Debug representation of the descriptor.
        fn fmt_dbg() -> &'static str;
    }
}

/// Regular Unix file descriptors.
#[derive(Copy, Clone, Debug)]
pub enum File {}

impl Descriptor for File {}

impl private::Descriptor for File {
    fn use_flags(_: &mut Submission) {
        // No additional flags needed.
    }

    fn create_flags(_: &mut Submission) {
        // No additional flags needed.
    }

    fn cloexec_flag() -> libc::c_int {
        libc::O_CLOEXEC
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
    fn use_flags(submission: &mut Submission) {
        submission.use_direct_fd();
    }

    fn create_flags(submission: &mut Submission) {
        submission.create_direct_fd();
    }

    fn cloexec_flag() -> libc::c_int {
        0 // Direct descriptor always have (the equivalant of) `O_CLOEXEC` set.
    }

    fn fmt_dbg() -> &'static str {
        "direct descriptor"
    }
}

// ToFd.
op_future! {
    fn AsyncFd::to_file_descriptor -> AsyncFd<File>,
    struct ToFd<'fd> {
        // No state needed.
    },
    setup_state: _unused: (),
    setup: |submission, fd, (), ()| unsafe {
        // NOTE: flags must currently be null.
        submission.create_file_descriptor(fd.fd(), 0);
    },
    map_result: |this, (), fd| {
        let sq = this.fd.sq.clone();
        // SAFETY: the fixed fd intall operation ensures that `fd` is valid.
        let fd = unsafe { AsyncFd::from_raw_fd(fd, sq) };
        Ok(fd)
    },
}

// ToDirect.
op_future! {
    fn AsyncFd::to_direct_descriptor -> AsyncFd<Direct>,
    struct ToDirect<'fd> {
        /// The file descriptor we're changing into a direct descriptor, needs
        /// to stay in memory so the kernel can access it safely.
        direct_fd: Box<RawFd>,
    },
    setup_state: _unused: (),
    setup: |submission, fd, (direct_fd,), ()| unsafe {
        submission.create_direct_descriptor(&mut **direct_fd, 1);
    },
    map_result: |this, (direct_fd,), res| {
        debug_assert!(res == 1);
        let sq = this.fd.sq.clone();
        // SAFETY: the files update operation ensures that `direct_fd` is valid.
        let fd = unsafe { AsyncFd::from_direct_fd(*direct_fd, sq) };
        Ok(fd)
    },
}

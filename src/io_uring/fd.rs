use std::os::fd::RawFd;
use std::{io, ptr};

use crate::fd::{AsyncFd, Descriptor, File};
use crate::io_uring::{self, cq, libc, sq};
use crate::op::{fd_operation, FdOperation};
use crate::SubmissionQueue;

pub(crate) fn use_direct_flags(submission: &mut sq::Submission) {
    submission.0.flags |= libc::IOSQE_FIXED_FILE;
}

#[allow(clippy::cast_sign_loss)]
pub(crate) fn create_direct_flags(submission: &mut sq::Submission) {
    submission.0.__bindgen_anon_5 = libc::io_uring_sqe__bindgen_ty_5 {
        file_index: libc::IORING_FILE_INDEX_ALLOC as u32,
    };
}

/// Direct descriptors are io_uring private file descriptors.
///
/// They avoid some of the overhead associated with thread shared file tables
/// and can be used in any io_uring request that takes a file descriptor.
/// However they cannot be used outside of io_uring.
#[derive(Copy, Clone, Debug)]
pub enum Direct {}

impl Descriptor for Direct {}

impl crate::fd::private::Descriptor for Direct {
    fn is_direct() -> bool {
        true
    }

    fn cloexec_flag() -> libc::c_int {
        0 // Direct descriptor always have (the equivalant of) `O_CLOEXEC` set.
    }

    fn cancel_flag() -> u32 {
        libc::IORING_ASYNC_CANCEL_FD_FIXED
    }

    fn fmt_dbg() -> &'static str {
        "direct descriptor"
    }

    #[allow(clippy::cast_sign_loss)] // For fd as u32.
    fn close_flags(fd: RawFd, submission: &mut sq::Submission) {
        submission.0.opcode = libc::IORING_OP_CLOSE as u8;
        submission.0.__bindgen_anon_5 = libc::io_uring_sqe__bindgen_ty_5 {
            file_index: fd as u32,
        };
    }

    #[allow(clippy::cast_sign_loss)] // For fd as u32.
    fn close(fd: RawFd, sq: &SubmissionQueue) -> io::Result<()> {
        let shared = sq.inner.shared_data();
        let fd_updates = &[-1]; // -1 mean unregistered, i.e. closing, the fd.
        let update = libc::io_uring_files_update {
            offset: fd as u32, // The fd is also the index/offset into the set.
            resv: 0,
            fds: ptr::from_ref(fd_updates).addr() as u64,
        };
        shared.register(
            libc::IORING_REGISTER_FILES_UPDATE,
            ptr::from_ref(&update).cast(),
            1,
        )
    }
}

/// io_uring specific methods.
impl AsyncFd<File> {
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
        // The `fd` needs to be valid until the operation is complete, so we
        // need to heap allocate it so we can delay it's allocation in case of
        // an early drop.
        let fd = Box::new(self.fd());
        ToDirect(FdOperation::new(self, fd, ()))
    }
}

/// Operations only available on direct descriptors (io_uring only).
impl AsyncFd<Direct> {
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
        ToFd(FdOperation::new(self, (), ()))
    }
}

fd_operation!(
    /// [`Future`] behind [`AsyncFd::to_direct_descriptor`].
    pub struct ToDirect(ToDirectOp) -> io::Result<AsyncFd<Direct>>;

    /// [`Future`] behind [`AsyncFd::to_file_descriptor`].
    pub struct ToFd(ToFdOp) -> io::Result<AsyncFd<File>>;
);

struct ToDirectOp;

impl io_uring::FdOp for ToDirectOp {
    type Output = AsyncFd<Direct>;
    type Resources = Box<RawFd>;
    type Args = ();

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission<D: Descriptor>(
        _: &AsyncFd<D>,
        fd: &mut Self::Resources,
        (): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_FILES_UPDATE as u8;
        submission.0.fd = -1;
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            off: libc::IORING_FILE_INDEX_ALLOC as u64,
        };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: ptr::from_mut(&mut **fd).addr() as u64,
        };
        submission.0.len = 1;
    }

    fn map_ok<D: Descriptor>(
        ofd: &AsyncFd<D>,
        fd: Self::Resources,
        (_, n): cq::OpReturn,
    ) -> Self::Output {
        debug_assert!(n == 1);
        let sq = ofd.sq.clone();
        // SAFETY: the kernel ensures that `fd` is valid.
        unsafe { AsyncFd::from_raw_fd(*fd, sq) }
    }
}

struct ToFdOp;

impl io_uring::FdOp for ToFdOp {
    type Output = AsyncFd<File>;
    type Resources = ();
    type Args = ();

    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        (): &mut Self::Resources,
        (): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_FIXED_FD_INSTALL as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            // NOTE: must currently be zero.
            install_fd_flags: 0,
        };
    }

    #[allow(clippy::cast_possible_wrap)]
    fn map_ok<D: Descriptor>(
        ofd: &AsyncFd<D>,
        (): Self::Resources,
        (_, fd): cq::OpReturn,
    ) -> Self::Output {
        let sq = ofd.sq.clone();
        // SAFETY: the kernel ensures that `fd` is valid.
        unsafe { AsyncFd::from_raw_fd(fd as RawFd, sq) }
    }
}

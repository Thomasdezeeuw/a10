use std::os::fd::RawFd;
use std::{io, ptr};

use crate::io_uring::{self, cq, libc, sq};
use crate::op::{fd_operation, FdOperation};
use crate::{fd, AsyncFd};

pub(crate) fn use_direct_flags(submission: &mut sq::Submission) {
    submission.0.flags |= libc::IOSQE_FIXED_FILE;
}

#[allow(clippy::cast_sign_loss)]
pub(crate) fn create_direct_flags(submission: &mut sq::Submission) {
    submission.0.__bindgen_anon_5 = libc::io_uring_sqe__bindgen_ty_5 {
        file_index: libc::IORING_FILE_INDEX_ALLOC as u32,
    };
}

/// io_uring specific methods.
impl AsyncFd {
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
    pub fn to_direct_descriptor<'fd>(&'fd self) -> ToDirect<'fd> {
        // The `fd` needs to be valid until the operation is complete, so we
        // need to heap allocate it so we can delay it's allocation in case of
        // an early drop.
        let fd = Box::new(self.fd());
        ToDirect(FdOperation::new(self, fd, ()))
    }
}

/// Operations only available on direct descriptors (io_uring only).
impl AsyncFd {
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
    pub const fn to_file_descriptor<'fd>(&'fd self) -> ToFd<'fd> {
        ToFd(FdOperation::new(self, (), ()))
    }
}

fd_operation!(
    /// [`Future`] behind [`AsyncFd::to_direct_descriptor`].
    pub struct ToDirect(ToDirectOp) -> io::Result<AsyncFd>;

    /// [`Future`] behind [`AsyncFd::to_file_descriptor`].
    pub struct ToFd(ToFdOp) -> io::Result<AsyncFd>;
);

struct ToDirectOp;

impl io_uring::FdOp for ToDirectOp {
    type Output = AsyncFd;
    type Resources = Box<RawFd>;
    type Args = ();

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        _: &AsyncFd,
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

    fn map_ok(ofd: &AsyncFd, fd: Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        debug_assert!(n == 1);
        let sq = ofd.sq.clone();
        // SAFETY: the kernel ensures that `fd` is valid.
        unsafe { AsyncFd::from_raw(*fd, fd::Kind::Direct, sq) }
    }
}

struct ToFdOp;

impl io_uring::FdOp for ToFdOp {
    type Output = AsyncFd;
    type Resources = ();
    type Args = ();

    fn fill_submission(
        fd: &AsyncFd,
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
    fn map_ok(ofd: &AsyncFd, (): Self::Resources, (_, fd): cq::OpReturn) -> Self::Output {
        let sq = ofd.sq.clone();
        // SAFETY: the kernel ensures that `fd` is valid.
        unsafe { AsyncFd::from_raw(fd as RawFd, fd::Kind::File, sq) }
    }
}

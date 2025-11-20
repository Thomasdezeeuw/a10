use std::marker::PhantomData;
use std::os::fd::RawFd;
use std::{io, ptr};

use crate::drop_waker::DropWake;
use crate::io_uring::{self, cq, libc, sq};
use crate::op::{FdOperation, fd_operation};
use crate::{AsyncFd, SubmissionQueue, asan, fd, msan};

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
        debug_assert!(
            matches!(self.kind(), fd::Kind::File),
            "can't covert a direct descriptor to a different direct descriptor"
        );
        // The `fd` needs to be valid until the operation is complete, so we
        // need to heap allocate it so we can delay it's allocation in case of
        // an early drop.
        let fd = Box::new(self.fd());
        ToDirect(FdOperation::new(self, ((), fd), ()))
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
    pub fn to_file_descriptor<'fd>(&'fd self) -> ToFd<'fd> {
        debug_assert!(
            matches!(self.kind(), fd::Kind::Direct),
            "can't covert a file descriptor to a different file descriptor"
        );
        ToFd(FdOperation::new(self, (), ()))
    }
}

fd_operation!(
    /// [`Future`] behind [`AsyncFd::to_direct_descriptor`].
    pub struct ToDirect(ToDirectOp) -> io::Result<AsyncFd>;

    /// [`Future`] behind [`AsyncFd::to_file_descriptor`].
    pub struct ToFd(ToFdOp) -> io::Result<AsyncFd>;
);

pub(crate) struct ToDirectOp<M = ()>(PhantomData<*const M>);

impl<M: DirectFdMapper> io_uring::FdOp for ToDirectOp<M>
where
    (M, Box<RawFd>): DropWake,
{
    type Output = M::Output;
    type Resources = (M, Box<RawFd>);
    type Args = ();

    fn fill_submission(
        _: &AsyncFd,
        resources: &mut Self::Resources,
        args: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        <Self as io_uring::Op>::fill_submission(resources, args, submission);
    }

    fn map_ok(ofd: &AsyncFd, resources: Self::Resources, ret: cq::OpReturn) -> Self::Output {
        <Self as io_uring::Op>::map_ok(ofd.sq(), resources, ret)
    }
}

impl<M: DirectFdMapper> io_uring::Op for ToDirectOp<M>
where
    (M, Box<RawFd>): DropWake,
{
    type Output = M::Output;
    type Resources = (M, Box<RawFd>);
    type Args = ();

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        (_, fd): &mut Self::Resources,
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
        asan::poison_box(fd);
        submission.0.len = 1;
    }

    fn map_ok(
        sq: &SubmissionQueue,
        (mapper, dfd): Self::Resources,
        (_, n): cq::OpReturn,
    ) -> Self::Output {
        asan::unpoison_box(&dfd);
        msan::unpoison_box(&dfd);
        debug_assert!(n == 1);
        let sq = sq.clone();
        // SAFETY: the kernel ensures that `dfd` is valid.
        let dfd = unsafe { AsyncFd::from_raw(*dfd, fd::Kind::Direct, sq) };
        mapper.map(dfd)
    }
}

/// Maps a `AsyncFd` with a direct descriptor into a different type `Output`.
pub(crate) trait DirectFdMapper {
    type Output;

    fn map(self, dfd: AsyncFd) -> Self::Output;
}

impl DirectFdMapper for () {
    type Output = AsyncFd;

    fn map(self, dfd: AsyncFd) -> Self::Output {
        dfd
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

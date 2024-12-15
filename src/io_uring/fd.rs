use std::io;
use std::os::fd::RawFd;

use crate::fd::{AsyncFd, Descriptor, File};
use crate::op::{op_future, OpResult, Operation};
use crate::sys::{self, cq, libc, sq};
use crate::SubmissionQueue;

/// Direct descriptors are io_uring private file descriptors.
///
/// They avoid some of the overhead associated with thread shared file tables
/// and can be used in any io_uring request that takes a file descriptor.
/// However they cannot be used outside of io_uring.
#[derive(Copy, Clone, Debug)]
pub enum Direct {}

impl Descriptor for Direct {}

impl crate::fd::private::Descriptor for Direct {
    fn use_flags(submission: &mut sys::sq::Submission) {
        submission.use_direct_fd();
    }

    /* TODO(port).
    fn create_flags(submission: &mut Submission) {
        submission.create_direct_fd();
    }

    fn cloexec_flag() -> libc::c_int {
        0 // Direct descriptor always have (the equivalant of) `O_CLOEXEC` set.
    }
    */

    fn cancel_flag() -> u32 {
        libc::IORING_ASYNC_CANCEL_FD_FIXED
    }

    fn fmt_dbg() -> &'static str {
        "direct descriptor"
    }

    fn close(fd: RawFd) -> io::Result<()> {
        // TODO: don't leak the the fd.
        log::warn!(fd = fd; "leaking direct descriptor");
        Ok(())
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
        ToDirect(Operation::new(self, self.sq.clone(), ()))
    }
}

op_future!(
    /// [`Future`] behind [`AsyncFd::to_direct_descriptor`].
    pub struct ToDirect(ToDirectOp) -> io::Result<AsyncFd<Direct>>;
);

struct ToDirectOp;

impl sys::Op for ToDirectOp {
    type Output = AsyncFd<Direct>;
    type Resources = SubmissionQueue;
    type Args = ();

    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        _: &mut Self::Resources,
        (): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_FILES_UPDATE as u8;
        submission.0.fd = -1;
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            off: libc::IORING_FILE_INDEX_ALLOC as _,
        };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            // SAFETY: this is safe because OwnedFd has `repr(transparent)` and
            // is safe to use in FFI per it's docs.
            addr: fd.owned_fd() as *const _ as _,
        };
        submission.0.len = 1;
    }

    fn check_result<D: Descriptor>(state: &mut cq::OperationState) -> OpResult<cq::OpReturn> {
        match state {
            cq::OperationState::Single { result } => result.as_op_result(),
            cq::OperationState::Multishot { results } if results.is_empty() => {
                OpResult::Again(false)
            }
            cq::OperationState::Multishot { results } => results.remove(0).as_op_result(),
        }
    }

    fn map_ok(sq: Self::Resources, (_, dfd): cq::OpReturn) -> Self::Output {
        // SAFETY: the kernel ensure that `dfd` is valid.
        unsafe { AsyncFd::from_raw(dfd as _, sq) }
    }
}

pub(crate) fn fill_close_submission<D: Descriptor>(
    fd: &AsyncFd<D>,
    submission: &mut sys::sq::Submission,
) {
    submission.0.opcode = libc::IORING_OP_CLOSE as u8;
    submission.0.fd = fd.fd();
    D::use_flags(submission);
}

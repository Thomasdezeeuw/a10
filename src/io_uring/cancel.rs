use crate::fd::{AsyncFd, Descriptor};
use crate::io_uring::{self, cancel, cq, libc, sq};
use crate::{OperationId, SubmissionQueue};

pub(crate) fn operation(op_id: OperationId, submission: &mut sq::Submission) {
    submission.0.opcode = libc::IORING_OP_ASYNC_CANCEL as u8;
    submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: op_id as u64 };
}

pub(crate) struct CancelAllOp;

impl io_uring::FdOp for CancelAllOp {
    type Output = usize;
    type Resources = ();
    type Args = ();

    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        (): &mut Self::Resources,
        (): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_ASYNC_CANCEL as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            cancel_flags: libc::IORING_ASYNC_CANCEL_ALL
                | libc::IORING_ASYNC_CANCEL_FD
                | D::cancel_flag(),
        };
    }

    fn map_ok<D: Descriptor>(
        _: &AsyncFd<D>,
        (): Self::Resources,
        (_, n): cq::OpReturn,
    ) -> Self::Output {
        n as usize
    }
}

pub(crate) struct CancelOperationOp;

impl io_uring::Op for CancelOperationOp {
    type Output = ();
    type Resources = ();
    type Args = OperationId;

    fn fill_submission(
        (): &mut Self::Resources,
        op_id: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        cancel::operation(*op_id, submission);
    }

    fn map_ok(_: &SubmissionQueue, (): Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}

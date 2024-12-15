use crate::fd::{AsyncFd, Descriptor};
use crate::op::OpResult;
use crate::sys::{self, cq, libc, sq};
use crate::OperationId;

pub(crate) fn operation(op_id: OperationId, submission: &mut sq::Submission) {
    submission.0.opcode = libc::IORING_OP_ASYNC_CANCEL as u8;
    submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: op_id as _ };
}

pub(crate) struct CancelAll;

impl sys::Op for CancelAll {
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

    fn check_result<D: Descriptor>(state: &mut cq::OperationState) -> OpResult<cq::OpReturn> {
        match state {
            cq::OperationState::Single { result } => result.as_op_result(),
            cq::OperationState::Multishot { results } if results.is_empty() => {
                OpResult::Again(false)
            }
            cq::OperationState::Multishot { results } => results.remove(0).as_op_result(),
        }
    }

    fn map_ok((): Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        n as usize
    }
}

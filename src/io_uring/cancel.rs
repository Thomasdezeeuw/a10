use crate::fd::{AsyncFd, Descriptor};
use crate::sys::{self, cq, libc, sq};
use crate::OperationId;

pub(crate) struct CancelAllOp;

impl sys::FdOp for CancelAllOp {
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

    fn map_ok((): Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        n as usize
    }
}

pub(crate) struct CancelOperationOp;

impl sys::Op for CancelOperationOp {
    type Output = ();
    type Resources = OperationId;
    type Args = ();

    fn fill_submission(
        op_id: &mut Self::Resources,
        (): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_ASYNC_CANCEL as u8;
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: *op_id as _ };
    }

    fn map_ok(_: Self::Resources, (_, _): cq::OpReturn) -> Self::Output {
        ()
    }
}

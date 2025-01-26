use crate::fd::{AsyncFd, Descriptor};
use crate::kqueue::{self, cancel};
use crate::op::OpResult;
use crate::{OperationId, SubmissionQueue};

pub(crate) fn operation(op_id: OperationId, submission: &mut kqueue::Event) {
    // TODO(port): look into removing the operation, we'll likely need more info
    // on the kind of operation though.
    // We can fake a user space event for completion.
    todo!("cancel::operation");
}

pub(crate) struct CancelAllOp;

impl kqueue::FdOp for CancelAllOp {
    type Output = usize;
    type Resources = ();
    type Args = ();
    type OperationOutput = usize;

    fn fill_submission<D: Descriptor>(fd: &AsyncFd<D>, kevent: &mut kqueue::Event) {
        // TODO(port): implement.
        todo!("CancelAllOp::fill_submission");
    }

    fn check_result<D: Descriptor>(
        fd: &AsyncFd<D>,
        _: &mut Self::Resources,
        _: &mut Self::Args,
    ) -> OpResult<Self::OperationOutput> {
        // TODO(port): implement.
        todo!("CancelAllOp::check_result");
    }

    fn map_ok(_: Self::Resources, n: Self::OperationOutput) -> Self::Output {
        // TODO(port): implement.
        todo!("CancelAllOp::map_ok");
    }
}

pub(crate) struct CancelOperationOp;

impl kqueue::Op for CancelOperationOp {
    type Output = ();
    type Resources = ();
    type Args = OperationId;
    type OperationOutput = usize;

    fn fill_submission(kevent: &mut kqueue::Event) {
        // TODO(port): implement.
        todo!("CancelOperationOp::fill_submission");
    }

    fn check_result(
        _: &mut Self::Resources,
        _: &mut Self::Args,
    ) -> OpResult<Self::OperationOutput> {
        // TODO(port): implement.
        todo!("CancelOperationOp::check_result");
    }

    fn map_ok(
        sq: &crate::SubmissionQueue,
        _: Self::Resources,
        n: Self::OperationOutput,
    ) -> Self::Output {
        // TODO(port): implement.
        todo!("CancelOperationOp::map_ok");
    }
}

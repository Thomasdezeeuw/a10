use crate::fd::{AsyncFd, Descriptor};
use crate::op::OpResult;
use crate::sys::{self, cancel};
use crate::{OperationId, SubmissionQueue};

// TODO: implement cancelation for kqueue.

pub(crate) fn operation(_: OperationId, _: &mut sys::Event) {
    unimplemented!("kqueue: cancel::operation")
}

pub(crate) struct CancelAllOp;

impl sys::FdOp for CancelAllOp {
    type Output = usize;
    type Resources = ();
    type Args = ();
    type OperationOutput = usize;

    fn fill_submission<D: Descriptor>(_: &AsyncFd<D>, _: &mut sys::Event) {
        unimplemented!("kqueue: CancelAllOp::fill_submission")
    }

    fn check_result<D: Descriptor>(
        _: &AsyncFd<D>,
        _: &mut Self::Resources,
        _: &mut Self::Args,
    ) -> OpResult<Self::OperationOutput> {
        unimplemented!("kqueue: CancelAllOp::check_result")
    }

    fn map_ok(_: Self::Resources, _: Self::OperationOutput) -> Self::Output {
        unimplemented!("kqueue: CancelAllOp::map_ok")
    }
}

pub(crate) struct CancelOperationOp;

impl sys::Op for CancelAllOp {
    type Output = ();
    type Resources = OperationId;
    type Args = ();
    type OperationOutput = ();

    fn fill_submission(_: &mut sys::Event) {
        unimplemented!("kqueue: CancelOperationOp::fill_submission")
    }

    fn check_result(
        _: &mut Self::Resources,
        _: &mut Self::Args,
    ) -> OpResult<Self::OperationOutput> {
        unimplemented!("kqueue: CancelOperationOp::check_result")
    }

    fn map_ok(_: &SubmissionQueue, _: Self::Resources, _: Self::OperationOutput) -> Self::Output {
        unimplemented!("kqueue: CancelOperationOp::map_ok")
    }
}

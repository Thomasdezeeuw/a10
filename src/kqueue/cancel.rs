use crate::fd::{AsyncFd, Descriptor};
use crate::op::OpResult;
use crate::{sys, OperationId};

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

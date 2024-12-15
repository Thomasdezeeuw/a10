use crate::fd::{AsyncFd, Descriptor};
use crate::op::OpResult;
use crate::sys::{self, cancel};
use crate::{OperationId, SubmissionQueue};

// TODO: implement file system operations for kqueue.

pub(crate) struct OpenOp<D>(PhantomData<*const D>);

impl<D: Descriptor> sys::Op for OpenOp<D> {
    type Output = AsyncFd<D>;
    type Resources = CString; // path.
    type Args = (libc::c_int, libc::mode_t); // flags, mode.
    type OperationOutput = ();

    fn fill_submission(_: &mut sys::Event) {
        unimplemented!("kqueue: OpenOp::fill_submission")
    }

    fn check_result(
        _: &mut Self::Resources,
        _: &mut Self::Args,
    ) -> OpResult<Self::OperationOutput> {
        unimplemented!("kqueue: OpenOp::check_result")
    }

    fn map_ok(_: &SubmissionQueue, _: Self::Resources, _: Self::OperationOutput) -> Self::Output {
        unimplemented!("kqueue: OpenOp::map_ok")
    }
}

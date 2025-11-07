use crate::kqueue::{self, cancel, cq, sq};
use crate::op::OpResult;
use crate::{fd, AsyncFd, OperationId, SubmissionQueue};

pub(crate) fn operation(op_id: OperationId, submission: &mut kqueue::Event) {
    // FIXME: we need a fd here, can't do it with the operation id only.

    todo!("cancel::operation")
}

pub(crate) struct CancelAllOp;

impl kqueue::FdOp for CancelAllOp {
    type Output = usize;
    type Resources = ();
    type Args = ();
    type OperationOutput = ();

    fn fill_submission(fd: &AsyncFd, kevent: &mut kqueue::Event) {
        // FIXME: need two events here, one for read one for writing.
        // FIXME: what happens to the canceled operation(s) -- are they awoken?

        /*
        kevent.0.ident = fd.fd() as _;
        kevent.0.filter = libc::EVFILT_READ;
        kevent.0.flags = libc::EV_DELETE;

        kevent.0.ident = fd.fd() as _;
        kevent.0.filter = libc::EVFILT_WRITE;
        kevent.0.flags = libc::EV_DELETE;
        */

        todo!("CancelAllOp")
    }

    fn check_result(
        fd: &AsyncFd,
        (): &mut Self::Resources,
        (): &mut Self::Args,
    ) -> OpResult<Self::OperationOutput> {
        OpResult::Ok(())
    }

    fn map_ok((): Self::Resources, (): Self::OperationOutput) -> Self::Output {
        0 // Can't determine the number of operations we cancelled.
    }
}

pub(crate) struct CancelOperationOp;

impl kqueue::Op for CancelOperationOp {
    type Output = ();
    type Resources = ();
    type Args = OperationId;
    type OperationOutput = ();

    fn fill_submission(op_id: &mut Self::Args, submission: &mut kqueue::Event) {
        cancel::operation(*op_id, submission);
    }

    fn check_result(
        (): &mut Self::Resources,
        _: &mut Self::Args,
    ) -> OpResult<Self::OperationOutput> {
        OpResult::Ok(())
    }

    fn map_ok(_: &SubmissionQueue, (): Self::Resources, (): Self::OperationOutput) -> Self::Output {
        ()
    }
}

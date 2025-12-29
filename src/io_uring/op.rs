use crate::AsyncFd;
use crate::drop_waker::DropWake;
use crate::io_uring::{cq, sq};
use crate::op::OpResult;

pub(crate) trait Op {
    type Output;
    type Resources: DropWake;
    type Args;

    fn fill_submission(
        resources: &mut Self::Resources,
        args: &mut Self::Args,
        submission: &mut sq::Submission,
    );

    fn map_ok(
        sq: &crate::SubmissionQueue,
        resources: Self::Resources,
        op_output: cq::OpReturn,
    ) -> Self::Output;
}

impl<T: Op> crate::op::Op for T {
    type Output = T::Output;
    type Resources = T::Resources;
    type Args = T::Args;
    type Submission = sq::Submission;
    type OperationState = cq::OperationState;
    type OperationOutput = cq::OpReturn;

    fn fill_submission(
        resources: &mut Self::Resources,
        args: &mut Self::Args,
        submission: &mut Self::Submission,
    ) {
        T::fill_submission(resources, args, submission);
    }

    fn check_result(
        _: &mut Self::Resources,
        _: &mut Self::Args,
        state: &mut Self::OperationState,
    ) -> OpResult<Self::OperationOutput> {
        match state {
            cq::OperationState::Single { result } => result.as_op_return(),
            cq::OperationState::Multishot { results } if results.is_empty() => {
                OpResult::Again(false)
            }
            cq::OperationState::Multishot { results } => results.remove(0).as_op_return(),
        }
    }

    fn map_ok(
        sq: &crate::SubmissionQueue,
        resources: Self::Resources,
        op_output: Self::OperationOutput,
    ) -> Self::Output {
        T::map_ok(sq, resources, op_output)
    }
}

pub(crate) trait FdOp {
    type Output;
    type Resources: DropWake;
    type Args;

    fn fill_submission(
        fd: &AsyncFd,
        resources: &mut Self::Resources,
        args: &mut Self::Args,
        submission: &mut sq::Submission,
    );

    fn map_ok(fd: &AsyncFd, resources: Self::Resources, op_output: cq::OpReturn) -> Self::Output;
}

impl<T: FdOp> crate::op::FdOp for T {
    type Output = T::Output;
    type Resources = T::Resources;
    type Args = T::Args;
    type Submission = sq::Submission;
    type OperationState = cq::OperationState;
    type OperationOutput = cq::OpReturn;

    fn fill_submission(
        fd: &AsyncFd,
        resources: &mut Self::Resources,
        args: &mut Self::Args,
        submission: &mut Self::Submission,
    ) {
        T::fill_submission(fd, resources, args, submission);
    }

    fn check_result(
        _: &AsyncFd,
        _: &mut Self::Resources,
        _: &mut Self::Args,
        state: &mut Self::OperationState,
    ) -> OpResult<Self::OperationOutput> {
        match state {
            cq::OperationState::Single { result } => result.as_op_return(),
            cq::OperationState::Multishot { results } if results.is_empty() => {
                OpResult::Again(false)
            }
            cq::OperationState::Multishot { results } => results.remove(0).as_op_return(),
        }
    }

    fn map_ok(
        fd: &AsyncFd,
        resources: Self::Resources,
        op_output: Self::OperationOutput,
    ) -> Self::Output {
        T::map_ok(fd, resources, op_output)
    }
}

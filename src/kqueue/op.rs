use std::marker::PhantomData;
use std::mem::replace;
use std::task::{self, Poll};
use std::{fmt, io};

use crate::op::OpState;
use crate::SubmissionQueue;

/// For (not fd) operations we keep a simple state as the operation is
/// synchronous.
#[derive(Debug)]
pub(crate) enum State<R, A> {
    /// Operation has not started yet.
    NotStarted { resources: R, args: A },
    /// Last state where the operation was fully cleaned up.
    Complete,
}

impl<R, A> OpState for State<R, A> {
    type Resources = R;
    type Args = A;

    fn new(resources: Self::Resources, args: Self::Args) -> Self {
        State::NotStarted { resources, args }
    }
}

pub(crate) trait Op {
    type Output;
    type Resources;
    type Args;

    /// Run the synchronous operation.
    fn run(
        sq: &SubmissionQueue,
        resources: Self::Resources,
        args: Self::Args,
    ) -> io::Result<Self::Output>;
}

impl<T: Op> crate::op::Op for T {
    type Output = io::Result<T::Output>;
    type Resources = T::Resources;
    type Args = T::Args;
    type State = State<T::Resources, T::Args>;

    fn poll(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        sq: &SubmissionQueue,
    ) -> Poll<Self::Output> {
        match replace(state, State::Complete) {
            State::NotStarted { resources, args } => Poll::Ready(T::run(sq, resources, args)),
            // Shouldn't be reachable, but if the Future is used incorrectly it
            // can be.
            State::Complete => panic!("polled Future after completion"),
        }
    }
}

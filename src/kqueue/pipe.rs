use std::io;
use std::os::fd::RawFd;

use crate::kqueue::op::DirectOp;
use crate::pipe::{PipeFlag, sync_pipe2};
use crate::{AsyncFd, SubmissionQueue, fd};

pub(crate) struct PipeOp;

impl DirectOp for PipeOp {
    type Output = [AsyncFd; 2];
    type Resources = ([RawFd; 2], fd::Kind);
    type Args = PipeFlag;

    fn run(
        sq: &SubmissionQueue,
        (_, fd::Kind::File): Self::Resources,
        flags: Self::Args,
    ) -> io::Result<Self::Output> {
        sync_pipe2(flags).map(|[r, s]| [AsyncFd::new(r, sq.clone()), AsyncFd::new(s, sq.clone())])
    }
}

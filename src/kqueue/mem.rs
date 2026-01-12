use std::io;

use crate::kqueue::op::DirectOp;
use crate::mem::AdviseFlag;
use crate::{SubmissionQueue, syscall};

pub(crate) struct AdviseOp;

impl DirectOp for AdviseOp {
    type Output = ();
    type Resources = ();
    type Args = (*mut (), u32, AdviseFlag); // address, length, advice.

    fn run(
        _: &SubmissionQueue,
        (): Self::Resources,
        (address, length, advice): Self::Args,
    ) -> io::Result<Self::Output> {
        syscall!(madvise(address.cast(), length as _, advice.0 as _))?;
        Ok(())
    }
}

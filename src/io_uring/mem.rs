use crate::io_uring::op::{Op, OpReturn};
use crate::io_uring::{self, cq, libc, sq};
use crate::mem::AdviseFlag;
use crate::SubmissionQueue;

pub(crate) struct AdviseOp;

impl Op for AdviseOp {
    type Output = ();
    type Resources = ();
    type Args = (*mut (), u32, AdviseFlag); // address, length, advice.

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        (): &mut Self::Resources,
        (address, length, advice): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_MADVISE as u8;
        submission.0.fd = -1;
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: address.addr() as u64,
        };
        submission.0.len = *length;
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            fadvise_advice: advice.0,
        };
    }

    fn map_ok(_: &SubmissionQueue, (): Self::Resources, (_, n): OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}

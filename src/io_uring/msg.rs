use std::os::fd::AsRawFd;

use crate::io_uring::{cq, libc, sq};
use crate::msg::Message;
use crate::{OperationId, SubmissionQueue};

pub(crate) fn next(state: &mut cq::OperationState) -> Option<Message> {
    let result = match state {
        cq::OperationState::Single { result } => *result,
        cq::OperationState::Multishot { results } if results.is_empty() => return None,
        cq::OperationState::Multishot { results } => results.remove(0),
    };
    Some(result.as_msg())
}

pub(crate) fn send(
    sq: &SubmissionQueue,
    op_id: OperationId,
    data: Message,
    submission: &mut sq::Submission,
) {
    submission.0.opcode = libc::IORING_OP_MSG_RING as u8;
    submission.0.fd = sq.inner.shared_data().rfd.as_raw_fd();
    submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
        addr: u64::from(libc::IORING_MSG_DATA),
    };
    submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: op_id as u64 };
    submission.0.len = data;
    submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 { msg_ring_flags: 0 };
}

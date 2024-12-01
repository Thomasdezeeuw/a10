use std::marker::PhantomData;

use crate::fd::{AsyncFd, Descriptor};
use crate::io::{BufId, BufMut, BufMutSlice};
use crate::op::OpResult;
use crate::sys::{self, cq, libc, sq};

// Re-export so we don't have to worry about import `std::io` and `crate::io`.
pub(crate) use std::io::*;

pub(crate) use crate::unix::{IoMutSlice, IoSlice};

pub(crate) struct Read<B>(PhantomData<*const B>);

impl<B: BufMut> sys::Op for Read<B> {
    type Output = B;
    type Resources = B;
    type Args = u64; // Offset.

    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        buf: &mut Self::Resources,
        offset: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        let (ptr, len) = unsafe { buf.parts_mut() };
        submission.0.opcode = libc::IORING_OP_READ as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: *offset };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: ptr as _ };
        submission.0.len = len;
        if let Some(buf_group) = buf.buffer_group() {
            submission.set_buffer_select(buf_group.0);
        }
    }

    fn check_result<D: Descriptor>(state: &mut cq::OperationState) -> OpResult<cq::OpReturn> {
        match state {
            cq::OperationState::Single { result } => result.as_op_result(),
            cq::OperationState::Multishot { results } if results.is_empty() => {
                OpResult::Again(false)
            }
            cq::OperationState::Multishot { results } => results.remove(0).as_op_result(),
        }
    }

    fn map_ok(mut buf: Self::Resources, (buf_id, n): cq::OpReturn) -> Self::Output {
        // SAFETY: kernel just initialised the bytes for us.
        unsafe {
            buf.buffer_init(BufId(buf_id), n);
        };
        buf
    }
}

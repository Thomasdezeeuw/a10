use std::marker::PhantomData;

use crate::fd::{AsyncFd, Descriptor};
use crate::io::BufMut;
use crate::op::OpResult;
use crate::sys::cq::CompletionState;
use crate::sys::{self, libc};

// Re-export so we don't have to worry about import `std::io` and `crate::io`.
pub(crate) use std::io::*;

pub(crate) struct Read<B>(PhantomData<*const B>);

// TODO(port): create and use short version.
impl<B: BufMut> crate::op::Op for Read<B> {
    type Output = B;
    type Resources = B;
    type Args = u64; // Offset.
    type Submission = sys::sq::Submission;
    type CompletionState = CompletionState;
    /// Buffer index and operation output.
    type OperationOutput = (u16, u32);

    /// Fill a submission for the operation.
    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        buf: &mut Self::Resources,
        offset: &mut Self::Args,
        submission: &mut Self::Submission,
    ) {
        let (ptr, len) = unsafe { buf.parts_mut() };
        submission.0.opcode = libc::IORING_OP_READ as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: *offset };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: ptr as _ };
        submission.0.len = len;
        /* TODO(port): add back support for read buffer pool.
        if let Some(buf_group) = buf.buffer_group() {
            submission.set_buffer_select(buf_group.0);
        }
        */
    }

    /// Check the result of an operation based on the `QueuedOperation.state`
    /// (`Self::CompletionState`).
    fn check_result<D: Descriptor>(
        fd: &AsyncFd<D>,
        buf: &mut Self::Resources,
        offset: &mut Self::Args,
        state: &mut Self::CompletionState,
    ) -> OpResult<Self::OperationOutput> {
        match state {
            CompletionState::Single { result } => return result.as_op_result(),
            CompletionState::Multishot { results } => {
                if !results.is_empty() {
                    return results.remove(0).as_op_result();
                }
            }
        }
        OpResult::Again
    }

    fn map_ok(mut buf: Self::Resources, (buf_idx, n): Self::OperationOutput) -> Self::Output {
        // SAFETY: kernel just initialised the bytes for us.
        unsafe {
            /* TODO(port): add back support for read buffer pool.
            buf.buffer_init(BufIdx(buf_idx), n as u32)
            */
            buf.set_init(n as usize)
        };
        buf
    }
}

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

pub(crate) struct ReadVectored<B, const N: usize>(PhantomData<*const B>);

impl<B: BufMutSlice<N>, const N: usize> sys::Op for ReadVectored<B, N> {
    type Output = B;
    /// `IoMutSlice` holds the buffer references used by the kernel.
    /// NOTE: we only need these in the submission, we don't have to keep around
    /// during the operation. Because of this we don't heap allocate it like we
    /// for other operations. This leaves a small duration between the
    /// submission of the entry and the submission being read by the kernel in
    /// which this future could be dropped and the kernel will read memory we
    /// don't own. However because we wake the kernel after submitting the
    /// timeout entry it's not really worth to heap allocation.
    type Resources = (B, [crate::io::IoMutSlice; N]);
    type Args = u64; // Offset.

    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        (_, iovecs): &mut Self::Resources,
        offset: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_READV as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: *offset };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: iovecs.as_ptr() as _,
        };
        submission.0.len = iovecs.len() as u32;
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

    fn map_ok((mut bufs, _): Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        // SAFETY: kernel just initialised the buffers for us.
        unsafe { bufs.set_init(n as usize) };
        bufs
    }
}

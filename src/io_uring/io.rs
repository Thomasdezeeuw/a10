use std::marker::PhantomData;

use crate::io::{Buf, BufId, BufMut, BufMutSlice, BufSlice};
use crate::io_uring::op::OpReturn;
use crate::io_uring::{self, cq, libc, sq};
use crate::{asan, fd, msan, AsyncFd, SubmissionQueue};

pub(crate) use crate::unix::{IoMutSlice, IoSlice};
pub(crate) use std::io::*; // So we don't have to worry about importing `std::io`.

pub(crate) struct ReadOp<B>(PhantomData<*const B>);

impl<B: BufMut> io_uring::op::FdOp for ReadOp<B> {
    type Output = B;
    type Resources = B;
    type Args = u64; // Offset.

    fn fill_submission(
        fd: &AsyncFd,
        buf: &mut Self::Resources,
        offset: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        let (ptr, len) = unsafe { buf.parts_mut() };
        submission.0.opcode = libc::IORING_OP_READ as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: *offset };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: ptr.addr() as u64,
        };
        asan::poison_region(ptr.cast(), len as usize);
        submission.0.len = len;
        if let Some(buf_group) = buf.buffer_group() {
            submission.0.__bindgen_anon_4.buf_group = buf_group.0;
            submission.0.flags |= libc::IOSQE_BUFFER_SELECT;
        }
    }

    fn map_ok(_: &AsyncFd, mut buf: Self::Resources, (buf_id, n): OpReturn) -> Self::Output {
        let (ptr, len) = unsafe { buf.parts_mut() };
        asan::unpoison_region(ptr.cast(), len as usize);
        msan::unpoison_region(ptr.cast(), len as usize);
        // SAFETY: kernel just initialised the bytes for us.
        unsafe {
            buf.buffer_init(BufId(buf_id), n);
        };
        buf
    }
}

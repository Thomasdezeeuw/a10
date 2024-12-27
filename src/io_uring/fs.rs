use std::ffi::CString;
use std::marker::PhantomData;

use crate::fd::{AsyncFd, Descriptor};
use crate::fs::{Metadata, SyncDataFlag, METADATA_FLAGS};
use crate::sys::{self, cq, libc, sq};
use crate::SubmissionQueue;

pub(crate) struct OpenOp<D>(PhantomData<*const D>);

impl<D: Descriptor> sys::Op for OpenOp<D> {
    type Output = AsyncFd<D>;
    type Resources = CString; // path.
    type Args = (libc::c_int, libc::mode_t); // flags, mode.

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        path: &mut Self::Resources,
        (flags, mode): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_OPENAT as u8;
        submission.0.fd = libc::AT_FDCWD;
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: path.as_ptr() as _,
        };
        submission.0.len = *mode;
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            open_flags: *flags as _,
        };
        D::create_flags(submission);
    }

    fn map_ok(sq: &SubmissionQueue, _: Self::Resources, (_, fd): cq::OpReturn) -> Self::Output {
        // SAFETY: kernel ensures that `fd` is valid.
        unsafe { AsyncFd::from_raw(fd as _, sq.clone()) }
    }
}

pub(crate) struct SyncDataOp;

impl sys::FdOp for SyncDataOp {
    type Output = ();
    type Resources = ();
    type Args = SyncDataFlag;

    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        (): &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_FSYNC as u8;
        submission.0.fd = fd.fd();
        let fsync_flags = match flags {
            SyncDataFlag::All => 0,
            SyncDataFlag::Data => libc::IORING_FSYNC_DATASYNC,
        };
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 { fsync_flags };
    }

    fn map_ok((): Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}

pub(crate) struct StatOp;

impl sys::FdOp for StatOp {
    type Output = Metadata;
    type Resources = Box<Metadata>;
    type Args = ();

    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        metadata: &mut Self::Resources,
        (): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_STATX as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            // SAFETY: this is safe because `Metadata` is transparent.
            off: metadata as *mut _ as _,
        };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: "\0".as_ptr() as _, // Not using a path.
        };
        submission.0.len = METADATA_FLAGS;
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            statx_flags: libc::AT_EMPTY_PATH as _,
        };
    }

    fn map_ok(metadata: Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        debug_assert!(n == 0);
        debug_assert!(metadata.mask() & METADATA_FLAGS == METADATA_FLAGS);
        *metadata
    }
}

pub(crate) struct AdviseOp;

impl sys::FdOp for AdviseOp {
    type Output = ();
    type Resources = ();
    type Args = (u64, u32, libc::c_int); // offset, length, advice

    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        (): &mut Self::Resources,
        (offset, length, advice): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_FADVISE as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: *offset };
        submission.0.len = *length;
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            fadvise_advice: *advice as _,
        };
    }

    fn map_ok((): Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}

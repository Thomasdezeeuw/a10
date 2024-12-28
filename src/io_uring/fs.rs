use std::cell::UnsafeCell;
use std::ffi::CString;
use std::marker::PhantomData;
use std::path::PathBuf;

use crate::fd::{AsyncFd, Descriptor};
use crate::fs::{path_from_cstring, Metadata, RemoveFlag, SyncDataFlag, METADATA_FLAGS};
use crate::op::OpExtract;
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

impl<D: Descriptor> OpExtract for OpenOp<D> {
    type ExtractOutput = PathBuf;

    fn map_ok_extract(
        _: &SubmissionQueue,
        path: Self::Resources,
        (_, n): Self::OperationOutput,
    ) -> Self::ExtractOutput {
        debug_assert!(n == 0);
        path_from_cstring(path)
    }
}

pub(crate) struct CreateDirOp;

impl sys::Op for CreateDirOp {
    type Output = ();
    type Resources = CString; // path.
    type Args = ();

    fn fill_submission(
        path: &mut Self::Resources,
        (): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_MKDIRAT as u8;
        submission.0.fd = libc::AT_FDCWD;
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: path.as_ptr() as _,
        };
        submission.0.len = 0o777; // Same as used by the standard library.
    }

    fn map_ok(_: &SubmissionQueue, _: Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}

impl OpExtract for CreateDirOp {
    type ExtractOutput = PathBuf;

    fn map_ok_extract(
        _: &SubmissionQueue,
        path: Self::Resources,
        (_, n): Self::OperationOutput,
    ) -> Self::ExtractOutput {
        debug_assert!(n == 0);
        path_from_cstring(path)
    }
}

pub(crate) struct RenameOp;

impl sys::Op for RenameOp {
    type Output = ();
    type Resources = (CString, CString); // from path, to path
    type Args = ();

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        (from, to): &mut Self::Resources,
        (): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_RENAMEAT as u8;
        submission.0.fd = libc::AT_FDCWD;
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            off: to.as_ptr() as _,
        };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: from.as_ptr() as _,
        };
        submission.0.len = libc::AT_FDCWD as _;
    }

    fn map_ok(_: &SubmissionQueue, _: Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}

impl OpExtract for RenameOp {
    type ExtractOutput = (PathBuf, PathBuf);

    fn map_ok_extract(
        _: &SubmissionQueue,
        (from, to): Self::Resources,
        (_, n): Self::OperationOutput,
    ) -> Self::ExtractOutput {
        debug_assert!(n == 0);
        (path_from_cstring(from), path_from_cstring(to))
    }
}

pub(crate) struct DeleteOp;

impl sys::Op for DeleteOp {
    type Output = ();
    type Resources = CString; // path
    type Args = RemoveFlag;

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        path: &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_UNLINKAT as u8;
        submission.0.fd = libc::AT_FDCWD;
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: path.as_ptr() as _,
        };
        let flags = match flags {
            RemoveFlag::File => 0,
            RemoveFlag::Directory => libc::AT_REMOVEDIR,
        };
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            unlink_flags: flags as _,
        };
    }

    fn map_ok(_: &SubmissionQueue, _: Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}

impl OpExtract for DeleteOp {
    type ExtractOutput = PathBuf;

    fn map_ok_extract(
        _: &SubmissionQueue,
        path: Self::Resources,
        (_, n): Self::OperationOutput,
    ) -> Self::ExtractOutput {
        debug_assert!(n == 0);
        path_from_cstring(path)
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
    type Resources = Box<UnsafeCell<Metadata>>;
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
        let metadata = metadata.into_inner();
        debug_assert!(metadata.mask() & METADATA_FLAGS == METADATA_FLAGS);
        metadata
    }
}

pub(crate) struct AdviseOp;

impl sys::FdOp for AdviseOp {
    type Output = ();
    type Resources = ();
    type Args = (u64, u32, libc::c_int); // offset, length, advice

    #[allow(clippy::cast_sign_loss)]
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

pub(crate) struct AllocateOp;

impl sys::FdOp for AllocateOp {
    type Output = ();
    type Resources = ();
    type Args = (u64, u32, libc::c_int); // offset, length, mode

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        (): &mut Self::Resources,
        (offset, length, mode): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_FALLOCATE as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: *offset };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: (*length).into(),
        };
        submission.0.len = *mode as u32;
    }

    fn map_ok((): Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}

pub(crate) struct TruncateOp;

impl sys::FdOp for TruncateOp {
    type Output = ();
    type Resources = ();
    type Args = u64; // length

    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        (): &mut Self::Resources,
        length: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_FTRUNCATE as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: *length };
    }

    fn map_ok((): Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}

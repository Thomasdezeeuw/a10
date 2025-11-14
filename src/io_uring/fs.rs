use std::ffi::CString;
use std::os::fd::RawFd;
use std::path::PathBuf;
use std::ptr;

use crate::fs::{
    AdviseFlag, AllocateFlag, METADATA_FLAGS, Metadata, RemoveFlag, SyncDataFlag, path_from_cstring,
};
use crate::io_uring::{self, cq, libc, sq};
use crate::op::OpExtract;
use crate::{AsyncFd, SubmissionQueue, asan, fd, msan};

pub(crate) struct OpenOp;

impl io_uring::Op for OpenOp {
    type Output = AsyncFd;
    type Resources = (CString, fd::Kind); // path.
    type Args = (libc::c_int, libc::mode_t); // flags, mode.

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        (path, fd_kind): &mut Self::Resources,
        (flags, mode): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_OPENAT as u8;
        submission.0.fd = libc::AT_FDCWD;
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: path.as_ptr().addr() as u64,
        };
        asan::poison_cstring(path);
        submission.0.len = *mode;
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            open_flags: *flags as u32,
        };
        if let fd::Kind::Direct = fd_kind {
            io_uring::fd::create_direct_flags(submission);
        }
    }

    #[allow(clippy::cast_possible_wrap)]
    fn map_ok(
        sq: &SubmissionQueue,
        (path, fd_kind): Self::Resources,
        (_, fd): cq::OpReturn,
    ) -> Self::Output {
        asan::unpoison_cstring(&path);
        // SAFETY: kernel ensures that `fd` is valid.
        unsafe { AsyncFd::from_raw(fd as RawFd, fd_kind, sq.clone()) }
    }
}

impl OpExtract for OpenOp {
    type ExtractOutput = (AsyncFd, PathBuf);

    #[allow(clippy::cast_possible_wrap)]
    fn map_ok_extract(
        sq: &SubmissionQueue,
        (path, fd_kind): Self::Resources,
        (_, fd): Self::OperationOutput,
    ) -> Self::ExtractOutput {
        asan::unpoison_cstring(&path);
        // SAFETY: kernel ensures that `fd` is valid.
        let fd = unsafe { AsyncFd::from_raw(fd as RawFd, fd_kind, sq.clone()) };
        let path = path_from_cstring(path);
        (fd, path)
    }
}

pub(crate) struct CreateDirOp;

impl io_uring::Op for CreateDirOp {
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
            addr: path.as_ptr().addr() as u64,
        };
        asan::poison_cstring(path);
        submission.0.len = 0o777; // Same as used by the standard library.
    }

    fn map_ok(_: &SubmissionQueue, path: Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        asan::unpoison_cstring(&path);
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
        asan::unpoison_cstring(&path);
        debug_assert!(n == 0);
        path_from_cstring(path)
    }
}

pub(crate) struct RenameOp;

impl io_uring::Op for RenameOp {
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
            off: to.as_ptr().addr() as u64,
        };
        asan::poison_cstring(to);
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: from.as_ptr().addr() as u64,
        };
        asan::poison_cstring(from);
        submission.0.len = libc::AT_FDCWD as u32;
    }

    fn map_ok(
        _: &SubmissionQueue,
        (from, to): Self::Resources,
        (_, n): cq::OpReturn,
    ) -> Self::Output {
        asan::unpoison_cstring(&from);
        asan::unpoison_cstring(&to);
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
        asan::unpoison_cstring(&from);
        asan::unpoison_cstring(&to);
        debug_assert!(n == 0);
        (path_from_cstring(from), path_from_cstring(to))
    }
}

pub(crate) struct DeleteOp;

impl io_uring::Op for DeleteOp {
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
            addr: path.as_ptr().addr() as u64,
        };
        asan::poison_cstring(path);
        let flags = match flags {
            RemoveFlag::File => 0,
            RemoveFlag::Directory => libc::AT_REMOVEDIR,
        };
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            unlink_flags: flags as u32,
        };
    }

    fn map_ok(_: &SubmissionQueue, path: Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        asan::unpoison_cstring(&path);
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
        asan::unpoison_cstring(&path);
        debug_assert!(n == 0);
        path_from_cstring(path)
    }
}

pub(crate) struct SyncDataOp;

impl io_uring::FdOp for SyncDataOp {
    type Output = ();
    type Resources = ();
    type Args = SyncDataFlag;

    fn fill_submission(
        fd: &AsyncFd,
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

    fn map_ok(_: &AsyncFd, (): Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}

pub(crate) struct StatOp;

impl io_uring::FdOp for StatOp {
    type Output = Metadata;
    type Resources = Box<Metadata>;
    type Args = ();

    fn fill_submission(
        fd: &AsyncFd,
        metadata: &mut Self::Resources,
        (): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_STATX as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            // SAFETY: this is safe because `Metadata` is transparent.
            off: ptr::from_mut(&mut **metadata).addr() as u64,
        };
        asan::poison_box(metadata);
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: c"".as_ptr().addr() as u64, // Not using a path.
        };
        submission.0.len = METADATA_FLAGS;
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            statx_flags: libc::AT_EMPTY_PATH as u32,
        };
    }

    fn map_ok(_: &AsyncFd, metadata: Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        asan::unpoison_box(&metadata);
        msan::unpoison_box(&metadata);
        debug_assert!(n == 0);
        debug_assert!(metadata.mask() & METADATA_FLAGS == METADATA_FLAGS);
        *metadata
    }
}

pub(crate) struct AdviseOp;

impl io_uring::FdOp for AdviseOp {
    type Output = ();
    type Resources = ();
    type Args = (u64, u32, AdviseFlag); // offset, length, advice

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        fd: &AsyncFd,
        (): &mut Self::Resources,
        (offset, length, advice): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_FADVISE as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: *offset };
        submission.0.len = *length;
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            fadvise_advice: advice.0,
        };
    }

    fn map_ok(_: &AsyncFd, (): Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}

pub(crate) struct AllocateOp;

impl io_uring::FdOp for AllocateOp {
    type Output = ();
    type Resources = ();
    type Args = (u64, u32, AllocateFlag); // offset, length, mode

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        fd: &AsyncFd,
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
        submission.0.len = mode.0;
    }

    fn map_ok(_: &AsyncFd, (): Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}

pub(crate) struct TruncateOp;

impl io_uring::FdOp for TruncateOp {
    type Output = ();
    type Resources = ();
    type Args = u64; // length

    fn fill_submission(
        fd: &AsyncFd,
        (): &mut Self::Resources,
        length: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_FTRUNCATE as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: *length };
    }

    fn map_ok(_: &AsyncFd, (): Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}

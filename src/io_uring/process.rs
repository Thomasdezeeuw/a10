use std::os::fd::RawFd;
use std::ptr;

use crate::fd::{AsyncFd, Descriptor, Direct, File};
use crate::io::NO_OFFSET;
use crate::io_uring::{self, cq, libc, sq};
use crate::op::FdIter;
use crate::process::{Signals, WaitOn};
use crate::SubmissionQueue;

pub(crate) struct WaitIdOp;

impl io_uring::Op for WaitIdOp {
    type Output = Box<libc::siginfo_t>;
    type Resources = Box<libc::siginfo_t>;
    type Args = (WaitOn, libc::c_int); // options.

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        info: &mut Self::Resources,
        (wait, options): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        let (id_type, pid) = match *wait {
            WaitOn::Process(pid) => (libc::P_PID, pid),
            WaitOn::Group(pid) => (libc::P_PGID, pid),
            WaitOn::All => (libc::P_ALL, 0), // NOTE: id is ignored.
        };
        submission.0.opcode = libc::IORING_OP_WAITID as u8;
        submission.0.fd = pid as _;
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            addr2: ptr::from_mut(&mut **info).addr() as _,
        };
        submission.0.len = id_type;
        submission.0.__bindgen_anon_5 = libc::io_uring_sqe__bindgen_ty_5 {
            file_index: *options as _,
        };
    }

    fn map_ok(_: &SubmissionQueue, info: Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        debug_assert!(n == 0);
        info
    }
}

pub(crate) struct ToSignalsDirectOp;

impl io_uring::Op for ToSignalsDirectOp {
    type Output = Signals<Direct>;
    type Resources = (Signals<File>, Box<RawFd>);
    type Args = ();

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        (_, fd): &mut Self::Resources,
        (): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_FILES_UPDATE as u8;
        submission.0.fd = -1;
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            off: libc::IORING_FILE_INDEX_ALLOC as _,
        };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: ptr::from_mut(&mut **fd).addr() as _,
        };
        submission.0.len = 1;
    }

    fn map_ok(
        sq: &SubmissionQueue,
        (signals, dfd): Self::Resources,
        (_, n): cq::OpReturn,
    ) -> Self::Output {
        debug_assert!(n == 1);
        // SAFETY: the kernel ensures that `dfd` is valid.
        let dfd = unsafe { AsyncFd::from_raw(*dfd, sq.clone()) };
        unsafe { signals.change_fd(dfd) }
    }
}

pub(crate) struct ReceiveSignalOp;

impl io_uring::FdOp for ReceiveSignalOp {
    type Output = libc::signalfd_siginfo;
    type Resources = Box<libc::signalfd_siginfo>;
    type Args = ();

    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        info: &mut Self::Resources,
        (): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_READ as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: NO_OFFSET };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: ptr::from_mut(&mut **info).addr() as _,
        };
        submission.0.len = size_of::<libc::signalfd_siginfo>() as u32;
        submission.set_async();
    }

    fn map_ok<D: Descriptor>(
        fd: &AsyncFd<D>,
        mut info: Self::Resources,
        ok: cq::OpReturn,
    ) -> Self::Output {
        ReceiveSignalOp::map_next(fd, &mut info, ok)
    }
}

impl FdIter for ReceiveSignalOp {
    fn map_next<D: Descriptor>(
        _: &AsyncFd<D>,
        info: &mut Self::Resources,
        (_, n): Self::OperationOutput,
    ) -> Self::Output {
        debug_assert!(n == size_of::<libc::signalfd_siginfo>() as u32);
        **info
    }
}

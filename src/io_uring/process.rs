use std::os::fd::RawFd;
use std::ptr;

use crate::io::NO_OFFSET;
use crate::io_uring::op::{FdOp, Op, OpReturn};
use crate::io_uring::{self, libc, sq};
use crate::process::{Signal, Signals, WaitInfo, WaitOn, WaitOption};
use crate::{AsyncFd, SubmissionQueue};

pub(crate) struct WaitIdOp;

impl Op for WaitIdOp {
    type Output = WaitInfo;
    type Resources = WaitInfo;
    type Args = (WaitOn, WaitOption);

    #[allow(clippy::cast_sign_loss)]
    #[allow(clippy::cast_possible_wrap)]
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
        submission.0.fd = pid as RawFd;
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            addr2: ptr::from_mut(info).addr() as u64,
        };
        submission.0.len = id_type;
        submission.0.__bindgen_anon_5 = libc::io_uring_sqe__bindgen_ty_5 {
            file_index: options.0,
        };
    }

    fn map_ok(_: &SubmissionQueue, info: Self::Resources, (_, n): OpReturn) -> Self::Output {
        debug_assert!(n == 0);
        info
    }
}

impl io_uring::fd::DirectFdMapper for Signals {
    type Output = Self;

    fn map(mut self, dfd: AsyncFd) -> Self::Output {
        // SAFETY: since we used the original descriptor to make this direct
        // descriptor we're ensure that it still a signalfd.
        unsafe { self.set_fd(dfd) }
        self
    }
}

pub(crate) struct ReceiveSignalOp;

impl FdOp for ReceiveSignalOp {
    type Output = crate::process::SignalInfo;
    type Resources = crate::process::SignalInfo;
    type Args = ();

    fn fill_submission(
        fd: &AsyncFd,
        info: &mut Self::Resources,
        (): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_READ as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: NO_OFFSET };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: ptr::from_mut(info).addr() as u64,
        };
        submission.0.len = size_of::<libc::signalfd_siginfo>() as u32;
        submission.set_async();
    }

    fn map_ok(_: &AsyncFd, info: Self::Resources, (_, n): OpReturn) -> Self::Output {
        debug_assert!(n == size_of::<libc::signalfd_siginfo>() as u32);
        info
    }
}

pub(crate) use libc::signalfd_siginfo as SignalInfo;

pub(crate) const fn signal(info: &SignalInfo) -> Signal {
    Signal(info.ssi_signo as i32)
}

pub(crate) const fn pid(info: &SignalInfo) -> u32 {
    info.ssi_pid
}

pub(crate) const fn real_user_id(info: &SignalInfo) -> u32 {
    info.ssi_uid
}

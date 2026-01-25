use std::mem::MaybeUninit;
use std::os::fd::RawFd;
use std::{io, ptr};

use crate::io::NO_OFFSET;
use crate::io_uring::op::{FdOp, Op, OpReturn};
use crate::io_uring::{self, libc, sq};
use crate::op::operation;
use crate::process::{Signal, SignalSet, Signals, WaitInfo, WaitOn, WaitOption};
use crate::{AsyncFd, SubmissionQueue, fd, syscall};

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

/// io_uring specific methods.
impl Signals {
    pub(crate) fn new(sq: SubmissionQueue, signals: SignalSet) -> io::Result<Signals> {
        let fd = syscall!(signalfd(-1, &raw const signals.0, libc::SFD_CLOEXEC))?;
        // SAFETY: signalfd(2) ensures that fd is valid.
        let sfd = unsafe { AsyncFd::from_raw(fd, fd::Kind::File, sq) };
        // Block all signals as we're going to read them from the signalfd.
        sigprocmask(libc::SIG_BLOCK, &signals.0)?;
        Ok(Signals { fd: sfd, signals })
    }

    /// Convert `Signals` from using a regular file descriptor to using a direct
    /// descriptor.
    ///
    /// See [`AsyncFd::to_direct_descriptor`].
    #[cfg(any(target_os = "android", target_os = "linux"))]
    pub fn to_direct_descriptor(self) -> ToDirect {
        debug_assert!(
            matches!(self.fd.kind(), fd::Kind::File),
            "can't covert a direct descriptor to a different direct descriptor"
        );
        let sq = self.fd.sq().clone();
        let fd = self.fd.fd();
        ToDirect::new(sq, (self, fd), ())
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
        if let Err(err) = sigprocmask(libc::SIG_UNBLOCK, &self.signals.0) {
            log::error!(signals:? = self.signals; "error unblocking signals: {err}");
        }
    }
}

fn sigprocmask(how: libc::c_int, set: &libc::sigset_t) -> io::Result<()> {
    syscall!(pthread_sigmask(how, set, ptr::null_mut()))?;
    Ok(())
}

operation!(
    /// [`Future`] behind [`Signals::to_direct_descriptor`].
    pub struct ToDirect(io_uring::fd::ToDirectOp<crate::process::Signals>) -> io::Result<crate::process::Signals>;
);

impl io_uring::fd::DirectFdMapper for crate::process::Signals {
    type Output = Self;

    fn map(mut self, dfd: AsyncFd) -> Self::Output {
        self.fd = dfd;
        self
    }
}

pub(crate) struct ReceiveSignalOp;

impl FdOp for ReceiveSignalOp {
    type Output = crate::process::SignalInfo;
    type Resources = MaybeUninit<crate::process::SignalInfo>;
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
            addr: info.as_mut_ptr().addr() as u64,
        };
        submission.0.len = size_of::<libc::signalfd_siginfo>() as u32;
        submission.set_async();
    }

    fn map_ok(_: &AsyncFd, info: Self::Resources, (_, n): OpReturn) -> Self::Output {
        debug_assert!(n == size_of::<libc::signalfd_siginfo>() as u32);
        // SAFETY: initialised the info above.
        unsafe { info.assume_init() }
    }
}

pub(crate) use libc::signalfd_siginfo as SignalInfo;

pub(crate) const fn signal(info: &SignalInfo) -> Signal {
    Signal(info.ssi_signo.cast_signed())
}

pub(crate) const fn pid(info: &SignalInfo) -> u32 {
    info.ssi_pid
}

pub(crate) const fn real_user_id(info: &SignalInfo) -> u32 {
    info.ssi_uid
}

use std::mem::{MaybeUninit, replace};
use std::os::fd::RawFd;
use std::task::{self, Poll};
use std::{io, ptr};

use crate::kqueue::fd::OpKind;
use crate::kqueue::op::{EventedState, FdOp};
use crate::process::{Signal, SignalSet, Signals, WaitInfo, WaitOn, WaitOption};
use crate::{AsyncFd, SubmissionQueue, syscall};

pub(crate) struct WaitIdOp;

impl crate::op::Op for WaitIdOp {
    type Output = io::Result<WaitInfo>;
    type Resources = WaitInfo;
    type Args = (WaitOn, WaitOption);
    type State = EventedState<Self::Resources, Self::Args>;

    fn poll(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        sq: &SubmissionQueue,
    ) -> Poll<Self::Output> {
        match state {
            EventedState::NotStarted { args, .. } | EventedState::ToSubmit { args, .. } => {
                let (wait, _) = args;
                let WaitOn::Process(pid) = *wait;
                sq.submissions().add(|event| {
                    event.0.filter = libc::EVFILT_PROC;
                    event.0.ident = pid as _;
                    event.0.flags = libc::EV_RECEIPT | libc::EV_ONESHOT | libc::EV_ADD;
                    event.0.fflags = libc::NOTE_EXIT;
                    // Wake the Future once the process has exited.
                    event.0.udata = Box::into_raw(Box::new(ctx.waker().clone())).cast();
                });

                // Set ourselves to waiting for an event from the kernel.
                if let EventedState::NotStarted { resources, args }
                | EventedState::ToSubmit { resources, args } =
                    replace(state, EventedState::Complete)
                {
                    *state = EventedState::Waiting { resources, args };
                }
                // We've added our waker above to the list, we'll be woken up
                // once we can make progress.
                Poll::Pending
            }
            EventedState::Waiting { resources, args } => {
                let info = resources;
                let (wait, options) = args;
                let (id_type, pid) = match *wait {
                    WaitOn::Process(pid) => (libc::P_PID, pid),
                };

                let options = options.0.cast_signed() | libc::WNOHANG; // Don't block.
                syscall!(waitid(id_type, pid, &raw mut info.0, options))?;

                if info.0.si_pid == 0 {
                    // Got polled without the process stopping, will have to
                    // wait again.
                    Poll::Pending
                } else {
                    let info = WaitInfo(info.0);
                    *state = EventedState::Complete;
                    Poll::Ready(Ok(info))
                }
            }
            // Shouldn't be reachable, but if the Future is used incorrectly it
            // can be.
            EventedState::Complete => panic!("polled Future after completion"),
        }
    }
}

impl Signals {
    pub(crate) fn new(sq: SubmissionQueue, signals: SignalSet) -> io::Result<Signals> {
        // SAFETY: `kqueue(2)` ensures the fd is valid.
        let kfd = unsafe { AsyncFd::from_raw_fd(syscall!(kqueue())?, sq) };
        syscall!(fcntl(kfd.fd(), libc::F_SETFD, libc::FD_CLOEXEC))?;
        register_signals(kfd.fd(), &signals)?;
        // Ignore all signals as we want them to be deleted to the kqueue.
        sigaction(&signals, libc::SIG_IGN)?;
        Ok(Signals { fd: kfd, signals })
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
        if let Err(err) = sigaction(&self.signals, libc::SIG_DFL) {
            log::error!(signals:? = self.signals; "error resetting signal handlers: {err}");
        }
    }
}

#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
fn register_signals(kfd: RawFd, signals: &SignalSet) -> io::Result<()> {
    let mut changes: [MaybeUninit<libc::kevent>; _] =
        [MaybeUninit::uninit(); Signal::ALL_VALUES.len()];

    let mut n_changes = 0;
    for signal in Signal::ALL_VALUES {
        if !signals.contains(*signal) {
            continue;
        }

        changes[n_changes].write(libc::kevent {
            ident: signal.0 as _,
            filter: libc::EVFILT_SIGNAL,
            flags: libc::EV_ADD,
            // SAFETY: all zeros is valid for `kevent`.
            ..unsafe { std::mem::zeroed() }
        });
        n_changes += 1;
    }

    syscall!(kevent(
        kfd,
        changes[0].as_ptr(),
        n_changes as _,
        ptr::null_mut(),
        0,
        ptr::null(),
    ))?;
    Ok(())
}

fn sigaction(signals: &SignalSet, action: libc::sighandler_t) -> io::Result<()> {
    let action = libc::sigaction {
        sa_sigaction: action,
        sa_mask: 0,
        sa_flags: 0,
    };
    for signal in Signal::ALL_VALUES {
        if !signals.contains(*signal) {
            continue;
        }

        syscall!(sigaction(signal.0, &raw const action, ptr::null_mut()))?;
    }
    Ok(())
}

pub(crate) struct ReceiveSignalOp;

impl FdOp for ReceiveSignalOp {
    type Output = crate::process::SignalInfo;
    type Resources = MaybeUninit<crate::process::SignalInfo>;
    type Args = ();
    type OperationOutput = ();

    const OP_KIND: OpKind = OpKind::Read;

    #[allow(clippy::cast_possible_wrap)]
    fn try_run(
        kfd: &AsyncFd,
        info: &mut Self::Resources,
        (): &mut Self::Args,
    ) -> io::Result<Self::OperationOutput> {
        let mut event: MaybeUninit<libc::kevent> = MaybeUninit::uninit();
        // No blocking.
        let timeout = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let n = syscall!(kevent(
            kfd.fd(),
            ptr::null(),
            0,
            event.as_mut_ptr(),
            1,
            &raw const timeout
        ))?;
        if n == 0 {
            // Wait for another readiness event.
            Err(io::ErrorKind::WouldBlock.into())
        } else {
            debug_assert!(n == 1);
            // SAFETY: kevent just initialised the event for us.
            let event = unsafe { event.assume_init() };
            debug_assert_eq!(event.filter, libc::EVFILT_SIGNAL);
            info.write(crate::process::SignalInfo(Signal(event.ident as _)));
            Ok(())
        }
    }

    fn map_ok(_: &AsyncFd, info: Self::Resources, (): Self::OperationOutput) -> Self::Output {
        // SAFETY: initialised the info above.
        unsafe { info.assume_init() }
    }
}

// kqueue doesn't give us a lot of info.
pub(crate) use crate::process::Signal as SignalInfo;

#[allow(clippy::trivially_copy_pass_by_ref)]
pub(crate) const fn signal(info: &SignalInfo) -> Signal {
    *info
}

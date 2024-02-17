//! Process handling.
//!
//! In this module process signal handling is also supported. For that See the
//! documentation of [`Signals`].

use std::future::Future;
use std::mem::{self, size_of, ManuallyDrop, MaybeUninit};
use std::pin::Pin;
use std::process::Child;
use std::task::{self, Poll};
use std::{fmt, io, ptr};

use log::{error, trace};

use crate::cancel::{Cancel, CancelOp, CancelResult};
use crate::libc::{self, syscall};
use crate::op::{op_future, poll_state, OpState, NO_OFFSET};
use crate::{AsyncFd, QueueFull, SubmissionQueue};

/// Wait on the child `process`.
///
/// See [`wait`].
pub fn wait_on(sq: SubmissionQueue, process: &Child, options: libc::c_int) -> WaitId {
    wait(sq, WaitOn::Process(process.id()), options)
}

/// Obtain status information on termination, stop, and/or continue events in
/// one of the caller's child processes.
///
/// Also see [`wait_on`] to wait on a [`Child`] process.
#[doc(alias = "waitid")]
pub fn wait(sq: SubmissionQueue, wait: WaitOn, options: libc::c_int) -> WaitId {
    WaitId {
        sq,
        state: OpState::NotStarted((wait, options)),
        info: Some(Box::new(MaybeUninit::uninit())),
    }
}

/// Defines on what process (or processes) to wait.
#[doc(alias = "idtype")]
#[doc(alias = "idtype_t")]
#[derive(Copy, Clone, Debug)]
pub enum WaitOn {
    /// Wait for the child process.
    #[doc(alias = "P_PID")]
    Process(libc::id_t),
    /// Wait for any child process in the process group with ID.
    #[doc(alias = "P_PGID")]
    Group(libc::id_t),
    /// Wait for all childeren.
    #[doc(alias = "P_ALL")]
    All,
}

/// [`Future`] behind [`wait`].
#[derive(Debug)]
#[must_use = "`Future`s do nothing unless polled"]
pub struct WaitId {
    sq: SubmissionQueue,
    /// Buffer to write into, needs to stay in memory so the kernel can
    /// access it safely.
    info: Option<Box<MaybeUninit<libc::signalfd_siginfo>>>,
    state: OpState<(WaitOn, libc::c_int)>,
}

impl Future for WaitId {
    type Output = io::Result<Box<libc::siginfo_t>>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let op_index = poll_state!(
            WaitId,
            self.state,
            self.sq,
            ctx,
            |submission, (wait, options)| unsafe {
                let (id_type, pid) = match wait {
                    WaitOn::Process(pid) => (libc::P_PID, pid),
                    WaitOn::Group(pid) => (libc::P_PGID, pid),
                    WaitOn::All => (libc::P_ALL, 0), // NOTE: id is ignored.
                };
                let info = self.info.as_ref().unwrap().as_ptr().cast_mut();
                submission.waitid(pid, id_type, options, info);
            }
        );

        match self.sq.poll_op(ctx, op_index) {
            Poll::Ready(result) => {
                self.state = OpState::Done;
                match result {
                    Ok((_, _)) => Poll::Ready(Ok(unsafe {
                        Box::from_raw(Box::into_raw(self.info.take().unwrap()).cast())
                    })),
                    Err(err) => Poll::Ready(Err(err)),
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Cancel for WaitId {
    fn try_cancel(&mut self) -> CancelResult {
        self.state.try_cancel(&self.sq)
    }

    fn cancel(&mut self) -> CancelOp {
        self.state.cancel(&self.sq)
    }
}

impl Drop for WaitId {
    fn drop(&mut self) {
        if let Some(info) = self.info.take() {
            match self.state {
                OpState::Running(op_index) => {
                    // Only drop the signal `info` field once we know the
                    // operation has finished, otherwise the kernel might write
                    // into memory we have deallocated.
                    let result = self.sq.cancel_op(op_index, info, |submission| unsafe {
                        submission.cancel_op(op_index);
                        // We'll get a canceled completion event if we succeeded, which
                        // is sufficient to cleanup the operation.
                        submission.no_completion_event();
                    });
                    if let Err(err) = result {
                        log::error!("dropped a10::WaitId before canceling it, attempt to cancel failed: {err}");
                    }
                }
                OpState::NotStarted((_, _)) | OpState::Done => drop(info),
            }
        }
    }
}

/// Notification of process signals.
///
/// # Multithreaded process
///
/// For `Signals` to function correctly in multithreaded processes it must be
/// created on the main thread **before** spawning any threads. This is due to
/// an implementation detail where the spawned threads must inherit various
/// signal related thread properties from the parent thread.
///
/// Any threads spawned before creating a `Signals` instance will experience the
/// default process signals behaviour, i.e. sending it a signal will interrupt
/// or stop it.
///
/// # Implementation Notes
///
/// This will block all signals in the signal set given when creating `Signals`,
/// using [`pthread_sigmask(3)`]. This means that the thread in which `Signals`
/// was created (and it's children) is not interrupted, or in any way notified
/// of a signal until [`Signals::receive`] is called (and the returned
/// [`Future`] polled to completion). Under the hood [`Signals`] is just a
/// wrapper around [`signalfd(2)`].
///
/// [`pthread_sigmask(3)`]: https://man7.org/linux/man-pages/man3/pthread_sigmask.3.html
/// [`Future`]: std::future::Future
/// [`signalfd(2)`]: http://man7.org/linux/man-pages/man2/signalfd.2.html
///
/// # Examples
///
/// ```
/// use std::io;
/// use std::mem::MaybeUninit;
///
/// use a10::Ring;
/// use a10::signals::Signals;
///
/// # fn main() {
/// async fn main() -> io::Result<()> {
///     let ring = Ring::new(128)?;
///     let sq = ring.submission_queue().clone();
///
///     // Create a new `Signals` instance.
///     let signals = Signals::from_signals(sq, [libc::SIGINT, libc::SIGQUIT, libc::SIGTERM])?;
///
///     let signal_info = signals.receive().await?;
///     println!("Got process signal: {}", signal_info.ssi_signo);
///     Ok(())
/// }
/// # }
/// ```
#[derive(Debug)]
pub struct Signals {
    /// `signalfd(2)` file descriptor.
    fd: AsyncFd,
    /// All signals this is listening for, used in resetting the signal handlers.
    signals: SignalSet,
}

/// Wrapper around [`libc::sigset_t`] to implement [`fmt::Debug`].
#[repr(transparent)]
struct SignalSet(libc::sigset_t);

impl Signals {
    /// Create a new signal notifier from a signal set.
    pub fn from_set(sq: SubmissionQueue, signals: libc::sigset_t) -> io::Result<Signals> {
        let signals = SignalSet(signals);
        trace!(signals:? = signals; "setting up signal handling");
        let fd = libc::syscall!(signalfd(-1, &signals.0, libc::SFD_CLOEXEC))?;
        // SAFETY: `signalfd(2)` ensures that `fd` is valid.
        let fd = unsafe { AsyncFd::from_raw_fd(fd, sq) };
        // Block all `signals` as we're going to read them from the signalfd.
        sigprocmask(libc::SIG_BLOCK, &signals.0)?;
        Ok(Signals { fd, signals })
    }

    /// Create a new signal notifier from a collection of signals.
    pub fn from_signals<I>(sq: SubmissionQueue, signals: I) -> io::Result<Signals>
    where
        I: IntoIterator<Item = libc::c_int>,
    {
        let set = create_sigset(signals)?;
        Signals::from_set(sq, set)
    }

    /// Create a new signal notifier for all supported signals (set by `sigfillset(3)`).
    pub fn for_all_signals(sq: SubmissionQueue) -> io::Result<Signals> {
        let mut set: MaybeUninit<libc::sigset_t> = MaybeUninit::uninit();
        syscall!(sigfillset(set.as_mut_ptr()))?;
        // SAFETY: initialised the set in the call to `sigfillset`.
        let set = unsafe { set.assume_init() };
        Signals::from_set(sq, set)
    }

    /// Receive a signal.
    pub fn receive<'fd>(&'fd self) -> ReceiveSignal<'fd> {
        // TODO: replace with `Box::new_uninit` once `new_uninit` is stable.
        let info = Box::new(MaybeUninit::uninit());
        ReceiveSignal::new(&self.fd, info, ())
    }

    /// Receive multiple signals.
    ///
    /// This is an combined, owned version of `Signals` and `Receive` (the
    /// future behind `Signals::receive`). This is useful if you don't want to
    /// deal with the `'fd` lifetime.
    pub fn receive_signals(self) -> ReceiveSignals {
        ReceiveSignals {
            signals: self,
            // TODO: replace with `Box::new_zeroed` once stable.
            // SAFETY: all zero is valid for `signalfd_siginfo`.
            info: ManuallyDrop::new(Box::new(unsafe { mem::zeroed() })),
            state: OpState::NotStarted(()),
        }
    }
}

/// Create a `sigset_t` from `signals`.
fn create_sigset<I: IntoIterator<Item = libc::c_int>>(signals: I) -> io::Result<libc::sigset_t> {
    let mut set: MaybeUninit<libc::sigset_t> = MaybeUninit::uninit();
    syscall!(sigemptyset(set.as_mut_ptr()))?;
    // SAFETY: initialised the set in the call to `sigemptyset`.
    let mut set = unsafe { set.assume_init() };
    for signal in signals {
        syscall!(sigaddset(&mut set, signal))?;
    }
    Ok(set)
}

// ReceiveSignal.
op_future! {
    fn Signals::receive -> Box<libc::signalfd_siginfo>,
    struct ReceiveSignal<'fd> {
        /// Buffer to write into, needs to stay in memory so the kernel can
        /// access it safely.
        info: Box<MaybeUninit<libc::signalfd_siginfo>>,
    },
    setup_state: _unused: (),
    setup: |submission, fd, (info,), _unused| unsafe {
        let ptr = (**info).as_mut_ptr().cast();
        submission.read_at(fd.fd(), ptr, size_of::<libc::signalfd_siginfo>() as u32, NO_OFFSET);
    },
    map_result: |this, (info,), n| {
        #[allow(clippy::cast_sign_loss)] // Negative values are mapped to errors.
        { debug_assert_eq!(n as usize, size_of::<libc::signalfd_siginfo>()) };
        // TODO: replace with `Box::assume_init` once `new_uninit` is stable.
        // SAFETY: the kernel initialised the info allocation for us as part of
        // the read call.
        Ok(unsafe { Box::from_raw(Box::into_raw(info).cast()) })
    },
}

/// Known signals supported by Linux as of v6.3.
const KNOWN_SIGNALS: [(libc::c_int, &str); 33] = [
    (libc::SIGHUP, "SIGHUP"),
    (libc::SIGINT, "SIGINT"),
    (libc::SIGQUIT, "SIGQUIT"),
    (libc::SIGILL, "SIGILL"),
    (libc::SIGTRAP, "SIGTRAP"),
    (libc::SIGABRT, "SIGABRT"),
    (libc::SIGIOT, "SIGIOT"),
    (libc::SIGBUS, "SIGBUS"),
    (libc::SIGFPE, "SIGFPE"),
    (libc::SIGKILL, "SIGKILL"),
    (libc::SIGUSR1, "SIGUSR1"),
    (libc::SIGSEGV, "SIGSEGV"),
    (libc::SIGUSR2, "SIGUSR2"),
    (libc::SIGPIPE, "SIGPIPE"),
    (libc::SIGALRM, "SIGALRM"),
    (libc::SIGTERM, "SIGTERM"),
    (libc::SIGSTKFLT, "SIGSTKFLT"),
    (libc::SIGCHLD, "SIGCHLD"),
    (libc::SIGCONT, "SIGCONT"),
    (libc::SIGSTOP, "SIGSTOP"),
    (libc::SIGTSTP, "SIGTSTP"),
    (libc::SIGTTIN, "SIGTTIN"),
    (libc::SIGTTOU, "SIGTTOU"),
    (libc::SIGURG, "SIGURG"),
    (libc::SIGXCPU, "SIGXCPU"),
    (libc::SIGXFSZ, "SIGXFSZ"),
    (libc::SIGVTALRM, "SIGVTALRM"),
    (libc::SIGPROF, "SIGPROF"),
    (libc::SIGWINCH, "SIGWINCH"),
    (libc::SIGIO, "SIGIO"),
    (libc::SIGPOLL, "SIGPOLL"), // NOTE: same value as `SIGIO`.
    //(libc::SIGLOST, "SIGLOST"),
    (libc::SIGPWR, "SIGPWR"),
    (libc::SIGSYS, "SIGSYS"),
];

impl fmt::Debug for SignalSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let signals = KNOWN_SIGNALS.into_iter().filter_map(|(signal, name)| {
            // SAFETY: we ensure the pointer to the signal set is valid.
            (unsafe { libc::sigismember(&self.0, signal) } == 1).then_some(name)
        });
        f.debug_list().entries(signals).finish()
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
        // Reverse the blocking of signals.
        if let Err(err) = sigprocmask(libc::SIG_UNBLOCK, &self.signals.0) {
            error!(signals:? = self.signals; "error unblocking signals: {err}");
        }
    }
}

fn sigprocmask(how: libc::c_int, set: &libc::sigset_t) -> io::Result<()> {
    libc::syscall!(pthread_sigmask(how, set, ptr::null_mut()))?;
    Ok(())
}

/// [`AsyncIterator`] behind [`Signals::receive_signals`].
///
/// [`AsyncIterator`]: std::async_iter::AsyncIterator
#[must_use = "`Future`s do nothing unless polled"]
#[allow(clippy::module_name_repetitions)]
pub struct ReceiveSignals {
    signals: Signals,
    info: ManuallyDrop<Box<libc::signalfd_siginfo>>,
    state: OpState<()>,
}

impl ReceiveSignals {
    /// Poll the next signal.
    pub fn poll_signal<'a>(
        &'a mut self,
        ctx: &mut task::Context<'_>,
    ) -> Poll<Option<io::Result<&'a libc::signalfd_siginfo>>> {
        let ReceiveSignals {
            signals,
            info,
            state,
        } = self;
        let op_index = match state {
            OpState::Running(op_index) => *op_index,
            OpState::NotStarted(()) => {
                let result = signals.fd.sq.add(|submission| unsafe {
                    submission.read_at(
                        signals.fd.fd(),
                        ptr::addr_of_mut!(***info).cast(),
                        size_of::<libc::signalfd_siginfo>() as u32,
                        NO_OFFSET,
                    );
                });
                match result {
                    Ok(op_index) => {
                        *state = OpState::Running(op_index);
                        op_index
                    }
                    Err(QueueFull(())) => {
                        signals.fd.sq.wait_for_submission(ctx.waker().clone());
                        return Poll::Pending;
                    }
                }
            }
            OpState::Done => return Poll::Ready(None),
        };

        match signals.fd.sq.poll_op(ctx, op_index) {
            Poll::Ready(Ok((_, n))) => {
                // Reset the state so that we start reading another signal in
                // the next call.
                *state = OpState::NotStarted(());
                #[allow(clippy::cast_sign_loss)] // Negative values are mapped to errors.
                {
                    debug_assert_eq!(n as usize, size_of::<libc::signalfd_siginfo>());
                }
                // SAFETY: the kernel initialised the info allocation for us as
                // part of the read call.
                Poll::Ready(Some(Ok(&**info)))
            }
            Poll::Ready(Err(err)) => {
                *state = OpState::Done; // Consider the error as fatal.
                Poll::Ready(Some(Err(err)))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[allow(clippy::missing_fields_in_debug)]
impl fmt::Debug for ReceiveSignals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReceiveSignals")
            .field("signals", &self.signals)
            // NOTE: `info` can't be read as the kernel might be writing to it.
            .field("state", &self.state)
            .finish()
    }
}

impl Drop for ReceiveSignals {
    fn drop(&mut self) {
        let signal_info = unsafe { ManuallyDrop::take(&mut self.info) };
        match self.state {
            OpState::Running(op_index) => {
                // Only drop the signal `info` field once we know the operation has
                // finished, otherwise the kernel might write into memory we have
                // deallocated.
                // SAFETY: we're in the `Drop` implementation, so `self.info` can't
                // be used anymore making it safe to take ownership.
                let result =
                    self.signals
                        .fd
                        .sq
                        .cancel_op(op_index, signal_info, |submission| unsafe {
                            submission.cancel_op(op_index);
                            // We'll get a canceled completion event if we succeeded, which
                            // is sufficient to cleanup the operation.
                            submission.no_completion_event();
                        });
                if let Err(err) = result {
                    log::error!(
                        "dropped a10::ReceiveSignals before canceling it, attempt to cancel failed: {err}"
                    );
                }
            }
            OpState::NotStarted(()) | OpState::Done => drop(signal_info),
        }
    }
}

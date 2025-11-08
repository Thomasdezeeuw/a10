//! Process handling.
//!
//! In this module process signal handling is also supported. For that See the
//! documentation of [`Signals`].

use std::mem::{self, ManuallyDrop, MaybeUninit};
use std::pin::Pin;
use std::process::Child;
use std::task::{self, Poll};
use std::{fmt, io, ptr};

use crate::op::{self, fd_operation, operation, FdIter, FdOp, FdOperation, Operation};
use crate::{fd, man_link, sys, syscall, AsyncFd, SubmissionQueue};

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
#[doc = man_link!(waitid(2))]
#[doc(alias = "waitid")]
pub fn wait(sq: SubmissionQueue, wait: WaitOn, options: libc::c_int) -> WaitId {
    // SAFETY: fully zeroed `libc::signalfd_siginfo` is a valid value.
    let info = unsafe { Box::new(mem::zeroed()) };
    WaitId(Operation::new(sq, info, (wait, options)))
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

operation!(
    /// [`Future`] behind [`wait_on`] and [`wait`].
    pub struct WaitId(sys::process::WaitIdOp) -> io::Result<Box<libc::siginfo_t>>;
);

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
/// use a10::process::Signals;
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
pub struct Signals {
    /// `signalfd(2)` file descriptor.
    fd: AsyncFd,
    /// All signals this is listening for, used in resetting the signal handlers.
    signals: SignalSet,
}

impl Signals {
    /// Create a new signal notifier from a signal set.
    pub fn from_set(sq: SubmissionQueue, signals: libc::sigset_t) -> io::Result<Signals> {
        let signals = SignalSet(signals);
        log::trace!(signals:? = signals; "setting up signal handling");
        let fd = syscall!(signalfd(-1, &raw const signals.0, libc::SFD_CLOEXEC))?;
        // SAFETY: `signalfd(2)` ensures that `fd` is valid.
        let fd = unsafe { AsyncFd::from_raw(fd, fd::Kind::File, sq) };
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

    /// Convert `Signals` from using a regular file descriptor to using a direct
    /// descriptor.
    ///
    /// See [`AsyncFd::to_direct_descriptor`].
    pub fn to_direct_descriptor(self) -> ToSignalsDirect {
        debug_assert!(
            matches!(self.fd.kind(), fd::Kind::File),
            "can't covert a direct descriptor to a different direct descriptor"
        );
        let sq = self.fd.sq().clone();
        let fd = self.fd.fd();
        ToSignalsDirect(Operation::new(sq, (self, Box::new(fd)), ()))
    }

    /// Receive a signal.
    pub fn receive<'fd>(&'fd self) -> ReceiveSignal<'fd> {
        // SAFETY: fully zeroed `libc::signalfd_siginfo` is a valid value.
        let info = unsafe { Box::new(mem::zeroed()) };
        ReceiveSignal(FdOperation::new(&self.fd, info, ()))
    }

    /// Receive multiple signals.
    ///
    /// This is an combined, owned version of `Signals` and [`ReceiveSignal`]
    /// (the future behind `Signals::receive`). This is useful if you don't want
    /// to deal with the `'fd` lifetime.
    pub fn receive_signals(self) -> ReceiveSignals {
        // SAFETY: fully zeroed `libc::signalfd_siginfo` is a valid value.
        let resources = unsafe { Box::new(mem::zeroed()) };
        ReceiveSignals {
            signals: self,
            state: op::State::new(resources, ()),
        }
    }

    /// Change the file descriptor on the `Signals`.
    ///
    /// # Safety
    ///
    /// Caller must ensure `fd` is a signalfd valid descriptor.
    pub(crate) unsafe fn change_fd(self, fd: AsyncFd) -> Signals {
        let Signals { fd: _, signals: _ } = &self;
        // SAFETY: reading or dropping all fields of `Signals`.
        let mut signals = ManuallyDrop::new(self);
        unsafe { ptr::drop_in_place(&raw mut signals.fd) }
        let signals = unsafe { ptr::read(&raw const signals.signals) };
        Signals { fd, signals }
    }
}

impl fmt::Debug for Signals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Signals")
            .field("fd", &self.fd)
            .field("signals", &self.signals)
            .finish()
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
        // Reverse the blocking of signals.
        if let Err(err) = sigprocmask(libc::SIG_UNBLOCK, &self.signals.0) {
            log::error!(signals:? = self.signals; "error unblocking signals: {err}");
        }
    }
}

operation!(
    /// [`Future`] behind [`Signals::to_direct_descriptor`].
    pub struct ToSignalsDirect(sys::process::ToSignalsDirectOp) -> io::Result<Signals>;
);

fd_operation!(
    /// [`Future`] behind [`Signals::receive`].
    pub struct ReceiveSignal(sys::process::ReceiveSignalOp) -> io::Result<libc::signalfd_siginfo>;
);

/// [`AsyncIterator`] behind [`Signals::receive_signals`].
///
/// [`AsyncIterator`]: std::async_iter::AsyncIterator
#[must_use = "`AsyncIterator`s do nothing unless polled"]
#[derive(Debug)]
pub struct ReceiveSignals {
    signals: Signals,
    state: op::State<Box<libc::signalfd_siginfo>, ()>,
}

impl ReceiveSignals {
    /// This is the same as the [`AsyncIterator::poll_next`] function, but
    /// then available on stable Rust.
    ///
    /// [`AsyncIterator::poll_next`]: std::async_iter::AsyncIterator::poll_next
    pub fn poll_next(
        self: Pin<&mut Self>,
        ctx: &mut task::Context<'_>,
    ) -> Poll<Option<io::Result<libc::signalfd_siginfo>>> {
        // SAFETY: not moving `signals` or `state`.
        let ReceiveSignals { signals, state } = unsafe { self.get_unchecked_mut() };
        let fd = &signals.fd;
        let mut reset = None;
        // NOTE: not using `poll_next` as it's not a multishot operation.
        let result = state.poll(
            ctx,
            fd.sq(),
            |resources, args, submission| {
                fd.use_flags(submission);
                sys::process::ReceiveSignalOp::fill_submission(fd, resources, args, submission);
            },
            |resources, args, state| {
                sys::process::ReceiveSignalOp::check_result(fd, resources, args, state)
            },
            |_, mut resources, output| {
                let info = sys::process::ReceiveSignalOp::map_next(fd, &mut resources, output);
                reset = Some(resources);
                info
            },
        );
        if let Some(resources) = reset {
            *state = op::State::new(resources, ());
        }
        result.map(Some)
    }

    /// Returns the underlying [`Signals`].
    pub fn into_inner(self) -> Signals {
        let mut this = ManuallyDrop::new(self);
        let ReceiveSignals { signals, state } = &mut *this;
        // SAFETY: not using `state` any more.
        unsafe {
            state.drop(signals.fd.sq());
            ptr::drop_in_place(state);
        }
        // SAFETY: we're not dropping `self`/ (due to the the `ManuallyDrop`, so
        // `signals` is safe to return.
        unsafe { ptr::read(signals) }
    }
}

#[cfg(feature = "nightly")]
impl std::async_iter::AsyncIterator for ReceiveSignals {
    type Item = io::Result<libc::signalfd_siginfo>;

    fn poll_next(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Option<Self::Item>> {
        self.poll_next(ctx)
    }
}

impl Drop for ReceiveSignals {
    fn drop(&mut self) {
        // SAFETY: we're in the `Drop` implementation.
        unsafe { self.state.drop(self.signals.fd.sq()) }
    }
}

/// Wrapper around [`libc::sigset_t`] to implement [`fmt::Debug`].
#[repr(transparent)]
struct SignalSet(libc::sigset_t);

/// Create a `sigset_t` from `signals`.
fn create_sigset<I: IntoIterator<Item = libc::c_int>>(signals: I) -> io::Result<libc::sigset_t> {
    let mut set: MaybeUninit<libc::sigset_t> = MaybeUninit::uninit();
    syscall!(sigemptyset(set.as_mut_ptr()))?;
    // SAFETY: initialised the set in the call to `sigemptyset`.
    let mut set = unsafe { set.assume_init() };
    for signal in signals {
        syscall!(sigaddset(&raw mut set, signal))?;
    }
    Ok(set)
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
            (unsafe { libc::sigismember(&raw const self.0, signal) } == 1).then_some(name)
        });
        f.debug_set().entries(signals).finish()
    }
}

fn sigprocmask(how: libc::c_int, set: &libc::sigset_t) -> io::Result<()> {
    syscall!(pthread_sigmask(how, set, ptr::null_mut()))?;
    Ok(())
}

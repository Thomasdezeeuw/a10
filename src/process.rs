//! Process handling.
//!
//! In this module process signal handling is also supported. For that See the
//! documentation of [`Signals`].

use std::mem::{self, ManuallyDrop, MaybeUninit};
use std::process::Child;
use std::{fmt, io, ptr};

use crate::fd::{AsyncFd, Descriptor, Direct, File};
use crate::op::{fd_operation, operation, FdOperation, Operation};
use crate::{man_link, sys, syscall, SubmissionQueue};

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
pub struct Signals<D: Descriptor = File> {
    /// `signalfd(2)` file descriptor.
    fd: AsyncFd<D>,
    /// All signals this is listening for, used in resetting the signal handlers.
    signals: SignalSet,
}

impl Signals {
    /// Create a new signal notifier from a signal set.
    pub fn from_set(sq: SubmissionQueue, signals: libc::sigset_t) -> io::Result<Signals> {
        let signals = SignalSet(signals);
        log::trace!(signals:? = signals; "setting up signal handling");
        let fd = syscall!(signalfd(-1, &signals.0, libc::SFD_CLOEXEC))?;
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

    /// Convert `Signals` from using a regular file descriptor to using a direct
    /// descriptor.
    ///
    /// See [`AsyncFd::to_direct_descriptor`].
    pub fn to_direct_descriptor(self) -> ToSignalsDirect {
        let sq = self.fd.sq().clone();
        let fd = self.fd.fd();
        ToSignalsDirect(Operation::new(sq, (self, Box::new(fd)), ()))
    }

    /// Change the file descriptor on the `Signals`.
    ///
    /// # Safety
    ///
    /// Caller must ensure `fd` is a signalfd valid descriptor.
    pub(crate) unsafe fn change_fd<D: Descriptor>(self, fd: AsyncFd<D>) -> Signals<D> {
        let Signals { fd: _, signals: _ } = &self;
        // SAFETY: reading or dropping all fields of `Signals`.
        let mut signals = ManuallyDrop::new(self);
        unsafe { ptr::drop_in_place(&mut signals.fd) }
        let signals = unsafe { ptr::read(&mut signals.signals) };
        Signals { fd, signals }
    }
}

impl<D: Descriptor> Signals<D> {
    /// Receive a signal.
    pub fn receive<'fd>(&'fd self) -> ReceiveSignal<'fd, D> {
        // SAFETY: fully zeroed `libc::signalfd_siginfo` is a valid value.
        let info = unsafe { Box::new(mem::zeroed()) };
        ReceiveSignal(FdOperation::new(&self.fd, info, ()))
    }
}

impl<D: Descriptor> fmt::Debug for Signals<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Signals")
            .field("fd", &self.fd)
            .field("signals", &self.signals)
            .finish()
    }
}

impl<D: Descriptor> Drop for Signals<D> {
    fn drop(&mut self) {
        // Reverse the blocking of signals.
        if let Err(err) = sigprocmask(libc::SIG_UNBLOCK, &self.signals.0) {
            log::error!(signals:? = self.signals; "error unblocking signals: {err}");
        }
    }
}

operation!(
    /// [`Future`] behind [`Signals::to_direct_descriptor`].
    pub struct ToSignalsDirect(sys::process::ToSignalsDirectOp) -> io::Result<Signals<Direct>>;
);

fd_operation!(
    /// [`Future`] behind [`Signals::receive`].
    pub struct ReceiveSignal(sys::process::ReceiveSignalOp) -> io::Result<Box<libc::signalfd_siginfo>>;
);

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
        syscall!(sigaddset(&mut set, signal))?;
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
            (unsafe { libc::sigismember(&self.0, signal) } == 1).then_some(name)
        });
        f.debug_set().entries(signals).finish()
    }
}

fn sigprocmask(how: libc::c_int, set: &libc::sigset_t) -> io::Result<()> {
    syscall!(pthread_sigmask(how, set, ptr::null_mut()))?;
    Ok(())
}

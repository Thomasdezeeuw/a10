//! Process handling.
//!
//! [`wait`] and [`wait_on`] can be used to wait on started processes.
//! [`Signals`] can be used for process signal handling.

use std::mem::{self, ManuallyDrop, MaybeUninit};
use std::os::unix::process::ExitStatusExt;
use std::pin::Pin;
use std::process::{Child, ExitStatus};
use std::task::{self, Poll};
use std::{fmt, io, ptr};

use crate::op::{FdOp, OpState, fd_operation, operation};
use crate::{AsyncFd, SubmissionQueue, man_link, new_flag, sys, syscall};

#[cfg(any(target_os = "android", target_os = "linux"))]
pub use crate::sys::process::ToDirect;

/// Wait on the child `process`.
///
/// See [`wait`].
pub fn wait_on(sq: SubmissionQueue, process: &Child) -> WaitId {
    wait(sq, WaitOn::Process(process.id()))
}

/// Obtain status information on termination, stop, and/or continue events in
/// one of the caller's child processes.
///
/// Also see [`wait_on`] to wait on a [`Child`] process.
#[doc = man_link!(waitid(2))]
#[doc(alias = "waitid")]
pub fn wait(sq: SubmissionQueue, wait: WaitOn) -> WaitId {
    // SAFETY: fully zeroed `libc::siginfo_t` is a valid value.
    let info = unsafe { mem::zeroed() };
    WaitId::new(sq, info, (wait, WaitOption(0)))
}

/// Defines on what process (or processes) to wait.
#[doc(alias = "idtype")]
#[doc(alias = "idtype_t")]
#[derive(Copy, Clone, Debug)]
pub enum WaitOn {
    /// Wait for the child process.
    #[doc(alias = "P_PID")]
    Process(u32),
    /// Wait for any child process in the process group with ID.
    #[doc(alias = "P_PGID")]
    #[cfg(any(target_os = "android", target_os = "linux"))]
    Group(u32),
    /// Wait for all childeren.
    #[doc(alias = "P_ALL")]
    #[cfg(any(target_os = "android", target_os = "linux"))]
    All,
}

new_flag!(
    /// Wait option.
    ///
    /// See [`WaitId::flags`].
    pub struct WaitOption(u32) {
        /// Return if a child has stopped (but not traced via `ptrace(2)`).
        UNTRACED = libc::WUNTRACED,
        /// Wait for children that have been stopped by delivery of a signal.
        STOPPED = libc::WSTOPPED,
        /// Wait for children that have terminated.
        EXITED = libc::WEXITED,
        /// Return if a stopped child has been resumed by delivery of `SIGCONT`.
        CONTINUED = libc::WCONTINUED,
        /// Leave the child in a waitable state; a later wait call can be used
        /// to again retrieve the child status information.
        NO_WAIT = libc::WNOWAIT,
    }
);

/// Information about an awaited process.
///
/// See [`wait`] and [`wait_on`].
#[repr(transparent)]
pub struct WaitInfo(pub(crate) libc::siginfo_t);

impl WaitInfo {
    /// Process id of the child.
    pub fn pid(&self) -> i32 {
        unsafe { self.0.si_pid() }
    }

    /// Real user ID of the child.
    pub fn real_user_id(&self) -> u32 {
        unsafe { self.0.si_uid() }
    }

    /// Process signal.
    pub fn signal(&self) -> Signal {
        Signal(self.0.si_signo)
    }

    /// Either the exit status of the child or the signal that caused the child
    /// to terminate, stop, or continue.
    ///
    /// The [`code`] field can be used to determine how to interpret this field.
    ///
    /// [`code`]: WaitInfo::code
    pub fn status(&self) -> ExitStatus {
        unsafe { ExitStatus::from_raw(self.0.si_status()) }
    }

    /// Status of the child process.
    pub fn code(&self) -> ChildStatus {
        ChildStatus(self.0.si_code)
    }
}

unsafe impl Send for WaitInfo {}
unsafe impl Sync for WaitInfo {}

impl fmt::Debug for WaitInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WaitInfo")
            .field("pid", &self.pid())
            .field("real_user_id", &self.real_user_id())
            .field("signal", &self.signal())
            .field("status", &self.status())
            .field("code", &self.code())
            .finish()
    }
}

new_flag!(
    /// See [`WaitInfo::code`].
    pub struct ChildStatus(i32) {
        /// Exited (called `exit(3)`).
        EXITED = libc::CLD_EXITED,
        /// Killed by a signal.
        KILLED = libc::CLD_KILLED,
        /// Killed by a signal, dumped core.
        DUMPED = libc::CLD_DUMPED,
        /// Stopped by signal.
        STOPPED = libc::CLD_STOPPED,
        /// Traced child has trapped.
        TRAPPED = libc::CLD_TRAPPED,
        /// Continue by continue signal.
        CONTINUED = libc::CLD_CONTINUED,
    }
);

operation!(
    /// [`Future`] behind [`wait_on`] and [`wait`].
    pub struct WaitId(sys::process::WaitIdOp) -> io::Result<WaitInfo>;
);

impl WaitId {
    /// Set the `flags`.
    pub fn flags(mut self, options: WaitOption) -> Self {
        if let Some((_, o)) = self.state.args_mut() {
            *o = options;
        }
        self
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
/// Furthermore only a single instance of `Signals` should be active for a given
/// signal set. In other words, no two `Signals` instances should be created
/// for the signal. Easiest is to limit yourself to a single instance.
///
/// # Implementation Notes
///
/// ## io_uring
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
/// ## kqueue
///
/// This will block all signals in the signal set given when creating `Signals`,
/// by setting the handler action to `SIG_IGN` using [`sigaction(2)`], meaning
/// that all signals will be ignored. Same as on io_uring based systems; the
/// program is not interrupted, or in any way notified of signal until the
/// assiocated `Ring` is polled.
///
/// [`sigaction(2)`]: https://www.freebsd.org/cgi/man.cgi?query=sigaction&sektion=2
///
/// # Examples
///
/// ```
/// use std::io;
/// use std::mem::MaybeUninit;
///
/// use a10::Ring;
/// use a10::process::{Signals, Signal};
///
/// # fn main() {
/// async fn main() -> io::Result<()> {
///     let ring = Ring::new()?;
///     let sq = ring.sq().clone();
///
///     // Create a new `Signals` instance.
///     let signals = Signals::from_signals(sq, [Signal::INTERRUPT, Signal::QUIT, Signal::TERMINATION])?;
///
///     let signal_info = signals.receive().await?;
///     println!("Got process signal: {:?}", signal_info.signal());
///     Ok(())
/// }
/// # }
/// ```
pub struct Signals {
    /// `signalfd(2)` or `kqueue(2)` file descriptor.
    pub(crate) fd: AsyncFd,
    /// All signals this is listening for, used in resetting the signal handlers.
    pub(crate) signals: SignalSet,
}

impl Signals {
    /// Create a new signal notifier from a signal set.
    pub fn from_set(sq: SubmissionQueue, signals: SignalSet) -> io::Result<Signals> {
        log::trace!(signals:?; "setting up signal handling");
        Signals::new(sq, signals) // See sys::process.
    }

    /// Create a new signal notifier from a collection of signals.
    pub fn from_signals<I>(sq: SubmissionQueue, signals: I) -> io::Result<Signals>
    where
        I: IntoIterator<Item = Signal>,
    {
        let mut set = SignalSet::empty()?;
        for signal in signals {
            set.add(signal)?;
        }
        Signals::from_set(sq, set)
    }

    /// Create a new signal notifier for all supported signals.
    pub fn for_all_signals(sq: SubmissionQueue) -> io::Result<Signals> {
        let set = SignalSet::all()?;
        Signals::from_set(sq, set)
    }

    /// Receive a signal.
    pub fn receive<'fd>(&'fd self) -> ReceiveSignal<'fd> {
        ReceiveSignal::new(&self.fd, MaybeUninit::uninit(), ())
    }

    /// Receive multiple signals.
    ///
    /// This is an combined, owned version of `Signals` and [`ReceiveSignal`]
    /// (the future behind `Signals::receive`). This is useful if you don't want
    /// to deal with the `'fd` lifetime.
    pub fn receive_signals(self) -> ReceiveSignals {
        ReceiveSignals {
            signals: self,
            state: <sys::process::ReceiveSignalOp as FdOp>::State::new(MaybeUninit::uninit(), ()),
        }
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

fd_operation!(
    /// [`Future`] behind [`Signals::receive`].
    pub struct ReceiveSignal(sys::process::ReceiveSignalOp) -> io::Result<SignalInfo>;
);

/// [`AsyncIterator`] behind [`Signals::receive_signals`].
///
/// [`AsyncIterator`]: std::async_iter::AsyncIterator
#[must_use = "`AsyncIterator`s do nothing unless polled"]
#[derive(Debug)]
pub struct ReceiveSignals {
    signals: Signals,
    state: <sys::process::ReceiveSignalOp as FdOp>::State,
}

impl ReceiveSignals {
    /// This is the same as the [`AsyncIterator::poll_next`] function, but
    /// then available on stable Rust.
    ///
    /// [`AsyncIterator::poll_next`]: std::async_iter::AsyncIterator::poll_next
    pub fn poll_next(
        self: Pin<&mut Self>,
        ctx: &mut task::Context<'_>,
    ) -> Poll<Option<io::Result<SignalInfo>>> {
        // SAFETY: not moving data out of self/this.
        let this = unsafe { Pin::get_unchecked_mut(self) };
        let result = sys::process::ReceiveSignalOp::poll(&mut this.state, ctx, &this.signals.fd);
        match result {
            Poll::Ready(Ok(info)) => {
                this.state.reset(MaybeUninit::uninit(), ());
                Poll::Ready(Some(Ok(info)))
            }
            Poll::Ready(Err(err)) => Poll::Ready(Some(Err(err))),
            Poll::Pending => Poll::Pending,
        }
    }

    /// Returns the underlying [`Signals`].
    pub fn into_inner(self) -> Signals {
        let mut this = ManuallyDrop::new(self);
        let ReceiveSignals { signals, state } = &mut *this;
        // SAFETY: not using state any more.
        unsafe {
            state.drop(signals.fd.sq());
            ptr::drop_in_place(state);
        }
        // SAFETY: we're not dropping self (due to the ManuallyDrop, so signals
        // is safe to return.
        unsafe { ptr::read(signals) }
    }
}

#[cfg(feature = "nightly")]
impl std::async_iter::AsyncIterator for ReceiveSignals {
    type Item = io::Result<SignalInfo>;

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

/// Process signal information.
///
/// See [`Signals::receive`].
#[repr(transparent)]
pub struct SignalInfo(pub(crate) crate::sys::process::SignalInfo);

impl SignalInfo {
    /// Signal send.
    pub fn signal(&self) -> Signal {
        crate::sys::process::signal(&self.0)
    }

    /// Process id of the sender.
    #[cfg(any(target_os = "android", target_os = "linux"))]
    pub fn pid(&self) -> u32 {
        crate::sys::process::pid(&self.0)
    }

    /// Real user ID of the sender.
    #[cfg(any(target_os = "android", target_os = "linux"))]
    pub fn real_user_id(&self) -> u32 {
        crate::sys::process::real_user_id(&self.0)
    }
}

impl fmt::Debug for SignalInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut f = f.debug_struct("SignalInfo");
        f.field("signal", &self.signal());
        #[cfg(any(target_os = "android", target_os = "linux"))]
        f.field("pid", &self.pid());
        #[cfg(any(target_os = "android", target_os = "linux"))]
        f.field("real_user_id", &self.real_user_id());
        f.finish()
    }
}

/// Set of signals.
#[repr(transparent)]
pub struct SignalSet(pub(crate) libc::sigset_t);

impl SignalSet {
    /// Create an empty set.
    #[doc(alias = "sigemptyset")]
    pub fn empty() -> io::Result<SignalSet> {
        let mut set: MaybeUninit<libc::sigset_t> = MaybeUninit::uninit();
        syscall!(sigemptyset(set.as_mut_ptr()))?;
        // SAFETY: initialised the set in the call to `sigemptyset`.
        Ok(SignalSet(unsafe { set.assume_init() }))
    }

    /// Create a set with all signals.
    #[doc(alias = "sigfillset")]
    pub fn all() -> io::Result<SignalSet> {
        let mut set: MaybeUninit<libc::sigset_t> = MaybeUninit::uninit();
        syscall!(sigfillset(set.as_mut_ptr()))?;
        // SAFETY: initialised the set in the call to `sigfillset`.
        Ok(SignalSet(unsafe { set.assume_init() }))
    }

    /// Returns true if `signal` is contained in the set.
    #[doc(alias = "sigismember")]
    pub fn contains(&self, signal: Signal) -> bool {
        // SAFETY: we ensure the pointer to the signal set is valid.
        unsafe { libc::sigismember(&raw const self.0, signal.0) == 1 }
    }

    /// Add `signal` to the set.
    #[doc(alias = "sigaddset")]
    pub fn add(&mut self, signal: Signal) -> io::Result<()> {
        syscall!(sigaddset(&raw mut self.0, signal.0)).map(|_| ())
    }
}

impl fmt::Debug for SignalSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let signals = Signal::ALL_VALUES
            .iter()
            .filter(|signal| self.contains(**signal));
        f.debug_set().entries(signals).finish()
    }
}

new_flag!(
    /// Process signal.
    #[doc = man_link!(signal(7))]
    pub struct Signal(i32) {
        /// Hangup.
        HUP = libc::SIGHUP,
        /// Interrupt.
        INTERRUPT = libc::SIGINT,
        /// Quit.
        QUIT = libc::SIGQUIT,
        /// Illegal instruction.
        ILLEGAL = libc::SIGILL,
        /// Trace/breakpoint trap.
        TRAP = libc::SIGTRAP,
        /// Abort.
        ABORT = libc::SIGABRT,
        /// IOT trap.
        IOT = libc::SIGIOT,
        /// Bus error (bad memory access).
        BUS = libc::SIGBUS,
        /// Erroneous arithmetic operation.
        FP_ERROR = libc::SIGFPE,
        /// User-defined 1.
        USER1 = libc::SIGUSR1,
        /// User-defined 2.
        USER2 = libc::SIGUSR2,
        /// Invalid memory reference.
        SEG_VAULT = libc::SIGSEGV,
        /// Broken pipe.
        PIPE = libc::SIGPIPE,
        /// Timer.
        ALARM = libc::SIGALRM,
        /// Termination.
        TERMINATION = libc::SIGTERM,
        /// Child stopped, terminated or continued.
        CHILD = libc::SIGCHLD,
        /// Continue if stopped.
        CONTINUE = libc::SIGCONT,
        /// Stop typed at terminal
        TERM_STOP = libc::SIGTSTP,
        /// Terminal input for background process.
        TTY_IN = libc::SIGTTIN,
        /// Terminal output for background process.
        TTY_OUT = libc::SIGTTOU,
        /// Urgent condition on socket.
        URGENT = libc::SIGURG,
        /// CPU time limit exceeded.
        EXCEEDED_CPU = libc::SIGXCPU,
        /// File size limit exceeded.
        EXCEEDED_FILE_SIZE = libc::SIGXFSZ,
        /// Virtual alarm clock.
        VIRTUAL_ALARM = libc::SIGVTALRM,
        /// Profiling timer expired.
        PROFILE_ALARM = libc::SIGPROF,
        /// Window resize signal.
        WINDOW_RESIZE = libc::SIGWINCH,
        /// I/O now possible.
        IO = libc::SIGIO,
        /// Pollable event.
        ///
        /// NOTE: same value as [`Signal::IO`].
        #[cfg(any(target_os = "android", target_os = "linux"))]
        POLL = libc::SIGPOLL,
        /// Power failure.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        PWR = libc::SIGPWR,
        /// Bad system call.
        SYS = libc::SIGSYS,
    }
);

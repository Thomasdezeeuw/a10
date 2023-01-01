//! Module for handling signals.
//!
//! See the [`Signals`] documentation.

use std::mem::{size_of, MaybeUninit};
use std::{fmt, io, ptr};

use log::error;

use crate::op::{op_future, NO_OFFSET};
use crate::{libc, AsyncFd, SubmissionQueue};

/// Notification of process signals.
///
/// # Multithreaded process
///
/// For `Signals` to function correctly in multithreaded processes it must be
/// created on the main thread **before** spawning any threads. This is due to
/// an implementation detail where the spawned threads must inherit various
/// signal related thread options from the parent thread.
///
/// Any threads spawned before calling `Signals::new` will experience the
/// default process signals behaviour, i.e. sending it a signal will interrupt
/// or stop it.
///
/// # Implementation Notes
///
/// This will block all signals in the signal set given when creating `Signals`,
/// using [`pthread_sigmask(3)`]. This means that the thread in which `Signals`
/// was created (and it's children) is not interrupted, or in any way notified
/// of signal until [`Signals::receive`] called (and the returned [`Future`]
/// polled to completion). Under the hood [`Signals`] is just a wrapper around
/// [`signalfd(2)`].
///
/// [`pthread_sigmask(3)`]: https://man7.org/linux/man-pages/man3/pthread_sigmask.3.html
/// [`Future`]: std::future::Future
/// [`signalfd(2)`]: http://man7.org/linux/man-pages/man2/signalfd.2.html
pub struct Signals {
    /// `signalfd(2)` file descriptor.
    fd: AsyncFd,
    /// All signals this is listening for, used in resetting the signal handlers.
    signals: libc::sigset_t,
}

impl Signals {
    /// Create a new signal notifier.
    pub fn new(sq: SubmissionQueue, signals: libc::sigset_t) -> io::Result<Signals> {
        let fd = libc::syscall!(signalfd(-1, &signals, libc::SFD_CLOEXEC))?;
        let fd = AsyncFd { fd, sq };
        // Block all `signals` as we're going to read them from the signalfd.
        sigprocmask(libc::SIG_BLOCK, &signals)?;
        Ok(Signals { fd, signals })
    }

    /// Receive a signal.
    pub fn receive<'fd>(&'fd self) -> Receive<'fd> {
        let info = Box::new_uninit();
        Receive::new(&self.fd, info, ())
    }
}

// Receive.
op_future! {
    fn Signals::receive -> Box<libc::signalfd_siginfo>,
    struct Receive<'fd> {
        /// Buffer to write into, needs to stay in memory so the kernel can
        /// access it safely.
        info: Box<MaybeUninit<libc::signalfd_siginfo>>,
    },
    setup_state: _unused: (),
    setup: |submission, fd, (info,), _unused| unsafe {
        let ptr = (**info).as_mut_ptr().cast();
        submission.read_at(fd.fd, ptr, size_of::<libc::signalfd_siginfo>() as u32, NO_OFFSET);
    },
    map_result: |this, (info,), n| {
        debug_assert_eq!(n, size_of::<libc::signalfd_siginfo>() as i32);
        // SAFETY: the kernel initialised the info allocation for us as part of
        // the read call.
        Ok(unsafe { Box::<MaybeUninit<libc::signalfd_siginfo>>::assume_init(info) })
    },
}

impl fmt::Debug for Signals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Signals").field("fd", &self.fd).finish()
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
        // Reverse the blocking of signals.
        if let Err(err) = sigprocmask(libc::SIG_UNBLOCK, &self.signals) {
            error!("error unblocking signals: {err}");
        }
    }
}

fn sigprocmask(how: libc::c_int, set: &libc::sigset_t) -> io::Result<()> {
    libc::syscall!(pthread_sigmask(how, set, ptr::null_mut()))?;
    Ok(())
}

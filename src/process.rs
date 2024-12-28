//! Process handling.
//!
//! In this module process signal handling is also supported. For that See the
//! documentation of [`Signals`].

use std::cell::UnsafeCell;
use std::process::Child;
use std::{io, mem};

use crate::op::{operation, Operation};
use crate::{man_link, sys, SubmissionQueue};

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
    let info = unsafe { Box::new(UnsafeCell::new(mem::zeroed())) };
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
    pub struct WaitId(sys::process::WaitIdOp) -> io::Result<Box<libc::signalfd_siginfo>>;
);

// SAFETY: `!Sync` due to `UnsafeCell`, but it's actually `Sync`.
unsafe impl Sync for WaitId {}
unsafe impl Send for WaitId {}

/* TODO(port): add back Cancel support.
impl Cancel for WaitId {
    fn try_cancel(&mut self) -> CancelResult {
        self.state.try_cancel(&self.sq)
    }

    fn cancel(&mut self) -> CancelOp {
        self.state.cancel(&self.sq)
    }
}
*/

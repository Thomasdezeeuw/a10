//! Process handling.

use std::future::Future;
use std::io;
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::process::Child;
use std::task::{self, Poll};

use crate::cancel::{Cancel, CancelOp, CancelResult};
use crate::op::{poll_state, OpState};
use crate::SubmissionQueue;

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

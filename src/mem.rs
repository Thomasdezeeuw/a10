//! Asynchronous memory operations.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{self, Poll};

use crate::op::{poll_state, OpState};
use crate::{libc, SubmissionQueue};

/// Give advice about use of memory.
///
/// Give advice or directions to the kernel about the address range beginning at
/// address `addr` and with size `length` bytes. In most cases, the goal of such
/// advice is to improve system or application performance.
#[doc(alias = "posix_madvise")]
pub const fn advise(
    sq: SubmissionQueue,
    address: *mut (),
    length: u32,
    advice: libc::c_int,
) -> Advise {
    Advise {
        sq,
        state: OpState::NotStarted((address, length, advice)),
    }
}

/// [`Future`] behind [`advise`].
#[derive(Debug)]
#[must_use = "`Future`s do nothing unless polled"]
pub struct Advise {
    sq: SubmissionQueue,
    state: OpState<(*mut (), u32, libc::c_int)>,
}

impl Future for Advise {
    type Output = io::Result<()>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        #[rustfmt::skip] // Rustfmt makes a right mess of this.
        let op_index = poll_state!(
            Advise, self.state, self.sq, ctx,
            |submission, (address, length, advice)| unsafe { submission.madvise(address, length, advice); }
        );

        match self.sq.poll_op(ctx, op_index) {
            Poll::Ready(result) => {
                self.state = OpState::Done;
                match result {
                    Ok((_, res)) => Poll::Ready(Ok(debug_assert!(res == 0))),
                    Err(err) => Poll::Ready(Err(err)),
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

unsafe impl Sync for Advise {}
unsafe impl Send for Advise {}

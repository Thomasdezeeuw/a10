//! Memory operations.

use std::io;

use crate::op::{operation, Operation};
use crate::{man_link, sys, SubmissionQueue};

/// Give advice about use of memory.
///
/// Give advice or directions to the kernel about the address range beginning at
/// address `addr` and with size `length` bytes. In most cases, the goal of such
/// advice is to improve system or application performance.
#[doc = man_link!(madvise(2))]
#[doc(alias = "madvise")]
#[doc(alias = "posix_madvise")]
pub const fn advise(
    sq: SubmissionQueue,
    address: *mut (),
    length: u32,
    advice: libc::c_int,
) -> Advise {
    Advise(Operation::new(sq, (), (address, length, advice)))
}

// SAFETY: `!Send` due to address, but the future is `Send`.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Sync for Advise {}
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for Advise {}

operation!(
    /// [`Future`] behind [`advise`].
    pub struct Advise(sys::mem::AdviseOp) -> io::Result<()>;
);

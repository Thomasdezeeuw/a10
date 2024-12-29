//! Networking.
//!
//! To create a new socket ([`AsyncFd`]) use the [`socket`] function, which
//! issues a non-blocking `socket(2)` call.

use std::io;

use crate::fd::{AsyncFd, Descriptor};
use crate::op::{operation, Operation};
use crate::{man_link, sys, SubmissionQueue};

/// Creates a new socket.
#[doc = man_link!(socket(2))]
pub const fn socket<D: Descriptor>(
    sq: SubmissionQueue,
    domain: libc::c_int,
    r#type: libc::c_int,
    protocol: libc::c_int,
    flags: libc::c_int,
) -> Socket<D> {
    Socket(Operation::new(sq, (), (domain, r#type, protocol, flags)))
}

operation!(
    /// [`Future`] behind [`socket`].
    ///
    /// If you're looking for a socket type, there is none, see [`AsyncFd`].
    pub struct Socket<D: Descriptor>(sys::net::SocketOp<D>) -> io::Result<AsyncFd<D>>;
);

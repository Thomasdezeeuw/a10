//! Unix pipes.
//!
//! To create a new pipe use the [`pipe`] function. It will return two
//! [`AsyncFd`], the sending and receiving side.

use std::io;

use crate::fd::{self, AsyncFd};
use crate::op::{operation, Operation};
use crate::{man_link, sys, SubmissionQueue};

/// Create a new Unix pipe.
///
/// This is a wrapper around Unix's `pipe(2)` system call and can be used as
/// inter-process or thread communication channel.
///
/// This channel may be created before forking the process and then one end used
/// in each process, e.g. the parent process has the sending end to send
/// commands to the child process.
///
/// ```
/// # async fn new_pipe() -> io::Result<()> {
/// let flags = 0; // NOTE: O_CLOEXEC is already set.
/// // Creating a new pipe using file descriptors.
/// let [receiver, sender] = pipe(sq, flags)?;
///
/// // Using direct descriptors.
/// let [receiver, sender] = pipe(sq, flags).kind(fd::Kind::Direct)?;
/// # }
/// ```
#[doc = man_link!(pipe(2))]
pub fn pipe(sq: SubmissionQueue, flags: libc::c_int) -> Pipe {
    let resources = (Box::new([-1, -1]), fd::Kind::File);
    Pipe(Operation::new(sq, resources, flags))
}

operation!(
    /// [`Future`] behind [`pipe`].
    pub struct Pipe(sys::pipe::PipeOp) -> io::Result<[AsyncFd; 2]>;
);

impl Pipe {
    /// Set the kind of descriptor to use.
    ///
    /// Defaults to a regular [`fd::Kind::File`] descriptor.
    pub fn kind(mut self, kind: fd::Kind) -> Self {
        if let Some(resources) = self.0.update_args() {
            resources.1 = kind;
        }
        self
    }
}

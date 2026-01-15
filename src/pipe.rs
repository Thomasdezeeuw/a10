//! Unix pipes.
//!
//! To create a new pipe use the [`pipe`] function. It will return two
//! [`AsyncFd`]s, the sending and receiving side.

use std::io;

use crate::fd::{self, AsyncFd};
use crate::op::{OpState, operation};
use crate::{SubmissionQueue, man_link, new_flag, sys};

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
/// # use std::io;
/// # use a10::pipe::pipe;
/// # use a10::fd;
/// # async fn new_pipe(sq: &a10::SubmissionQueue) -> io::Result<()> {
/// // Creating a new pipe using file descriptors.
/// let [receiver, sender] = pipe(sq.clone(), None).await?;
///
/// // Using direct descriptors.
/// let [receiver, sender] = pipe(sq.clone(), None).kind(fd::Kind::Direct).await?;
/// # Ok(())
/// # }
/// ```
#[doc = man_link!(pipe(2))]
pub fn pipe(sq: SubmissionQueue, flags: Option<PipeFlag>) -> Pipe {
    let flags = flags.unwrap_or(PipeFlag(0));
    let resources = ([-1, -1], fd::Kind::File);
    Pipe::new(sq, resources, flags)
}

new_flag!(
    /// Flags to [`pipe`].
    pub struct PipeFlag(u32) {
        /// Create a pipe that performs I/O in "packet" mode.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        DIRECT = libc::O_DIRECT,
    }
);

operation!(
    /// [`Future`] behind [`pipe`].
    pub struct Pipe(sys::pipe::PipeOp) -> io::Result<[AsyncFd; 2]>;
);

impl Pipe {
    /// Set the kind of descriptor to use.
    ///
    /// Defaults to a regular [`File`] descriptor.
    ///
    /// [`File`]: fd::Kind::File
    pub fn kind(mut self, kind: fd::Kind) -> Self {
        if let Some(resources) = self.state.resources_mut() {
            resources.1 = kind;
        }
        self
    }
}

//! Unix pipes.
//!
//! To create a new pipe use the [`pipe`] function. It will return two
//! [`AsyncFd`]s, the sending and receiving side.
//!
//! If you're looking for a synchronous version of the `pipe` function (for
//! easier creation in non-async setup code) see [`sync_pipe`] and
//! [`sync_pipe2`].

use std::os::fd::{FromRawFd, OwnedFd};
use std::{io, ptr};

use crate::fd::{self, AsyncFd};
use crate::op::{OpState, operation};
use crate::{SubmissionQueue, man_link, new_flag, sys, syscall};

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
/// let [receiver, sender] = pipe(sq.clone()).await?;
///
/// // Using direct descriptors.
/// #[cfg(any(target_os = "android", target_os = "linux"))]
/// let [receiver, sender] = pipe(sq.clone()).kind(fd::Kind::Direct).await?;
/// # Ok(())
/// # }
/// ```
///
/// # Implementation Notes
///
/// On Linux kernels older than 6.16 io_uring doesn't support the creation of a
/// pipe. If the creation fails because of this, we fallback to a synchronous system call.
/// This does mean that the returned fds are regulator file descriptors
/// ([`fd::Kind::File`]), even if [`Pipe::kind`] was used to request direct
/// descriptors.
#[doc = man_link!(pipe(2))]
pub fn pipe(sq: SubmissionQueue) -> Pipe {
    let resources = ([-1, -1], fd::Kind::File);
    Pipe::new(sq, resources, PipeFlag(0))
}

new_flag!(
    /// Pipe flags.
    ///
    /// Set using [`Pipe::flags`].
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

    /// Set the `flags`.
    pub fn flags(mut self, flags: PipeFlag) -> Self {
        if let Some(f) = self.state.args_mut() {
            *f = flags;
        }
        self
    }
}

/// Synchronous version of [`pipe`].
///
/// See [`sync_pipe2`] for more.
pub fn sync_pipe() -> io::Result<[OwnedFd; 2]> {
    sync_pipe2(PipeFlag(0))
}

/// Synchronous version of [`pipe`].
///
/// # Notes
///
/// The returned fd is setup such that it can be converted into an [`AsyncFd`]
/// without any further changes required to it.
pub fn sync_pipe2(flags: PipeFlag) -> io::Result<[OwnedFd; 2]> {
    let mut fds = [-1, -1];

    let flags = flags.0.cast_signed() | libc::O_CLOEXEC;
    // NOTE: io_uring doesn't need NON_BLOCK.
    #[cfg(any(
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
    ))]
    let flags = flags | libc::O_NONBLOCK;

    #[cfg(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "linux",
        target_os = "netbsd",
        target_os = "openbsd",
    ))]
    syscall!(pipe2(ptr::from_mut(&mut fds).cast(), flags))?;
    #[cfg(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "tvos",
        target_os = "visionos",
        target_os = "watchos",
    ))]
    syscall!(pipe(ptr::from_mut(&mut fds).cast()))?;

    // SAFETY: created the pipe fds above.
    let owned_fds = unsafe { [OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])] };

    // OS that don't support pipe2, we set NONBLOCK and CLOEXEC after opening.
    #[cfg(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "tvos",
        target_os = "visionos",
        target_os = "watchos",
    ))]
    {
        syscall!(fcntl(fds[0], libc::F_SETFL, libc::O_NONBLOCK))?;
        syscall!(fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC))?;
        syscall!(fcntl(fds[1], libc::F_SETFL, libc::O_NONBLOCK))?;
        syscall!(fcntl(fds[1], libc::F_SETFD, libc::FD_CLOEXEC))?;
        let _ = flags;
    }

    Ok(owned_fds)
}

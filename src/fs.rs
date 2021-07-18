//! Asynchronous filesystem manipulation operations.

use std::ffi::CString;
use std::io;
use std::mem::{forget as leak, take};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::io::RawFd;
use std::path::PathBuf;
use std::task::Poll;

use crate::op::SharedOperationState;
use crate::{libc, QueueFull, SubmissionQueue};

/// A reference to an open file on the filesystem.
///
/// See [`std::fs::File`] for more documentation.
#[derive(Debug)]
pub struct File {
    fd: RawFd,
    state: SharedOperationState,
}

impl File {
    /// Open `path` for reading.
    pub fn open(queue: SubmissionQueue, path: PathBuf) -> Result<Open, QueueFull> {
        let path = path.into_os_string().into_vec();
        let path = unsafe { CString::from_vec_unchecked(path) };

        let state = SharedOperationState::new(queue);
        state.start(|submission| unsafe {
            let flags = libc::O_RDONLY | libc::O_CLOEXEC;
            submission.open_at(libc::AT_FDCWD, path.as_ptr(), flags, 0)
        })?;

        Ok(Open {
            path,
            state: Some(state),
        })
    }

    /// Read from this file into `buf`.
    ///
    /// # Notes
    ///
    /// This leave the current contents of `buf` untouched and only uses the
    /// spare capacity.
    pub fn read<'f>(&'f self, mut buf: Vec<u8>) -> Result<Read<'f>, QueueFull> {
        self.state
            .start(|submission| unsafe { submission.read(self.fd, buf.spare_capacity_mut()) })?;

        Ok(Read {
            buf: Some(buf),
            file: &self,
        })
    }
}

impl Drop for File {
    fn drop(&mut self) {
        let result = self
            .state
            // TODO: check if no operation is in progress.
            .start(|submission| unsafe { submission.close_fd(self.fd) });
        if let Err(err) = result {
            log::error!("error closing file: {}", err);
        }
    }
}

/// [`Future`] to open a [`File`].
#[derive(Debug)]
pub struct Open {
    /// Path used to open the file, need to stay in memory so the kernel can
    /// access it safely.
    path: CString,
    state: Option<SharedOperationState>,
}

impl Open {
    // TODO: replace with `Future` impl.
    pub fn check(&mut self) -> Poll<io::Result<File>> {
        let result = self.state.as_mut().unwrap().take_result();
        match result {
            None => Poll::Pending,
            Some(Ok(fd)) => Poll::Ready(Ok(File {
                fd,
                state: self.state.take().unwrap(),
            })),
            Some(Err(err)) => Poll::Ready(Err(err)),
        }
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        if self.state.is_some() {
            let path = take(&mut self.path);
            log::debug!("dropped `a10::fs::Open` before completion, leaking path");
            leak(path);
        }
    }
}

/// [`Future`] to read from a [`File`].
#[derive(Debug)]
pub struct Read<'f> {
    /// Buffer to write into, need to stay in memory so the kernel can access it
    /// safely.
    buf: Option<Vec<u8>>,
    file: &'f File,
}

impl<'f> Read<'f> {
    // TODO: replace with `Future` impl.
    pub fn check(&mut self) -> Poll<io::Result<Vec<u8>>> {
        let result = self.file.state.take_result();
        match result {
            None => Poll::Pending,
            Some(Ok(n)) => Poll::Ready(Ok({
                let mut buf = self.buf.take().unwrap();
                unsafe { buf.set_len(buf.len() + n as usize) };
                buf
            })),
            Some(Err(err)) => Poll::Ready(Err(err)),
        }
    }
}

impl<'f> Drop for Read<'f> {
    fn drop(&mut self) {
        if let Some(buf) = take(&mut self.buf) {
            log::debug!("dropped `a10::fs::Read` before completion, leaking buffer");
            leak(buf);
        }
    }
}

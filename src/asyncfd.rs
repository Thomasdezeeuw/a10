//! Module with [`AsyncFd`].

use std::os::unix::io::RawFd;

use crate::extract::Extractor;
use crate::op::{SharedOperationState, NO_OFFSET};
use crate::QueueFull;

/// An open file descriptor.
///
/// All functions on `AsyncFd` are asynchronous and return a [`Future`].
///
/// [`Future`]: std::future::Future
#[derive(Debug)]
pub struct AsyncFd {
    pub(crate) fd: RawFd,
    pub(crate) state: SharedOperationState,
}

impl Drop for AsyncFd {
    fn drop(&mut self) {
        let result = self
            .state
            .start(|submission| unsafe { submission.close_fd(self.fd) });
        if let Err(err) = result {
            log::error!("error closing fd: {}", err);
        }
    }
}

/// Generic system calls.
impl AsyncFd {
    /// Read from this fd into `buf`.
    ///
    /// # Notes
    ///
    /// This leave the current contents of `buf` untouched and only uses the
    /// spare capacity.
    pub fn read<'fd>(&'fd self, buf: Vec<u8>) -> Result<Read<'fd>, QueueFull> {
        self.read_at(buf, NO_OFFSET)
    }

    /// Read from this fd into `buf` starting at `offset`.
    ///
    /// The current file cursor is not affected by this function. This means
    /// that a call `read_at(buf, 1024)` with a buffer of 1kb will **not**
    /// continue reading at 2kb in the next call to `read`.
    ///
    /// # Notes
    ///
    /// This leave the current contents of `buf` untouched and only uses the
    /// spare capacity.
    pub fn read_at<'fd>(&'fd self, mut buf: Vec<u8>, offset: u64) -> Result<Read<'fd>, QueueFull> {
        self.state.start(|submission| unsafe {
            submission.read_at(self.fd, buf.spare_capacity_mut(), offset);
        })?;

        Ok(Read {
            buf: Some(buf),
            fd: self,
        })
    }

    /// Write `buf` to this file.
    pub fn write<'fd>(&'fd self, buf: Vec<u8>) -> Result<Write<'fd>, QueueFull> {
        self.write_at(buf, NO_OFFSET)
    }

    /// Write `buf` to this file.
    ///
    /// The current file cursor is not affected by this function.
    pub fn write_at<'fd>(&'fd self, buf: Vec<u8>, offset: u64) -> Result<Write<'fd>, QueueFull> {
        self.state
            .start(|submission| unsafe { submission.write_at(self.fd, &buf, offset) })?;

        Ok(Write {
            buf: Some(buf),
            fd: self,
        })
    }
}

// Read.
op_future! {
    fn AsyncFd::read -> Vec<u8>,
    struct Read<'fd> {
        /// Buffer to write into, needs to stay in memory so the kernel can
        /// access it safely.
        buf: Option<Vec<u8>>, "dropped `a10::Read` before completion, leaking buffer",
    },
    |this, n| {
        let mut buf = this.buf.take().unwrap();
        unsafe { buf.set_len(buf.len() + n as usize) };
        Ok(buf)
    },
}

// Write.
op_future! {
    fn AsyncFd::write -> usize,
    struct Write<'fd> {
        /// Buffer to read from, needs to stay in memory so the kernel can
        /// access it safely.
        buf: Option<Vec<u8>>, "dropped `a10::Write` before completion, leaking buffer",
    },
    |n| Ok(n as usize),
    extract: |this, n| -> (Vec<u8>, usize) {
        let buf = this.buf.take().unwrap();
        Ok((buf, n as usize))
    }
}

//! Module with [`AsyncFd`].

use std::future::Future;
use std::io;
use std::mem::{forget as leak, take};
use std::os::unix::io::RawFd;
use std::pin::Pin;
use std::task::{self, Poll};

use crate::extract::Extractor;
use crate::op::{SharedOperationState, NO_OFFSET};
use crate::{Extract, QueueFull};

/// An open file descriptor.
#[derive(Debug)]
pub struct AsyncFd {
    fd: RawFd,
    state: SharedOperationState,
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
    pub fn read<'f>(&'f self, buf: Vec<u8>) -> Result<Read<'f>, QueueFull> {
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
    pub fn read_at<'f>(&'f self, mut buf: Vec<u8>, offset: u64) -> Result<Read<'f>, QueueFull> {
        self.state.start(|submission| unsafe {
            submission.read_at(self.fd, buf.spare_capacity_mut(), offset);
        })?;

        Ok(Read {
            buf: Some(buf),
            fd: self,
        })
    }

    /// Write `buf` to this file.
    pub fn write<'f>(&'f self, buf: Vec<u8>) -> Result<Write<'f>, QueueFull> {
        self.write_at(buf, NO_OFFSET)
    }

    /// Write `buf` to this file.
    ///
    /// The current file cursor is not affected by this function.
    pub fn write_at<'f>(&'f self, buf: Vec<u8>, offset: u64) -> Result<Write<'f>, QueueFull> {
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

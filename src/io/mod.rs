//! Type definitions for I/O functionality.
//!
//! The main types of this module are the [`Buf`] and [`BufMut`] traits, which
//! define the requirements on buffers using the I/O system calls on an file
//! descriptor ([`AsyncFd`]). Additionally the [`BufSlice`] and [`BufMutSlice`]
//! traits existing to define the behaviour of buffers in vectored I/O.
//!
//! A specialised io_uring-only read buffer pool implementation exists in
//! [`ReadBufPool`], which is a buffer pool managed by the kernel when making
//! `read(2)`-like system calls.
//!
//! Finally we have the [`stdin`], [`stdout`] and [`stderr`] functions to create
//! `AsyncFd`s for standard in, out and error respectively.

mod traits;

pub use traits::{Buf, BufMut, BufMutSlice, BufSlice};

// Re-export so we don't have to worry about import `std::io` and `crate::io`.
pub(crate) use std::io::*;

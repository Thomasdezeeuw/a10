//! Type definitions for I/O functionality.
//!
//! The main types of this module are the [`Buf`] and [`BufMut`] traits, which
//! define the requirements on buffers using the I/O system calls on an file
//! descriptor ([`AsyncFd`]).
//!
//! A speciailised read buffer pool implementation exists in [`ReadBufPool`],
//! which is a buffer pool managed by the kernel when making `read(2)`-like
//! system calls.
//!
//! Finally we have the [`stdin`], [`stdout`] and [`stderr`] functions to create
//! `AsyncFd`s for standard in, out and error respectively.

use std::future::Future;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::os::unix::io::RawFd;
use std::pin::Pin;
use std::task::{self, Poll};
use std::{io, ptr};

use crate::op::{op_future, poll_state, OpState, NO_OFFSET};
use crate::{libc, AsyncFd, SubmissionQueue};

mod read_buf;
pub(crate) use read_buf::{BufGroupId, BufIdx};
pub use read_buf::{ReadBuf, ReadBufPool};

// Re-export so we don't have to worry about import `std::io` and `crate::io`.
pub(crate) use std::io::*;

macro_rules! stdio {
    (
        $fn: ident () -> $name: ident, $fd: expr
    ) => {
        #[doc = concat!("Create a new `", stringify!($name), "`.\n\n")]
        pub const fn $fn(sq: $crate::SubmissionQueue) -> $name {
            $name(std::mem::ManuallyDrop::new($crate::AsyncFd {
                fd: libc::STDOUT_FILENO as std::os::unix::io::RawFd,
                sq,
            }))
        }

        #[doc = concat!(
                    "An [`AsyncFd`] for ", stringify!($fn), ".\n\n",
                    "# Notes\n\n",
                    "This directly writes to the raw file descriptor, which means it's not buffered and will not flush anything buffered by the standard library.\n\n",
                    "When the returned type is dropped it will not close ", stringify!($fn), ".",
                )]
        pub struct $name(std::mem::ManuallyDrop<$crate::AsyncFd>);

        impl std::ops::Deref for $name {
            type Target = $crate::AsyncFd;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
    };
}

stdio!(stdin() -> Stdin, libc::STDIN_FILENO);
stdio!(stdout() -> Stdout, libc::STDOUT_FILENO);
stdio!(stderr() -> Stderr, libc::STDERR_FILENO);

/// I/O system calls.
impl AsyncFd {
    /// Read from this fd into `buf`.
    pub const fn read<'fd, B>(&'fd self, buf: B) -> Read<'fd, B>
    where
        B: BufMut,
    {
        self.read_at(buf, NO_OFFSET)
    }

    /// Read from this fd into `buf` starting at `offset`.
    ///
    /// The current file cursor is not affected by this function. This means
    /// that a call `read_at(buf, 1024)` with a buffer of 1kb will **not**
    /// continue reading at 2kb in the next call to `read`.
    pub const fn read_at<'fd, B>(&'fd self, buf: B, offset: u64) -> Read<'fd, B>
    where
        B: BufMut,
    {
        Read::new(self, buf, offset)
    }

    /// Write `buf` to this file.
    pub const fn write<'fd, B>(&'fd self, buf: B) -> Write<'fd, B>
    where
        B: Buf,
    {
        self.write_at(buf, NO_OFFSET)
    }

    /// Write `buf` to this file.
    ///
    /// The current file cursor is not affected by this function.
    pub const fn write_at<'fd, B>(&'fd self, buf: B, offset: u64) -> Write<'fd, B>
    where
        B: Buf,
    {
        Write::new(self, buf, offset)
    }

    /// Attempt to cancel an in progress operation.
    ///
    /// If the previous I/O operation was succesfully canceled this returns
    /// `Ok(())` and the canceled operation will return `ECANCELED` to indicate
    /// it was canceled.
    ///
    /// If no previous operation was found, for example if it was already
    /// completed, this will return `io::ErrorKind::NotFound`.
    ///
    /// In general, requests that are interruptible (like socket IO) will get
    /// canceled, while disk IO requests cannot be canceled if already started.
    ///
    /// # Notes
    ///
    /// Due to the lazyness of [`Future`]s it's possible that this will return
    /// `NotFound` if the previous operation was never polled.
    pub const fn cancel_previous<'fd>(&'fd self) -> Cancel<'fd> {
        Cancel {
            fd: self,
            state: OpState::NotStarted(0),
        }
    }

    /// Same as [`AsyncFd::cancel_previous`], but attempts to cancel all
    /// operations.
    pub const fn cancel_all<'fd>(&'fd self) -> Cancel<'fd> {
        Cancel {
            fd: self,
            state: OpState::NotStarted(libc::IORING_ASYNC_CANCEL_ALL),
        }
    }

    /// Explicitly close the file descriptor.
    ///
    /// # Notes
    ///
    /// This happens automatically on drop, this can be used to get a possible
    /// error.
    pub fn close(self) -> Close {
        // We deconstruct `self` without dropping it to avoid closing the fd
        // twice.
        let this = ManuallyDrop::new(self);
        // SAFETY: this is safe because we're ensure the pointers are valid and
        // not touching `this` after reading the fields.
        let fd = unsafe { ptr::read(&this.fd) };
        let sq = unsafe { ptr::read(&this.sq) };

        Close {
            sq,
            state: OpState::NotStarted(fd),
        }
    }
}

// Read.
op_future! {
    fn AsyncFd::read -> B,
    struct Read<'fd, B: BufMut> {
        /// Buffer to write into, needs to stay in memory so the kernel can
        /// access it safely.
        buf: B,
    },
    setup_state: offset: u64,
    setup: |submission, fd, (buf,), offset| unsafe {
        let (ptr, len) = buf.parts();
        submission.read_at(fd.fd, ptr, len, offset);
        if let Some(buf_group) = buf.buffer_group() {
            submission.set_buffer_select(buf_group.0);
        }
    },
    map_result: |this, (mut buf,), buf_idx, n| {
        // SAFETY: the kernel initialised the bytes for us as part of the read
        // call.
        unsafe { buf.buffer_init(BufIdx(buf_idx), n as u32) };
        Ok(buf)
    },
}

// Write.
op_future! {
    fn AsyncFd::write -> usize,
    struct Write<'fd, B: Buf> {
        /// Buffer to read from, needs to stay in memory so the kernel can
        /// access it safely.
        buf: B,
    },
    setup_state: offset: u64,
    setup: |submission, fd, (buf,), offset| unsafe {
        let (ptr, len) = buf.parts();
        submission.write_at(fd.fd, ptr, len, offset);
    },
    map_result: |n| Ok(n as usize),
    extract: |this, (buf,), n| -> (B, usize) {
        Ok((buf, n as usize))
    },
}

/// [`Future`] behind [`AsyncFd::cancel_previous`] and [`AsyncFd::cancel_all`].
#[derive(Debug)]
pub struct Cancel<'fd> {
    fd: &'fd AsyncFd,
    state: OpState<u32>,
}

impl<'fd> Future for Cancel<'fd> {
    type Output = io::Result<()>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let op_index = poll_state!(Cancel, *self, ctx, |submission, fd, flags| unsafe {
            submission.cancel(fd.fd, flags);
        });

        match self.fd.sq.poll_op(ctx, op_index) {
            Poll::Ready(result) => {
                self.state = OpState::Done;
                match result {
                    Ok(_) => Poll::Ready(Ok(())),
                    Err(err) => Poll::Ready(Err(err)),
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// [`Future`] behind [`AsyncFd::close`].
#[derive(Debug)]
pub struct Close {
    sq: SubmissionQueue,
    state: OpState<RawFd>,
}

impl Future for Close {
    type Output = io::Result<()>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let op_index = poll_state!(Close, self.state, self.sq, ctx, |submission, fd| unsafe {
            submission.close(fd);
        });

        match self.sq.poll_op(ctx, op_index) {
            Poll::Ready(result) => {
                self.state = OpState::Done;
                match result {
                    Ok(_) => Poll::Ready(Ok(())),
                    Err(err) => Poll::Ready(Err(err)),
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Trait that defines the behaviour of buffers used in reading, which requires
/// mutable access.
///
/// # Safety
///
/// Unlike normal buffers the buffer implementations for A10 have additional
/// requirements.
///
/// If the operation (that uses this buffer) is not polled to completion, i.e.
/// the `Future` is dropped before it returns `Poll::Ready`, the kernel still
/// has access to the buffer and will still attempt to write into it. This means
/// that we must delay deallocation in such a way that the kernel will not write
/// into memory we don't have access to any more. This makes, for example, stack
/// based buffers unfit to implement `BufMut`. Because we can't delay the
/// deallocation once its dropped and the kernel will overwrite part of your
/// stack (where the buffer used to be)!
pub unsafe trait BufMut: 'static {
    /// Returns the writable buffer as pointer and length parts.
    ///
    /// # Safety
    ///
    /// Only initialised bytes may be written to the pointer returned. The
    /// pointer *may* point to uninitialised bytes, so reading from the pointer
    /// is UB.
    ///
    /// The implementation must ensure that the pointer is valid, i.e. not null
    /// and pointing to memory owned by the buffer. Furthermore it must ensure
    /// that the returned length is, in combination with the pointer, valid. In
    /// other words the memory the pointer and length are pointing to must be a
    /// valid memory address and owned by the buffer.
    ///
    /// Note that the above requirements are only required for implementations
    /// outside of A10. **This trait is unfit for external use!**
    ///
    /// # Why not a slice?
    ///
    /// Returning a slice `&[u8]` would prevent us to use unitialised bytes,
    /// meaning we have to zero the buffer before usage, not ideal for
    /// performance. So, naturally you would suggest `&[MaybeUninit<u8>]`,
    /// however that would prevent buffer types with only initialised bytes.
    /// Returning a slice with `MaybeUninit` to such as type would be unsound as
    /// it would allow the caller to write unitialised bytes without using
    /// `unsafe`.
    ///
    /// # Notes
    ///
    /// Most Rust API use a `usize` for length, but io_uring uses `u32`, hence
    /// we do also.
    unsafe fn parts(&mut self) -> (*mut u8, u32);

    /// Mark `n` bytes as initialised.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `n` bytes, starting at the pointer returned
    /// by [`BufMut::parts`], are initialised.
    unsafe fn set_init(&mut self, n: usize);

    /// Buffer group id, or `None` if it's not part of a buffer pool.
    ///
    /// Don't implement this.
    #[doc(hidden)]
    fn buffer_group(&self) -> Option<BufGroupId> {
        None
    }

    /// Mark `n` bytes as initialised in buffer with `idx`.
    ///
    /// Don't implement this.
    #[doc(hidden)]
    unsafe fn buffer_init(&mut self, idx: BufIdx, n: u32) {
        debug_assert!(idx.0 == 0);
        self.set_init(n as usize);
    }
}

/// The implementation for `Vec<u8>` only uses the unused capacity, so any bytes
/// already in the buffer will be untouched.
// SAFETY: `Vec<u8>` manages the allocation of the bytes, so as long as it's
// alive, so is the slice of bytes. When the `Vec`tor is leaked the allocation
// will also be leaked.
unsafe impl BufMut for Vec<u8> {
    unsafe fn parts(&mut self) -> (*mut u8, u32) {
        let slice = self.spare_capacity_mut();
        (slice.as_mut_ptr().cast(), slice.len() as u32)
    }

    unsafe fn set_init(&mut self, n: usize) {
        self.set_len(self.len() + n);
    }
}

/// Trait that defines the behaviour of buffers used in writing, which requires
/// read only access.
///
/// # Safety
///
/// Unlike normal buffers the buffer implementations for A10 have additional
/// requirements.
///
/// If the operation (that uses this buffer) is not polled to completion, i.e.
/// the `Future` is dropped before it returns `Poll::Ready`, the kernel still
/// has access to the buffer and will still attempt to read from it. This means
/// that we must delay deallocation in such a way that the kernel will not read
/// memory we don't have access to any more. This makes, for example, stack
/// based buffers unfit to implement `Buf`.  Because we can't delay the
/// deallocation once its dropped and the kernel will read part of your stack
/// (where the buffer used to be)! This would be a huge security risk.
pub unsafe trait Buf: 'static {
    /// Returns the reabable buffer as pointer and length parts.
    ///
    /// # Safety
    ///
    /// The implementation must ensure that the pointer is valid, i.e. not null
    /// and pointing to memory owned by the buffer. Furthermore it must ensure
    /// that the returned length is, in combination with the pointer, valid. In
    /// other words the memory the pointer and length are pointing to must be a
    /// valid memory address and owned by the buffer.
    ///
    /// # Notes
    ///
    /// Most Rust API use a `usize` for length, but io_uring uses `u32`, hence
    /// we do also.
    unsafe fn parts(&self) -> (*const u8, u32);
}

// SAFETY: `Vec<u8>` manages the allocation of the bytes, so as long as it's
// alive, so is the slice of bytes. When the `Vec`tor is leaked the allocation
// will also be leaked.
unsafe impl Buf for Vec<u8> {
    unsafe fn parts(&self) -> (*const u8, u32) {
        let slice = self.as_slice();
        (slice.as_ptr().cast(), slice.len() as u32)
    }
}

// SAFETY: `String` is just a `Vec<u8>`, see it's implementation for the safety
// reasoning.
unsafe impl Buf for String {
    unsafe fn parts(&self) -> (*const u8, u32) {
        let slice = self.as_bytes();
        (slice.as_ptr().cast(), slice.len() as u32)
    }
}

// SAFETY: because the reference has a `'static` lifetime we know the bytes
// can't be deallocated, so it's safe to implement `Buf`.
unsafe impl Buf for &'static [u8] {
    unsafe fn parts(&self) -> (*const u8, u32) {
        (self.as_ptr(), self.len() as u32)
    }
}

// SAFETY: because the reference has a `'static` lifetime we know the bytes
// can't be deallocated, so it's safe to implement `Buf`.
unsafe impl Buf for &'static str {
    unsafe fn parts(&self) -> (*const u8, u32) {
        (self.as_bytes().as_ptr(), self.len() as u32)
    }
}

/// Trait that defines the behaviour of buffers used in writing using vectored
/// I/O, which requires read only access.
///
/// # Safety
///
/// This has the same safety requirements as [`Buf`], but then for all buffers
/// used.
pub unsafe trait BufSlice<const N: usize>: 'static {
    /// Returns the reabable buffer as `iovec` structures.
    ///
    /// # Safety
    ///
    /// This has the same safety requirements as [`Buf::parts`], but then for
    /// all buffers used.
    unsafe fn as_iovec(&self) -> [libc::iovec; N];
}

// SAFETY: `BufSlice` has the same safety requirements as `Buf` and since `B`
// implements `Buf` it's safe to implement `BufSlice` for an array of `B`.
unsafe impl<B: Buf, const N: usize> BufSlice<N> for [B; N] {
    unsafe fn as_iovec(&self) -> [libc::iovec; N] {
        let mut iovecs = MaybeUninit::uninit_array();
        for (buf, iovec) in self.iter().zip(iovecs.iter_mut()) {
            let (ptr, len) = buf.parts();
            iovec.write(libc::iovec {
                iov_base: ptr as _,
                iov_len: len as _,
            });
        }
        MaybeUninit::array_assume_init(iovecs)
    }
}

macro_rules! buf_slice_for_tuple {
    (
        // Number of values.
        $N: expr,
        // Generic parameter name and tuple index.
        $( $generic: ident . $index: tt ),+
    ) => {
        // SAFETY: `BufSlice` has the same safety requirements as `Buf` and
        // since all generic buffer must implement `Buf` it's safe to implement
        // `BufSlice` a tuple of all those buffers.
        unsafe impl<$( $generic: Buf ),+> BufSlice<$N> for ($( $generic ),+) {
            unsafe fn as_iovec(&self) -> [libc::iovec; $N] {
                [
                    $({
                        let (ptr, len) = self.$index.parts();
                        libc::iovec {
                            iov_base: ptr as _,
                            iov_len: len as _,
                        }
                    }),+
                ]
            }
        }
    };
}

buf_slice_for_tuple!(2, A.0, B.1);
buf_slice_for_tuple!(3, A.0, B.1, C.2);
buf_slice_for_tuple!(4, A.0, B.1, C.2, D.3);
buf_slice_for_tuple!(5, A.0, B.1, C.2, D.3, E.4);
buf_slice_for_tuple!(6, A.0, B.1, C.2, D.3, E.4, F.5);
buf_slice_for_tuple!(7, A.0, B.1, C.2, D.3, E.4, F.5, G.6);
buf_slice_for_tuple!(8, A.0, B.1, C.2, D.3, E.4, F.5, G.6, I.7);

//! Types shared across Unix-like implementations.

use crate::io::{Buf, BufMut};

#[repr(transparent)] // Needed for I/O.
pub(crate) struct IoMutSlice(libc::iovec);

impl IoMutSlice {
    pub(crate) fn new<B: BufMut>(buf: &mut B) -> IoMutSlice {
        let (ptr, len) = unsafe { buf.parts_mut() };
        IoMutSlice(libc::iovec {
            iov_base: ptr.cast(),
            iov_len: len as _,
        })
    }

    // NOTE: can't implement `as_bytes` as we don't know if the bytes are
    // initialised. `len` will have to do.
    pub(crate) const fn len(&self) -> usize {
        self.0.iov_len
    }
}

// SAFETY: `libc::iovec` is `!Sync`, but it's just a point to some bytes, so
// it's actually `Send` and `Sync`.
unsafe impl Send for IoMutSlice {}
unsafe impl Sync for IoMutSlice {}

#[repr(transparent)] // Needed for I/O.
pub(crate) struct IoSlice(libc::iovec);

impl IoSlice {
    pub(crate) fn new<B: Buf>(buf: &B) -> IoSlice {
        let (ptr, len) = unsafe { buf.parts() };
        IoSlice(libc::iovec {
            iov_base: ptr.cast_mut().cast(),
            iov_len: len as _,
        })
    }

    pub(crate) const fn as_bytes(&self) -> &[u8] {
        // SAFETY: on creation we've ensure that `iov_base` and `iov_len` are
        // valid.
        unsafe { std::slice::from_raw_parts(self.0.iov_base.cast(), self.0.iov_len) }
    }

    pub(crate) const fn len(&self) -> usize {
        self.0.iov_len
    }

    pub(crate) fn set_len(&mut self, new_len: usize) {
        debug_assert!(self.0.iov_len <= new_len);
        self.0.iov_len = new_len;
    }
}

// SAFETY: `libc::iovec` is `!Sync`, but it's just a point to some bytes, so
// it's actually `Send` and `Sync`.
unsafe impl Send for IoSlice {}
unsafe impl Sync for IoSlice {}

//! Types shared across Unix-like implementations.

use std::fmt;
use std::mem::{self, MaybeUninit};

use crate::io::{Buf, BufMut};
use crate::net::SocketAddress;

#[repr(transparent)] // Needed for I/O.
pub(crate) struct IoMutSlice(libc::iovec);

impl IoMutSlice {
    pub(crate) fn new<B: BufMut>(buf: &mut B) -> IoMutSlice {
        let (ptr, len) = unsafe { buf.parts_mut() };
        IoMutSlice(libc::iovec {
            iov_base: ptr.cast(),
            iov_len: len as libc::size_t,
        })
    }

    pub(crate) unsafe fn parts_mut(&mut self) -> (*mut u8, usize) {
        (self.0.iov_base.cast(), self.0.iov_len)
    }

    pub(crate) const fn ptr(&self) -> *const u8 {
        self.0.iov_base.cast()
    }

    // NOTE: can't implement `as_bytes` as we don't know if the bytes are
    // initialised. `len` will have to do.
    pub(crate) const fn len(&self) -> usize {
        self.0.iov_len
    }

    pub(crate) fn set_len(&mut self, new_len: usize) {
        self.0.iov_len = new_len;
    }
}

// SAFETY: `libc::iovec` is `!Sync`, but it's just a pointer to some bytes, so
// it's actually `Send` and `Sync`.
unsafe impl Send for IoMutSlice {}
unsafe impl Sync for IoMutSlice {}

impl fmt::Debug for IoMutSlice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IoMutSlice")
            .field("ptr", &self.ptr())
            .field("len", &self.len())
            .finish()
    }
}

#[repr(transparent)] // Needed for I/O.
pub(crate) struct IoSlice(libc::iovec);

impl IoSlice {
    pub(crate) fn new<B: Buf>(buf: &B) -> IoSlice {
        let (ptr, len) = unsafe { buf.parts() };
        IoSlice(libc::iovec {
            iov_base: ptr.cast_mut().cast(),
            iov_len: len as libc::size_t,
        })
    }

    pub(crate) const fn as_bytes(&self) -> &[u8] {
        // SAFETY: on creation we've ensure that `iov_base` and `iov_len` are
        // valid.
        unsafe { std::slice::from_raw_parts(self.0.iov_base.cast(), self.0.iov_len) }
    }

    pub(crate) const fn ptr(&self) -> *const u8 {
        self.0.iov_base.cast()
    }

    pub(crate) const fn len(&self) -> usize {
        self.0.iov_len
    }

    pub(crate) fn set_len(&mut self, new_len: usize) {
        debug_assert!(self.0.iov_len >= new_len);
        self.0.iov_len = new_len;
    }

    pub(crate) fn skip(&mut self, n: usize) {
        debug_assert!(self.0.iov_len >= n);
        self.0.iov_base = unsafe { self.0.iov_base.offset(n.cast_signed()) };
        self.0.iov_len -= n;
    }
}

// SAFETY: `libc::iovec` is `!Sync`, but it's just a pointer to some bytes, so
// it's actually `Send` and `Sync`.
unsafe impl Send for IoSlice {}
unsafe impl Sync for IoSlice {}

impl fmt::Debug for IoSlice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("IoSlice").field(&self.as_bytes()).finish()
    }
}

#[repr(transparent)] // Needed for system calls.
pub(crate) struct MsgHeader(libc::msghdr);

impl MsgHeader {
    pub(crate) const fn empty() -> MsgHeader {
        // SAFETY: zeroed `msghdr` is valid.
        unsafe { mem::zeroed() }
    }

    /// # Safety
    ///
    /// Caller must ensure that `address` and `iovecs` outlives `MsgHeader`.
    #[allow(trivial_numeric_casts, clippy::cast_possible_wrap)]
    pub(crate) unsafe fn init_recv<A: SocketAddress>(
        &mut self,
        address: &mut MaybeUninit<A::Storage>,
        iovecs: &mut [crate::io::IoMutSlice],
    ) -> *mut libc::msghdr {
        let (address_ptr, address_length) = unsafe { A::as_mut_ptr(address) };
        self.0.msg_name = address_ptr.cast();
        self.0.msg_namelen = address_length;
        // SAFETY: this cast is safe because `IoMutSlice` is `repr(transparent)`.
        self.0.msg_iov = iovecs.as_mut_ptr().cast();
        self.0.msg_iovlen = iovecs.len() as _;
        &raw mut self.0
    }

    /// # Safety
    ///
    /// Caller must ensure that `address` and `iovecs` outlives `MsgHeader`.
    #[allow(trivial_numeric_casts, clippy::cast_possible_wrap)]
    pub(crate) unsafe fn init_send<A: SocketAddress>(
        &mut self,
        address: &mut A::Storage,
        iovecs: &mut [crate::io::IoSlice],
    ) -> *const libc::msghdr {
        let (address_ptr, address_length) = unsafe { A::as_ptr(address) };
        self.0.msg_name = address_ptr.cast_mut().cast();
        self.0.msg_namelen = address_length;
        // SAFETY: this cast is safe because `IoSlice` is `repr(transparent)`.
        self.0.msg_iov = iovecs.as_mut_ptr().cast();
        self.0.msg_iovlen = iovecs.len() as _;
        &raw const self.0
    }

    pub(crate) const fn address_len(&self) -> libc::socklen_t {
        self.0.msg_namelen
    }

    pub(crate) const fn flags(&self) -> libc::c_int {
        self.0.msg_flags
    }
}

// SAFETY: `libc::msghr` is `!Sync`, but the two pointers two the address and
// `iovecs` (`IoMutSlice`/`IoSlice`) are `Send` and `Sync`.
unsafe impl Send for MsgHeader {}
unsafe impl Sync for MsgHeader {}

impl fmt::Debug for MsgHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MsgHeader")
            .field("msg_name", &self.0.msg_name)
            .field("msg_namelen", &self.0.msg_namelen)
            .field("msg_iov", &self.0.msg_iov)
            .field("msg_iovlen", &self.0.msg_iovlen)
            .field("msg_control", &self.0.msg_control)
            .field("msg_controllen", &self.0.msg_controllen)
            .field("msg_flags", &self.0.msg_flags)
            .finish()
    }
}

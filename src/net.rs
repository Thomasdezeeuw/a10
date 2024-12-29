//! Networking.
//!
//! To create a new socket ([`AsyncFd`]) use the [`socket`] function, which
//! issues a non-blocking `socket(2)` call.

use std::mem::MaybeUninit;
use std::{io, ptr};

use crate::fd::{AsyncFd, Descriptor};
use crate::op::{operation, Operation};
use crate::{man_link, sys, SubmissionQueue};

/// Creates a new socket.
#[doc = man_link!(socket(2))]
pub const fn socket<D: Descriptor>(
    sq: SubmissionQueue,
    domain: libc::c_int,
    r#type: libc::c_int,
    protocol: libc::c_int,
    flags: libc::c_int,
) -> Socket<D> {
    Socket(Operation::new(sq, (), (domain, r#type, protocol, flags)))
}

operation!(
    /// [`Future`] behind [`socket`].
    ///
    /// If you're looking for a socket type, there is none, see [`AsyncFd`].
    pub struct Socket<D: Descriptor>(sys::net::SocketOp<D>) -> io::Result<AsyncFd<D>>;
);

/// Trait that defines the behaviour of socket addresses.
///
/// Linux (Unix) uses different address types for different sockets, to support
/// all of them A10 uses a trait to define the behaviour.
///
/// Current implementations include
///  * IPv4 addresses: [`libc::sockaddr_in`],
///  * IPv6 addresses: [`libc::sockaddr_in6`],
///  * Unix addresses: [`libc::sockaddr_un`],
///  * Storage of any address [`libc::sockaddr_storage`] kind.
///
/// For the last two types we need to keep track of the length of the address,
/// for which it uses a tuple `(addr, `[`libc::socklen_t`]`)`.
pub trait SocketAddress: Sized {
    /// Returns itself as raw pointer and length.
    ///
    /// # Safety
    ///
    /// The pointer must be valid to read up to length bytes from.
    ///
    /// The implementation must ensure that the pointer is valid, i.e. not null
    /// and pointing to memory owned by the address. Furthermore it must ensure
    /// that the returned length is, in combination with the pointer, valid. In
    /// other words the memory the pointer and length are pointing to must be a
    /// valid memory address and owned by the address.
    ///
    /// Note that the above requirements are only required for implementations
    /// outside of A10. **This trait is unfit for external use!**
    unsafe fn as_ptr(&self) -> (*const libc::sockaddr, libc::socklen_t);

    /// Returns `address` as an adress storage and it's length.
    ///
    /// # Safety
    ///
    /// Only initialised bytes may be written to the pointer returned.
    ///
    /// The implementation must ensure that the pointer is valid, i.e. not null
    /// and pointing to memory owned by the address. Furthermore it must ensure
    /// that the returned length is, in combination with the pointer, valid. In
    /// other words the memory the pointer and length are pointing to must be a
    /// valid memory address and owned by the address.
    ///
    /// Note that the above requirements are only required for implementations
    /// outside of A10. **This trait is unfit for external use!**
    unsafe fn as_mut_ptr(address: &mut MaybeUninit<Self>)
        -> (*mut libc::sockaddr, libc::socklen_t);

    /// Initialise `address` to which at least `length` bytes have been written
    /// (by the kernel).
    ///
    /// # Safety
    ///
    /// Caller must ensure that at least `length` bytes have been written to
    /// `address`.
    unsafe fn init(address: MaybeUninit<Self>, length: libc::socklen_t) -> Self;
}

/// Socket address.
impl SocketAddress for (libc::sockaddr, libc::socklen_t) {
    unsafe fn as_ptr(&self) -> (*const libc::sockaddr, libc::socklen_t) {
        (ptr::addr_of!(self.0).cast(), self.1)
    }

    unsafe fn as_mut_ptr(this: &mut MaybeUninit<Self>) -> (*mut libc::sockaddr, libc::socklen_t) {
        (
            ptr::addr_of_mut!((*this.as_mut_ptr()).0).cast(),
            size_of::<libc::sockaddr>() as _,
        )
    }

    unsafe fn init(this: MaybeUninit<Self>, length: libc::socklen_t) -> Self {
        debug_assert!(length >= size_of::<libc::sa_family_t>() as _);
        // SAFETY: caller must initialise the address.
        let mut this = this.assume_init();
        this.1 = length;
        this
    }
}

/// Any kind of address.
impl SocketAddress for (libc::sockaddr_storage, libc::socklen_t) {
    unsafe fn as_ptr(&self) -> (*const libc::sockaddr, libc::socklen_t) {
        (ptr::addr_of!(self.0).cast(), self.1)
    }

    unsafe fn as_mut_ptr(this: &mut MaybeUninit<Self>) -> (*mut libc::sockaddr, libc::socklen_t) {
        (
            ptr::addr_of_mut!((*this.as_mut_ptr()).0).cast(),
            size_of::<libc::sockaddr_storage>() as _,
        )
    }

    unsafe fn init(this: MaybeUninit<Self>, length: libc::socklen_t) -> Self {
        debug_assert!(length >= size_of::<libc::sa_family_t>() as _);
        // SAFETY: caller must initialise the address.
        let mut this = this.assume_init();
        this.1 = length;
        this
    }
}

/// IPv4 address.
impl SocketAddress for libc::sockaddr_in {
    unsafe fn as_ptr(&self) -> (*const libc::sockaddr, libc::socklen_t) {
        (
            (self as *const libc::sockaddr_in).cast(),
            size_of::<Self>() as _,
        )
    }

    unsafe fn as_mut_ptr(this: &mut MaybeUninit<Self>) -> (*mut libc::sockaddr, libc::socklen_t) {
        (this.as_mut_ptr().cast(), size_of::<Self>() as _)
    }

    unsafe fn init(this: MaybeUninit<Self>, length: libc::socklen_t) -> Self {
        debug_assert!(length == size_of::<Self>() as _);
        // SAFETY: caller must initialise the address.
        this.assume_init()
    }
}

/// IPv6 address.
impl SocketAddress for libc::sockaddr_in6 {
    unsafe fn as_ptr(&self) -> (*const libc::sockaddr, libc::socklen_t) {
        (
            (self as *const libc::sockaddr_in6).cast(),
            size_of::<Self>() as _,
        )
    }

    unsafe fn as_mut_ptr(this: &mut MaybeUninit<Self>) -> (*mut libc::sockaddr, libc::socklen_t) {
        (this.as_mut_ptr().cast(), size_of::<Self>() as _)
    }

    unsafe fn init(this: MaybeUninit<Self>, length: libc::socklen_t) -> Self {
        debug_assert!(length == size_of::<Self>() as _);
        // SAFETY: caller must initialise the address.
        this.assume_init()
    }
}

/// Unix address.
impl SocketAddress for (libc::sockaddr_un, libc::socklen_t) {
    unsafe fn as_ptr(&self) -> (*const libc::sockaddr, libc::socklen_t) {
        (ptr::addr_of!(self.0).cast(), self.1)
    }

    unsafe fn as_mut_ptr(this: &mut MaybeUninit<Self>) -> (*mut libc::sockaddr, libc::socklen_t) {
        (
            ptr::addr_of_mut!((*this.as_mut_ptr()).0).cast(),
            size_of::<libc::sockaddr_un>() as _,
        )
    }

    unsafe fn init(this: MaybeUninit<Self>, length: libc::socklen_t) -> Self {
        debug_assert!(length >= size_of::<libc::sa_family_t>() as _);
        // SAFETY: caller must initialise the address.
        let mut this = this.assume_init();
        this.1 = length;
        this
    }
}

/// When [`accept`]ing connections we're not interested in the address.
///
/// This is not acceptable in calls to [`connect`].
///
/// [`accept`]: AsyncFd::accept
/// [`connect`]: AsyncFd::connect
#[derive(Debug)]
pub struct NoAddress;

impl SocketAddress for NoAddress {
    unsafe fn as_ptr(&self) -> (*const libc::sockaddr, libc::socklen_t) {
        // NOTE: this goes against the requirements of `cast_ptr`.
        (ptr::null_mut(), 0)
    }

    unsafe fn as_mut_ptr(this: &mut MaybeUninit<Self>) -> (*mut libc::sockaddr, libc::socklen_t) {
        _ = this;
        // NOTE: this goes against the requirements of `as_mut_ptr`.
        (ptr::null_mut(), 0)
    }

    unsafe fn init(this: MaybeUninit<Self>, length: libc::socklen_t) -> Self {
        debug_assert!(length == 0);
        this.assume_init()
    }
}

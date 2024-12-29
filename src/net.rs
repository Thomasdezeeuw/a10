//! Networking.
//!
//! To create a new socket ([`AsyncFd`]) use the [`socket`] function, which
//! issues a non-blocking `socket(2)` call.

use std::ffi::OsStr;
use std::mem::{self, MaybeUninit};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::os::linux::net::SocketAddrExt;
use std::os::unix;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::{io, ptr, slice};

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
/// Unix uses different address types for different sockets, to support
/// all of them A10 uses this trait.
pub trait SocketAddress: private::SocketAddress + Sized {}

mod private {
    use std::mem::MaybeUninit;

    pub trait SocketAddress {
        type Storage: Sized;

        /// Returns itself as storage.
        fn as_storage(self) -> Self::Storage;

        /// Returns a raw pointer and length to the storage.
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
        unsafe fn as_ptr(storage: &Self::Storage) -> (*const libc::sockaddr, libc::socklen_t);

        /// Returns a mutable raw pointer and length to `storage`.
        ///
        /// # Safety
        ///
        /// Only initialised bytes may be written to the pointer returned.
        unsafe fn as_mut_ptr(
            storage: &mut MaybeUninit<Self::Storage>,
        ) -> (*mut libc::sockaddr, libc::socklen_t);

        /// Initialise the address from `storage`, to which at least `length`
        /// bytes have been written (by the kernel).
        ///
        /// # Safety
        ///
        /// Caller must ensure that at least `length` bytes have been written to
        /// `address`.
        unsafe fn init(storage: MaybeUninit<Self::Storage>, length: libc::socklen_t) -> Self;
    }
}

impl SocketAddress for SocketAddr {}

impl private::SocketAddress for SocketAddr {
    type Storage = libc::sockaddr_in6; // Fits both v4 and v6.

    fn as_storage(self) -> Self::Storage {
        match self {
            SocketAddr::V4(addr) => {
                // SAFETY: all zeroes is valid for `sockaddr_in6`.
                let mut storage = unsafe { mem::zeroed::<libc::sockaddr_in6>() };
                // SAFETY: `sockaddr_in` fits in `sockaddr_in6`.
                unsafe {
                    (&mut storage as *mut libc::sockaddr_in6)
                        .cast::<libc::sockaddr_in>()
                        .write(addr.as_storage());
                }
                storage
            }
            SocketAddr::V6(addr) => addr.as_storage(),
        }
    }

    unsafe fn as_ptr(storage: &Self::Storage) -> (*const libc::sockaddr, libc::socklen_t) {
        let ptr = (storage as *const Self::Storage).cast();
        let size = if storage.sin6_family as i32 == libc::AF_INET {
            size_of::<libc::sockaddr_in>()
        } else {
            size_of::<libc::sockaddr_in6>()
        };
        (ptr, size as libc::socklen_t)
    }

    unsafe fn as_mut_ptr(
        storage: &mut MaybeUninit<Self::Storage>,
    ) -> (*mut libc::sockaddr, libc::socklen_t) {
        (storage.as_mut_ptr().cast(), size_of::<Self::Storage>() as _)
    }

    unsafe fn init(storage: MaybeUninit<Self::Storage>, length: libc::socklen_t) -> Self {
        debug_assert!(length as usize >= size_of::<libc::sa_family_t>());
        let family = unsafe { ptr::addr_of!((*storage.as_ptr()).sin6_family).read() };
        if family == libc::AF_INET as libc::sa_family_t {
            let storage = storage.as_ptr().cast::<libc::sockaddr_in>().read();
            SocketAddrV4::init(MaybeUninit::new(storage), length).into()
        } else {
            SocketAddrV6::init(storage, length).into()
        }
    }
}

impl SocketAddress for SocketAddrV4 {}

impl private::SocketAddress for SocketAddrV4 {
    type Storage = libc::sockaddr_in;

    fn as_storage(self) -> Self::Storage {
        libc::sockaddr_in {
            sin_family: libc::AF_INET as libc::sa_family_t,
            sin_port: self.port().to_be(),
            sin_addr: libc::in_addr {
                s_addr: u32::from_ne_bytes(self.ip().octets()),
            },
            sin_zero: [0; 8],
        }
    }

    unsafe fn as_ptr(storage: &Self::Storage) -> (*const libc::sockaddr, libc::socklen_t) {
        let ptr = (storage as *const Self::Storage).cast();
        (ptr, size_of::<Self::Storage>() as _)
    }

    unsafe fn as_mut_ptr(
        storage: &mut MaybeUninit<Self::Storage>,
    ) -> (*mut libc::sockaddr, libc::socklen_t) {
        (storage.as_mut_ptr().cast(), size_of::<Self::Storage>() as _)
    }

    unsafe fn init(storage: MaybeUninit<Self::Storage>, length: libc::socklen_t) -> Self {
        debug_assert!(length == size_of::<Self::Storage>() as _);
        // SAFETY: caller must initialise the address.
        let storage = unsafe { storage.assume_init() };
        debug_assert!(storage.sin_family as i32 == libc::AF_INET);
        let ip = Ipv4Addr::from(storage.sin_addr.s_addr.to_ne_bytes());
        let port = u16::from_be(storage.sin_port);
        SocketAddrV4::new(ip, port)
    }
}

impl SocketAddress for SocketAddrV6 {}

impl private::SocketAddress for SocketAddrV6 {
    type Storage = libc::sockaddr_in6;

    fn as_storage(self) -> Self::Storage {
        libc::sockaddr_in6 {
            sin6_family: libc::AF_INET6 as libc::sa_family_t,
            sin6_port: self.port().to_be(),
            sin6_flowinfo: self.flowinfo(),
            sin6_addr: libc::in6_addr {
                s6_addr: self.ip().octets(),
            },
            sin6_scope_id: self.scope_id(),
        }
    }

    unsafe fn as_ptr(storage: &Self::Storage) -> (*const libc::sockaddr, libc::socklen_t) {
        let ptr = (storage as *const Self::Storage).cast();
        (ptr, size_of::<Self::Storage>() as _)
    }

    unsafe fn as_mut_ptr(
        storage: &mut MaybeUninit<Self::Storage>,
    ) -> (*mut libc::sockaddr, libc::socklen_t) {
        (storage.as_mut_ptr().cast(), size_of::<Self::Storage>() as _)
    }

    unsafe fn init(storage: MaybeUninit<Self::Storage>, length: libc::socklen_t) -> Self {
        debug_assert!(length == size_of::<Self::Storage>() as _);
        // SAFETY: caller must initialise the address.
        let storage = unsafe { storage.assume_init() };
        debug_assert!(storage.sin6_family as i32 == libc::AF_INET6);
        let ip = Ipv6Addr::from(storage.sin6_addr.s6_addr);
        let port = u16::from_be(storage.sin6_port);
        SocketAddrV6::new(ip, port, storage.sin6_flowinfo, storage.sin6_scope_id)
    }
}

impl SocketAddress for unix::net::SocketAddr {}

impl private::SocketAddress for unix::net::SocketAddr {
    type Storage = libc::sockaddr_un;

    fn as_storage(self) -> Self::Storage {
        let mut storage = libc::sockaddr_un {
            sun_family: libc::AF_UNIX as libc::sa_family_t,
            // SAFETY: all zero is valid for `sockaddr_un`.
            ..unsafe { mem::zeroed() }
        };
        // SAFETY: casting `[i8]` to `[u8]` is safe.
        let path = unsafe {
            slice::from_raw_parts_mut::<u8>(
                storage.sun_path.as_mut_ptr().cast(),
                storage.sun_path.len(),
            )
        };
        if let Some(pathname) = self.as_pathname() {
            let bytes = pathname.as_os_str().as_bytes();
            path[..bytes.len()].copy_from_slice(bytes);
        } else if let Some(bytes) = self.as_abstract_name() {
            path[1..][..bytes.len()].copy_from_slice(bytes);
        } else {
            // Unnamed address, we'll leave it all zero.
        }
        storage
    }

    unsafe fn as_ptr(storage: &Self::Storage) -> (*const libc::sockaddr, libc::socklen_t) {
        let ptr = (storage as *const Self::Storage).cast();
        (ptr, size_of::<Self::Storage>() as _)
    }

    unsafe fn as_mut_ptr(
        storage: &mut MaybeUninit<Self::Storage>,
    ) -> (*mut libc::sockaddr, libc::socklen_t) {
        (storage.as_mut_ptr().cast(), size_of::<Self::Storage>() as _)
    }

    unsafe fn init(storage: MaybeUninit<Self::Storage>, length: libc::socklen_t) -> Self {
        debug_assert!(length as usize >= size_of::<libc::sa_family_t>());
        let family = unsafe { ptr::addr_of!((*storage.as_ptr()).sun_family).read() };
        debug_assert!(family == libc::AF_UNIX as libc::sa_family_t);
        let path_ptr = ptr::addr_of!((*storage.as_ptr()).sun_path);
        let length = length as usize - (storage.as_ptr().addr() - path_ptr.addr());
        // SAFETY: the kernel ensures that at least `length` bytes are
        // initialised.
        let path = unsafe { slice::from_raw_parts::<u8>(path_ptr.cast(), length) };
        if let Some(0) = path.get(0) {
            // NOTE: `from_abstract_name` adds a starting null byte.
            unix::net::SocketAddr::from_abstract_name(&path[1..])
        } else {
            unix::net::SocketAddr::from_pathname(Path::new(OsStr::from_bytes(path)))
        }
        // Fallback to an unnamed address.
        .unwrap_or_else(|_| unix::net::SocketAddr::from_pathname("").unwrap())
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

impl SocketAddress for NoAddress {}

impl private::SocketAddress for NoAddress {
    type Storage = Self;

    fn as_storage(self) -> Self::Storage {
        NoAddress
    }

    unsafe fn as_ptr(_: &Self::Storage) -> (*const libc::sockaddr, libc::socklen_t) {
        // NOTE: this goes against the requirements of `cast_ptr`.
        (ptr::null_mut(), 0)
    }

    unsafe fn as_mut_ptr(_: &mut MaybeUninit<Self>) -> (*mut libc::sockaddr, libc::socklen_t) {
        // NOTE: this goes against the requirements of `as_mut_ptr`.
        (ptr::null_mut(), 0)
    }

    unsafe fn init(_: MaybeUninit<Self>, length: libc::socklen_t) -> Self {
        debug_assert!(length == 0);
        NoAddress
    }
}

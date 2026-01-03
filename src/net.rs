//! Networking.
//!
//! To create a new socket ([`AsyncFd`]) use the [`socket`] function, which
//! issues a non-blocking `socket(2)` call.

use std::ffi::OsStr;
use std::future::Future;
use std::mem::{self, MaybeUninit};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
#[cfg(any(target_os = "android", target_os = "linux"))]
use std::os::linux::net::SocketAddrExt;
use std::os::unix;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::pin::Pin;
use std::task::{self, Poll};
use std::{fmt, io, ptr, slice};

use crate::new_flag;

new_flag!(
    /// Specification of the communication domain for a socket.
    pub struct Domain(i32) {
        /// Domain for IPv4 communication.
        IPV4 = libc::AF_INET,
        /// Domain for IPv6 communication.
        IPV6 = libc::AF_INET6,
        /// Domain for Unix socket communication.
        UNIX = libc::AF_UNIX,
        /// Domain for low-level packet interface.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        PACKET = libc::AF_PACKET,
        /// Domain for low-level VSOCK interface.
        VSOCK = libc::AF_VSOCK,
    }

    /// Specification of communication semantics on a socket.
    pub struct Type(u32) {
        /// Provides sequenced, reliable, two-way, connection-based byte
        /// streams.
        ///
        /// Used for protocols such as TCP.
        STREAM = libc::SOCK_STREAM,
        /// Supports datagrams (connectionless, unreliable messages of a fixed
        /// maximum length).
        ///
        /// Used for protocols such as UDP.
        DGRAM = libc::SOCK_DGRAM,
        /// Raw network protocol access.
        RAW = libc::SOCK_RAW,
        /// Provides a reliable datagram layer that does not guarantee ordering.
        RDM = libc::SOCK_RDM,
        /// Provides a sequenced, reliable, two-way connection-based data
        /// transmission path for datagrams of fixed maximum length.
        SEQPACKET = libc::SOCK_SEQPACKET,
        /// Datagram Congestion Control Protocol socket.
        ///
        /// Used for the DCCP protocol.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        DCCP = libc::SOCK_DCCP,
    }

    /// Specification of communication protocol.
    pub struct Protocol(u32) {
        /// Internet Control Message Protocol IPv4.
        ICMPV4 = libc::IPPROTO_ICMP,
        /// Internet Control Message Protocol IPv6.
        ICMPV6 = libc::IPPROTO_ICMPV6,
        /// Transmission Control Protocol.
        TCP = libc::IPPROTO_TCP,
        /// User Datagram Protocol.
        UDP = libc::IPPROTO_UDP,
        /// Datagram Congestion Control Protocol.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        DCCP = libc::IPPROTO_DCCP,
        /// Stream Control Transport Protocol.
        SCTP = libc::IPPROTO_SCTP,
        /// UDP-Lite.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        UDPLITE = libc::IPPROTO_UDPLITE,
        /// Raw IP packets.
        RAW = libc::IPPROTO_RAW,
        /// Multipath TCP connection.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        MPTCP = libc::IPPROTO_MPTCP,
    }
);

impl Domain {
    /// Returns the correct domain for `address`.
    pub fn for_address<A: SocketAddress>(address: &A) -> Domain {
        address.domain()
    }
}

/// Trait that defines the behaviour of socket addresses.
///
/// Unix uses different address types for different sockets, to support
/// all of them A10 uses this trait.
pub trait SocketAddress: private::SocketAddress + Sized {}

mod private {
    use std::mem::MaybeUninit;

    use super::Domain;

    pub trait SocketAddress {
        type Storage: Sized;

        /// Returns itself as storage.
        fn into_storage(self) -> Self::Storage;

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

        /// Return the correct domain for the address.
        fn domain(&self) -> Domain;
    }
}

impl SocketAddress for SocketAddr {}

impl private::SocketAddress for SocketAddr {
    type Storage = libc::sockaddr_in6; // Fits both v4 and v6.

    fn into_storage(self) -> Self::Storage {
        match self {
            SocketAddr::V4(addr) => {
                // SAFETY: all zeroes is valid for `sockaddr_in6`.
                let mut storage = unsafe { mem::zeroed::<libc::sockaddr_in6>() };
                // SAFETY: `sockaddr_in` fits in `sockaddr_in6`.
                unsafe {
                    ptr::from_mut(&mut storage)
                        .cast::<libc::sockaddr_in>()
                        .write(addr.into_storage());
                }
                storage
            }
            SocketAddr::V6(addr) => addr.into_storage(),
        }
    }

    unsafe fn as_ptr(storage: &Self::Storage) -> (*const libc::sockaddr, libc::socklen_t) {
        let ptr = ptr::from_ref(storage).cast();
        let size = if <libc::c_int>::from(storage.sin6_family) == libc::AF_INET {
            size_of::<libc::sockaddr_in>()
        } else {
            size_of::<libc::sockaddr_in6>()
        };
        (ptr, size as libc::socklen_t)
    }

    unsafe fn as_mut_ptr(
        storage: &mut MaybeUninit<Self::Storage>,
    ) -> (*mut libc::sockaddr, libc::socklen_t) {
        (
            storage.as_mut_ptr().cast(),
            size_of::<Self::Storage>() as libc::socklen_t,
        )
    }

    unsafe fn init(storage: MaybeUninit<Self::Storage>, length: libc::socklen_t) -> Self {
        debug_assert!(length as usize >= size_of::<libc::sa_family_t>());
        let family = unsafe { storage.as_ptr().cast::<libc::sa_family_t>().read() };
        if family == libc::AF_INET as libc::sa_family_t {
            let storage = unsafe { storage.as_ptr().cast::<libc::sockaddr_in>().read() };
            unsafe { SocketAddrV4::init(MaybeUninit::new(storage), length).into() }
        } else {
            unsafe { SocketAddrV6::init(storage, length).into() }
        }
    }

    fn domain(&self) -> Domain {
        match self {
            SocketAddr::V4(_) => Domain::IPV4,
            SocketAddr::V6(_) => Domain::IPV6,
        }
    }
}

impl SocketAddress for SocketAddrV4 {}

impl private::SocketAddress for SocketAddrV4 {
    type Storage = libc::sockaddr_in;

    fn into_storage(self) -> Self::Storage {
        libc::sockaddr_in {
            sin_family: libc::AF_INET as libc::sa_family_t,
            sin_port: self.port().to_be(),
            sin_addr: libc::in_addr {
                s_addr: u32::from_ne_bytes(self.ip().octets()),
            },
            sin_zero: [0; 8],
            // A number of OS have `sin_len`, but we don't use it.
            #[cfg(not(any(target_os = "android", target_os = "linux")))]
            sin_len: 0,
        }
    }

    unsafe fn as_ptr(storage: &Self::Storage) -> (*const libc::sockaddr, libc::socklen_t) {
        let ptr = ptr::from_ref(storage).cast();
        (ptr, size_of::<Self::Storage>() as libc::socklen_t)
    }

    unsafe fn as_mut_ptr(
        storage: &mut MaybeUninit<Self::Storage>,
    ) -> (*mut libc::sockaddr, libc::socklen_t) {
        (
            storage.as_mut_ptr().cast(),
            size_of::<Self::Storage>() as libc::socklen_t,
        )
    }

    unsafe fn init(storage: MaybeUninit<Self::Storage>, length: libc::socklen_t) -> Self {
        debug_assert!(length == size_of::<Self::Storage>() as libc::socklen_t);
        // SAFETY: caller must initialise the address.
        let storage = unsafe { storage.assume_init() };
        debug_assert!(<libc::c_int>::from(storage.sin_family) == libc::AF_INET);
        let ip = Ipv4Addr::from(storage.sin_addr.s_addr.to_ne_bytes());
        let port = u16::from_be(storage.sin_port);
        SocketAddrV4::new(ip, port)
    }

    fn domain(&self) -> Domain {
        Domain::IPV4
    }
}

impl SocketAddress for SocketAddrV6 {}

impl private::SocketAddress for SocketAddrV6 {
    type Storage = libc::sockaddr_in6;

    fn into_storage(self) -> Self::Storage {
        libc::sockaddr_in6 {
            sin6_family: libc::AF_INET6 as libc::sa_family_t,
            sin6_port: self.port().to_be(),
            sin6_flowinfo: self.flowinfo(),
            sin6_addr: libc::in6_addr {
                s6_addr: self.ip().octets(),
            },
            sin6_scope_id: self.scope_id(),
            // A number of OS have `sin6_len`, but we don't use it.
            #[cfg(not(any(target_os = "android", target_os = "linux")))]
            sin6_len: 0,
        }
    }

    unsafe fn as_ptr(storage: &Self::Storage) -> (*const libc::sockaddr, libc::socklen_t) {
        let ptr = ptr::from_ref(storage).cast();
        (ptr, size_of::<Self::Storage>() as libc::socklen_t)
    }

    unsafe fn as_mut_ptr(
        storage: &mut MaybeUninit<Self::Storage>,
    ) -> (*mut libc::sockaddr, libc::socklen_t) {
        (
            storage.as_mut_ptr().cast(),
            size_of::<Self::Storage>() as libc::socklen_t,
        )
    }

    unsafe fn init(storage: MaybeUninit<Self::Storage>, length: libc::socklen_t) -> Self {
        debug_assert!(length == size_of::<Self::Storage>() as libc::socklen_t);
        // SAFETY: caller must initialise the address.
        let storage = unsafe { storage.assume_init() };
        debug_assert!(<libc::c_int>::from(storage.sin6_family) == libc::AF_INET6);
        let ip = Ipv6Addr::from(storage.sin6_addr.s6_addr);
        let port = u16::from_be(storage.sin6_port);
        SocketAddrV6::new(ip, port, storage.sin6_flowinfo, storage.sin6_scope_id)
    }

    fn domain(&self) -> Domain {
        Domain::IPV6
    }
}

impl SocketAddress for unix::net::SocketAddr {}

impl private::SocketAddress for unix::net::SocketAddr {
    type Storage = libc::sockaddr_un;

    fn into_storage(self) -> Self::Storage {
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
        } else {
            #[cfg(any(target_os = "android", target_os = "linux"))]
            if let Some(bytes) = self.as_abstract_name() {
                path[1..][..bytes.len()].copy_from_slice(bytes);
            }

            // Unnamed address, we'll leave it all zero.
        }
        storage
    }

    unsafe fn as_ptr(storage: &Self::Storage) -> (*const libc::sockaddr, libc::socklen_t) {
        let ptr = ptr::from_ref(storage).cast();
        (ptr, size_of::<Self::Storage>() as libc::socklen_t)
    }

    unsafe fn as_mut_ptr(
        storage: &mut MaybeUninit<Self::Storage>,
    ) -> (*mut libc::sockaddr, libc::socklen_t) {
        (
            storage.as_mut_ptr().cast(),
            size_of::<Self::Storage>() as libc::socklen_t,
        )
    }

    unsafe fn init(storage: MaybeUninit<Self::Storage>, length: libc::socklen_t) -> Self {
        debug_assert!(length as usize >= size_of::<libc::sa_family_t>());
        let family = unsafe { ptr::addr_of!((*storage.as_ptr()).sun_family).read() };
        debug_assert!(family == libc::AF_UNIX as libc::sa_family_t);
        let path_ptr = unsafe { ptr::addr_of!((*storage.as_ptr()).sun_path) };
        let length = length as usize - (storage.as_ptr().addr() - path_ptr.addr());
        // SAFETY: the kernel ensures that at least `length` bytes are
        // initialised.
        let path = unsafe { slice::from_raw_parts::<u8>(path_ptr.cast(), length) };
        #[cfg(any(target_os = "android", target_os = "linux"))]
        if let Some(0) = path.first() {
            // NOTE: `from_abstract_name` adds a starting null byte.
            if let Ok(addr) = unix::net::SocketAddr::from_abstract_name(&path[1..]) {
                return addr;
            }
        }

        unix::net::SocketAddr::from_pathname(Path::new(OsStr::from_bytes(path)))
            // Fallback to an unnamed address.
            .unwrap_or_else(|_| unix::net::SocketAddr::from_pathname("").unwrap())
    }

    fn domain(&self) -> Domain {
        Domain::UNIX
    }
}

/// When [`accept`]ing connections we're not interested in the address.
///
/// This is not acceptable in calls to [`connect`].
///
/// [`accept`]: AsyncFd::accept
/// [`connect`]: AsyncFd::connect
#[derive(Copy, Clone, Debug)]
pub struct NoAddress;

impl SocketAddress for NoAddress {}

impl private::SocketAddress for NoAddress {
    type Storage = Self;

    fn into_storage(self) -> Self::Storage {
        NoAddress
    }

    unsafe fn as_ptr(_: &Self::Storage) -> (*const libc::sockaddr, libc::socklen_t) {
        // NOTE: this goes against the requirements of `cast_ptr`.
        (ptr::null_mut(), 0)
    }

    unsafe fn as_mut_ptr(
        _: &mut MaybeUninit<Self::Storage>,
    ) -> (*mut libc::sockaddr, libc::socklen_t) {
        // NOTE: this goes against the requirements of `as_mut_ptr`.
        (ptr::null_mut(), 0)
    }

    unsafe fn init(_: MaybeUninit<Self>, length: libc::socklen_t) -> Self {
        debug_assert!(length == 0);
        NoAddress
    }

    fn domain(&self) -> Domain {
        Domain(libc::AF_UNSPEC)
    }
}

/// Implement [`fmt::Debug`] for [`SocketAddress::Storage`].
///
/// [`SocketAddress::Storage`]: private::SocketAddress::Storage
pub(crate) struct AddressStorage<A>(pub(crate) A);

impl<A> fmt::Debug for AddressStorage<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Address").finish()
    }
}

/// Implement [`fmt::Debug`] for [`SockOpt::Storage`].
///
/// [`SockOpt::Storage`]: private::SockOpt::Storage
pub(crate) struct OptionStorage<T>(pub(crate) T);

impl<T> fmt::Debug for OptionStorage<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Option").finish()
    }
}

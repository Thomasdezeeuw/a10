//! Networking.
//!
//! To create a new socket ([`AsyncFd`]) use the [`socket`] function, which
//! issues a non-blocking `socket(2)` call.

use std::ffi::OsStr;
use std::future::Future;
use std::mem::{self, MaybeUninit};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::os::linux::net::SocketAddrExt;
use std::os::unix;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::pin::Pin;
use std::task::{self, Poll};
use std::{fmt, io, ptr, slice};

use crate::cancel::{Cancel, CancelOperation, CancelResult};
use crate::fd::{AsyncFd, Descriptor, File};
use crate::io::{Buf, BufMut, Buffer, ReadNBuf};
use crate::op::{fd_operation, operation, FdOperation, Operation};
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

/// Socket related system calls.
impl<D: Descriptor> AsyncFd<D> {
    /// Initiate a connection on this socket to the specified address.
    #[doc = man_link!(connect(2))]
    pub fn connect<'fd, A>(&'fd self, address: A) -> Connect<'fd, A, D>
    where
        A: SocketAddress,
    {
        let storage = AddressStorage(Box::from(address.as_storage()));
        Connect(FdOperation::new(self, storage, ()))
    }

    /// Receives data on the socket from the remote address to which it is
    /// connected.
    #[doc = man_link!(recv(2))]
    pub const fn recv<'fd, B>(&'fd self, buf: B, flags: libc::c_int) -> Recv<'fd, B, D>
    where
        B: BufMut,
    {
        let buf = Buffer { buf };
        Recv(FdOperation::new(self, buf, flags))
    }

    /// Receives at least `n` bytes on the socket from the remote address to
    /// which it is connected.
    pub const fn recv_n<'fd, B>(&'fd self, buf: B, n: usize) -> RecvN<'fd, B, D>
    where
        B: BufMut,
    {
        let buf = ReadNBuf { buf, last_read: 0 };
        RecvN {
            recv: self.recv(buf, 0),
            left: n,
        }
    }

    /// Sends data on the socket to a connected peer.
    #[doc = man_link!(send(2))]
    pub const fn send<'fd, B>(&'fd self, buf: B, flags: libc::c_int) -> Send<'fd, B, D>
    where
        B: Buf,
    {
        let buf = Buffer { buf };
        Send(FdOperation::new(self, buf, (SendCall::Normal, flags)))
    }

    /// Same as [`AsyncFd::send`], but tries to avoid making intermediate copies
    /// of `buf`.
    ///
    /// # Notes
    ///
    /// Zerocopy execution is not guaranteed and may fall back to copying. The
    /// request may also fail with `EOPNOTSUPP`, when a protocol doesn't support
    /// zerocopy, in which case users are recommended to use [`AsyncFd::send`]
    /// instead.
    ///
    /// The `Future` only returns once it safe for the buffer to be used again,
    /// for TCP for example this means until the data is ACKed by the peer.
    pub const fn send_zc<'fd, B>(&'fd self, buf: B, flags: libc::c_int) -> Send<'fd, B, D>
    where
        B: Buf,
    {
        let buf = Buffer { buf };
        Send(FdOperation::new(self, buf, (SendCall::ZeroCopy, flags)))
    }
}

#[derive(Copy, Clone, Debug)]
pub(crate) enum SendCall {
    Normal,
    ZeroCopy,
}

fd_operation! {
    /// [`Future`] behind [`AsyncFd::connect`].
    pub struct Connect<A: SocketAddress>(sys::net::ConnectOp<A>) -> io::Result<()>;

    /// [`Future`] behind [`AsyncFd::recv`].
    pub struct Recv<B: BufMut>(sys::net::RecvOp<B>) -> io::Result<B>;

    /// [`Future`] behind [`AsyncFd::send`] and [`AsyncFd::send_zc`].
    pub struct Send<B: Buf>(sys::net::SendOp<B>) -> io::Result<usize>,
      with Extract -> io::Result<(B, usize)>;
}

/// [`Future`] behind [`AsyncFd::recv_n`].
#[derive(Debug)]
pub struct RecvN<'fd, B: BufMut, D: Descriptor = File> {
    recv: Recv<'fd, ReadNBuf<B>, D>,
    /// Number of bytes we still need to receive to hit our target `N`.
    left: usize,
}

impl<'fd, B: BufMut, D: Descriptor> Cancel for RecvN<'fd, B, D> {
    fn try_cancel(&mut self) -> CancelResult {
        self.recv.try_cancel()
    }

    fn cancel(&mut self) -> CancelOperation {
        self.recv.cancel()
    }
}

impl<'fd, B: BufMut, D: Descriptor> Future for RecvN<'fd, B, D> {
    type Output = io::Result<B>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        // SAFETY: not moving the `Recv` future.
        let this = unsafe { Pin::into_inner_unchecked(self) };
        let mut recv = unsafe { Pin::new_unchecked(&mut this.recv) };
        match recv.as_mut().poll(ctx) {
            Poll::Ready(Ok(buf)) => {
                if buf.last_read == 0 {
                    return Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into()));
                }

                if buf.last_read >= this.left {
                    // Received the required amount of bytes.
                    return Poll::Ready(Ok(buf.buf));
                }

                this.left -= buf.last_read;

                recv.set(recv.0.fd().recv(buf, 0));
                unsafe { Pin::new_unchecked(this) }.poll(ctx)
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

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
#[derive(Copy, Clone, Debug)]
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

/// Implement [`fmt::Debug`] for [`SocketAddress::Storage`].
pub(crate) struct AddressStorage<A>(pub(crate) A);

impl<A> fmt::Debug for AddressStorage<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Address").finish()
    }
}

//! This is the same example as found in `http_client.rs`, with one difference,
//! this uses linking. Using linked operations multiple operations will be
//! linked to gether for even more performance gains.

use std::future::{Future, IntoFuture};
use std::net::{SocketAddr, SocketAddrV4};
use std::pin::Pin;
use std::task::{self, Poll};
use std::{io, mem, ptr, str};

const HTTP_REQUEST: &str = "GET / HTTP/1.1\r\nHost: thomasdezeeuw.nl\r\nUser-Agent: a10-example/0.1.0\r\nAccept: */*\r\n\r\n";

fn main() -> io::Result<()> {
    // Create a new I/O uring.
    let mut ring = a10::Ring::new(8)?;

    // Get an IPv4 address for the domain (using blocking I/O).
    let address = std::net::ToSocketAddrs::to_socket_addrs("thomasdezeeuw.nl:80")?
        .filter(SocketAddr::is_ipv4)
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "failed to lookup ip"))?;
    let (address, address_length) = match address {
        SocketAddr::V4(address) => to_sockaddr_storage(address),
        SocketAddr::V6(_) => unreachable!(),
    };

    // Create a new socket, connect to `address`, send our `HTTP_REQUEST` and
    // receive the response all in a chain of operations.
    let domain = libc::AF_INET;
    let r#type = libc::SOCK_STREAM | libc::SOCK_CLOEXEC;
    let protocol = 0;
    let flags = 0;
    let future = a10::net::socket(ring.submission_queue(), domain, r#type, protocol, flags)?
        .connect(address, address_length)?
        .extract()
        .send(HTTP_REQUEST.into())?
        .extract()
        .recv(Vec::with_capacity(8192))?;
    let (recv_buf, send_buf, send_size, address_buf, socket) = block_on(&mut ring, future)??;

    // Done receiving, we'll print the result (using ol' fashioned blocking I/O).
    let response = str::from_utf8(&recv_buf).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("response doesn't contain UTF-8: {}", err),
        )
    })?;
    println!("{response}");

    Ok(())
}

fn to_sockaddr_storage(addr: SocketAddrV4) -> (libc::sockaddr_storage, libc::socklen_t) {
    // SAFETY: a `sockaddr_storage` of all zeros is valid.
    let mut storage: libc::sockaddr_storage = unsafe { mem::zeroed() };
    let len = {
        let storage: &mut libc::sockaddr_in = unsafe { &mut *ptr::addr_of_mut!(storage).cast() };
        storage.sin_family = libc::AF_INET as _;
        storage.sin_port = addr.port().to_be();
        storage.sin_addr = libc::in_addr {
            s_addr: u32::from_ne_bytes(addr.ip().octets()),
        };
        storage.sin_zero = Default::default();
        mem::size_of::<libc::sockaddr_in>() as _
    };
    (storage, len)
}

/// Replace this with your favorite [`Future`] runtime.
fn block_on<Fut>(ring: &mut a10::Ring, fut: Fut) -> io::Result<Fut::Output>
where
    Fut: IntoFuture,
    Fut::IntoFuture: Unpin,
{
    let waker = noop_waker();
    let mut ctx = task::Context::from_waker(&waker);
    let mut fut = fut.into_future();
    let mut fut = Pin::new(&mut fut);
    loop {
        if let Poll::Ready(result) = fut.as_mut().poll(&mut ctx) {
            return Ok(result);
        }

        ring.poll(None)?;
    }
}

fn noop_waker() -> task::Waker {
    use std::task::{RawWaker, RawWakerVTable};
    static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
        |_| RawWaker::new(ptr::null(), &WAKER_VTABLE),
        |_| {},
        |_| {},
        |_| {},
    );
    unsafe { task::Waker::from_raw(RawWaker::new(ptr::null(), &WAKER_VTABLE)) }
}

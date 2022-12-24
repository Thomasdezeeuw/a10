use std::future::Future;
use std::net::{SocketAddr, SocketAddrV4};
use std::pin::Pin;
use std::task::{self, Poll};
use std::{io, mem, ptr, str};

use a10::net::socket;
use a10::{Ring, SubmissionQueue};

const HTTP_REQUEST: &str = "GET / HTTP/1.1\r\nHost: thomasdezeeuw.nl\r\nUser-Agent: a10-example/0.1.0\r\nAccept: */*\r\n\r\n";

fn main() -> io::Result<()> {
    // Create a new I/O uring.
    let mut ring = Ring::new(2)?;

    // Get an IPv4 address for the domain (using blocking I/O).
    let address = std::net::ToSocketAddrs::to_socket_addrs("thomasdezeeuw.nl:80")?
        .filter(SocketAddr::is_ipv4)
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "failed to lookup ip"))?;
    let address = match address {
        SocketAddr::V4(address) => address,
        SocketAddr::V6(_) => unreachable!(),
    };

    // Create our future that makes the request.
    let mut request = Box::pin(request(ring.submission_queue(), address));

    // This loop will represent our `Future`s runtime, in practice use an actual
    // implementation.
    let response = loop {
        match poll_future(request.as_mut()) {
            Poll::Ready(res) => break res?,
            Poll::Pending => {
                // Poll the `Ring` to get an update on the operation (s).
                //
                // In pratice you would first yield to another future, but in
                // this example we don't have one, so we'll always poll the
                // `Ring`.
                ring.poll(None)?;
            }
        }
    };

    // We'll print the response (using ol' fashioned blocking I/O).
    let response = str::from_utf8(&response).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("response doesn't contain UTF-8: {}", err),
        )
    })?;
    println!("{response}");

    Ok(())
}

/// Make a HTTP GET request to `address`.
async fn request(sq: SubmissionQueue, address: SocketAddrV4) -> io::Result<Vec<u8>> {
    // Create a new TCP, IPv4 socket.
    let domain = libc::AF_INET;
    let r#type = libc::SOCK_STREAM | libc::SOCK_CLOEXEC;
    let protocol = 0;
    let flags = 0;
    let socket = socket(sq, domain, r#type, protocol, flags)?.await?;

    // Connect.
    let (addr, addr_len) = to_sockaddr_storage(address);
    socket.connect(addr, addr_len)?.await?;

    // Send a HTTP GET / request to the socket.
    let n = socket.send(HTTP_REQUEST)?.await?;
    assert!(n == HTTP_REQUEST.len()); // In practice try reading again.

    // Receiving the response.
    let recv_buf = socket.recv(Vec::with_capacity(8192))?.await?;

    // We'll explicitly close the socket, although that happens for us when we
    // drop the socket. In other words, this is not needed.
    socket.close()?.await?;

    Ok(recv_buf)
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
fn poll_future<Fut>(fut: Pin<&mut Fut>) -> Poll<Fut::Output>
where
    Fut: Future,
{
    use std::task::{RawWaker, RawWakerVTable};
    static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
        |_| RawWaker::new(ptr::null(), &WAKER_VTABLE),
        |_| {},
        |_| {},
        |_| {},
    );
    let waker = unsafe { task::Waker::from_raw(RawWaker::new(ptr::null(), &WAKER_VTABLE)) };
    let mut ctx = task::Context::from_waker(&waker);
    fut.poll(&mut ctx)
}

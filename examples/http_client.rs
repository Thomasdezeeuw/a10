use std::future::Future;
use std::net::{SocketAddr, SocketAddrV4};
use std::pin::Pin;
use std::task::{self, Poll};
use std::{io, mem, ptr, str};

use a10::{AsyncFd, Ring};

fn main() -> io::Result<()> {
    // Create a new I/O uring.
    let mut ring = Ring::new(1)?;

    // Manually create a socket. You'll want to use the socket2 library for
    // this.
    // SAFETY: system call.
    let socket = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
    if socket == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: just created the socket above.
    let socket = unsafe { AsyncFd::new(socket, ring.submission_queue()) };

    // Start a connect call.
    let address = std::net::ToSocketAddrs::to_socket_addrs("thomasdezeeuw.nl:80")?
        .filter(SocketAddr::is_ipv4)
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "failed to lookup ip"))?;
    let (address, address_length) = match address {
        SocketAddr::V4(address) => to_sockaddr_storage(address),
        SocketAddr::V6(_) => unreachable!(),
    };
    let connect = socket.connect(address, address_length)?;

    // Poll the ring and check if the socket is connected.
    ring.poll(None)?;
    block_on(connect)?; // Replace this with a `.await`.

    // Start aysynchronously sending a HTTP `GET /` request to the socket.
    let request = format!("GET / HTTP/1.1\r\nHost: thomasdezeeuw.nl\r\nUser-Agent: curl/7.79.1\r\nAccept: */*\r\n\r\n");
    let send = socket.send(request.into())?;
    ring.poll(None)?;
    block_on(send)?;

    // Start aysynchronously receinv the response.
    let recv = socket.recv(Vec::with_capacity(8192))?;
    ring.poll(None)?;
    let buf = block_on(recv)?;

    // Done receivinreceivingg, we'll print the result (using ol' fashioned blocking I/O).
    let data = str::from_utf8(&buf).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("file doesn't contain UTF-8: {}", err),
        )
    })?;
    println!("{data}");

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
fn block_on<Fut>(mut fut: Fut) -> Fut::Output
where
    Fut: Future + Unpin,
{
    let waker = noop_waker();
    let mut ctx = task::Context::from_waker(&waker);
    let mut fut = Pin::new(&mut fut);
    loop {
        if let Poll::Ready(result) = fut.as_mut().poll(&mut ctx) {
            return result;
        }
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

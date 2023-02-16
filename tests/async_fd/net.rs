//! Tests for the networking operations.

use std::io::{self, Read, Write};
use std::mem::{self, size_of};
use std::net::{
    Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV4, SocketAddrV6, TcpListener, TcpStream,
};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::pin::Pin;
use std::ptr;

use a10::io::{CancelResult, ReadBufPool};
use a10::{Extract, Ring};

use crate::util::{
    bind_ipv4, block_on, init, next, poll_nop, syscall, tcp_ipv4_socket, test_queue, Waker,
};

const DATA1: &[u8] = b"Hello, World!";
const DATA2: &[u8] = b"Hello, Mars!";

#[test]
fn accept() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = waker.block_on(tcp_ipv4_socket(sq));
    let local_addr = bind_ipv4(&listener);

    // Accept a connection.
    let mut stream = TcpStream::connect(local_addr).expect("failed to connect");
    let accept = listener.accept();
    let (client, addr) = waker.block_on(accept).expect("failed to accept connection");
    assert_eq!(stream.peer_addr().unwrap(), local_addr);
    assert_eq!(stream.local_addr().unwrap(), addr);

    // Read some data.
    stream.write(DATA1).expect("failed to write");
    let mut buf = waker
        .block_on(client.read(Vec::with_capacity(DATA1.len() + 1)))
        .expect("failed to read");
    assert_eq!(buf, DATA1);

    // Write some data.
    let n = waker
        .block_on(client.write(DATA2))
        .expect("failed to write");
    assert_eq!(n, DATA2.len());
    buf.resize(DATA2.len() + 1, 0);
    let n = stream.read(&mut buf).expect("failed to read");
    assert_eq!(&buf[..n], DATA2);

    // Closing the client should get a result.
    drop(stream);
    buf.clear();
    let buf = waker.block_on(client.read(buf)).expect("failed to read");
    assert!(buf.is_empty());
}

#[test]
fn multishot_accept() {
    test_multishot_accept(0);
    test_multishot_accept(1);
    test_multishot_accept(5);

    fn test_multishot_accept(n: usize) {
        let sq = test_queue();
        let waker = Waker::new();

        // Bind a socket.
        let listener = waker.block_on(tcp_ipv4_socket(sq));
        let local_addr = bind_ipv4(&listener);

        let mut accept_stream = listener.multishot_accept();

        // Create connections and accept them.
        let streams = (0..n)
            .map(|_| {
                let stream = TcpStream::connect(local_addr).expect("failed to connect");
                let addr = stream.local_addr().expect("failed to get address");
                (stream, addr)
            })
            .collect::<Vec<_>>();
        let mut clients = (0..n)
            .map(|_| {
                let client = waker
                    .block_on(next(&mut accept_stream))
                    .expect("missing a connection")
                    .expect("failed to accept connection");
                let addr = peer_addr(client.as_fd()).expect("failed to get address");
                (client, addr)
            })
            .collect::<Vec<_>>();

        // Make sure we use the correct stream, client pair.
        let mut tests = Vec::with_capacity(clients.len());
        for (stream, addr) in streams {
            let idx = clients
                .iter()
                .position(|(_, a)| *a == addr)
                .expect("failed to find client");
            let client = clients.remove(idx);
            tests.push((stream, client.0));
        }

        // Test each connection.
        for (mut stream, client) in tests {
            // Read some data.
            stream.write(DATA1).expect("failed to write");
            let mut buf = waker
                .block_on(client.read(Vec::with_capacity(DATA1.len() + 1)))
                .expect("failed to read");
            assert_eq!(buf, DATA1);

            // Write some data.
            let n = waker
                .block_on(client.write(DATA2))
                .expect("failed to write");
            assert_eq!(n, DATA2.len());
            buf.resize(DATA2.len() + 1, 0);
            let n = stream.read(&mut buf).expect("failed to read");
            assert_eq!(&buf[..n], DATA2);

            // Closing the client should get a result.
            drop(stream);
            buf.clear();
            let buf = waker.block_on(client.read(buf)).expect("failed to read");
            assert!(buf.is_empty());
        }
    }
}

#[test]
fn cancel_multishot_accept() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = waker.block_on(tcp_ipv4_socket(sq));
    let local_addr = bind_ipv4(&listener);

    let mut accept_stream = listener.multishot_accept();

    // Start two connections.
    let stream1 = TcpStream::connect(local_addr).expect("failed to connect");
    let s_addr1 = stream1.local_addr().expect("failed to get address");
    let stream2 = TcpStream::connect(local_addr).expect("failed to connect");

    // Accept the first.
    let client1 = waker
        .block_on(next(&mut accept_stream))
        .expect("missing a connection")
        .expect("failed to accept connection");
    let c_addr1 = peer_addr(client1.as_fd()).expect("failed to get address");

    // Then cancel the accept multishot call.
    waker.block_on(accept_stream.cancel());

    // We should still be able to accept the second connection.
    let client2 = waker
        .block_on(next(&mut accept_stream))
        .expect("missing a connection")
        .expect("failed to accept connection");

    // After that we expect no more connections.
    assert!(waker.block_on(next(&mut accept_stream)).is_none());

    // Match the connections.
    let tests = if s_addr1 == c_addr1 {
        [(stream1, client1), (stream2, client2)]
    } else {
        [(stream1, client2), (stream2, client1)]
    };

    // Test each connection.
    for (mut stream, client) in tests {
        // Read some data.
        stream.write(DATA1).expect("failed to write");
        let mut buf = waker
            .block_on(client.read(Vec::with_capacity(DATA1.len() + 1)))
            .expect("failed to read");
        assert_eq!(buf, DATA1);

        // Write some data.
        let n = waker
            .block_on(client.write(DATA2))
            .expect("failed to write");
        assert_eq!(n, DATA2.len());
        buf.resize(DATA2.len() + 1, 0);
        let n = stream.read(&mut buf).expect("failed to read");
        assert_eq!(&buf[..n], DATA2);

        // Closing the client should get a result.
        drop(stream);
        buf.clear();
        let buf = waker.block_on(client.read(buf)).expect("failed to read");
        assert!(buf.is_empty());
    }
}

#[test]
fn try_cancel_multishot_accept_before_poll() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = waker.block_on(tcp_ipv4_socket(sq));
    let local_addr = bind_ipv4(&listener);

    let mut accept_stream = listener.multishot_accept();

    // Start a connection.
    let _stream = TcpStream::connect(local_addr).expect("failed to connect");

    // But before we accept we cancel the accept call.
    if !matches!(accept_stream.try_cancel(), CancelResult::NotStarted) {
        panic!("failed to cancel");
    }
}

#[test]
fn multishot_accept_incorrect_usage() {
    let sq = test_queue();
    let waker = Waker::new();

    // Create a socket, but don't bind it.
    let listener = waker.block_on(tcp_ipv4_socket(sq));

    let mut accept_stream = listener.multishot_accept();

    let res = waker.block_on(next(&mut accept_stream)).unwrap();
    assert!(res.is_err(), "unexpected ok result: {:?}", res);
    assert!(waker.block_on(next(&mut accept_stream)).is_none());
}

#[test]
fn connect() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = {
        let mut addr: libc::sockaddr_storage = unsafe { mem::zeroed() };
        let a = unsafe { &mut *(&mut addr as *mut _ as *mut libc::sockaddr_in) };
        a.sin_family = libc::AF_INET as libc::sa_family_t;
        a.sin_port = local_addr.port().to_be();
        a.sin_addr = libc::in_addr {
            s_addr: u32::from_ne_bytes(local_addr.ip().octets()),
        };
        addr
    };
    let addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let mut connect_future = stream.connect(addr, addr_len);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    waker.block_on(connect_future).expect("failed to connect");

    // Write some data.
    waker
        .block_on(stream.write(DATA1))
        .expect("failed to write");
    let mut buf = vec![0; DATA1.len() + 1];
    let n = client.read(&mut buf).expect("failed to read");
    assert_eq!(&buf[0..n], DATA1);

    // Read some data.
    client.write_all(DATA2).expect("failed to write");
    buf.clear();
    buf.reserve(DATA2.len() + 1);
    let mut buf = waker.block_on(stream.read(buf)).expect("failed to read");
    assert_eq!(buf, DATA2);

    // Dropping the stream should closing it.
    drop(stream);
    let n = client.read(&mut buf).expect("failed to read");
    assert_eq!(n, 0);
}

#[test]
fn connect_extractor() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let mut connect_future = stream.connect(addr, addr_len).extract();
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    let _addr = waker.block_on(connect_future).expect("failed to connect");

    // Write some data.
    waker
        .block_on(stream.write(DATA1))
        .expect("failed to write");
    let mut buf = vec![0; DATA1.len() + 1];
    let n = client.read(&mut buf).expect("failed to read");
    assert_eq!(&buf[0..n], DATA1);

    // Read some data.
    client.write_all(DATA2).expect("failed to write");
    buf.clear();
    buf.reserve(DATA2.len() + 1);
    let buf = waker.block_on(stream.read(buf)).expect("failed to read");
    assert_eq!(buf, DATA2);
}

#[test]
fn recv() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let mut connect_future = stream.connect(addr, addr_len);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    waker.block_on(connect_future).expect("failed to connect");

    // Receive some data.
    let recv_future = stream.recv(Vec::with_capacity(DATA1.len() + 1), 0);
    client.write_all(DATA1).expect("failed to send data");
    let mut buf = waker.block_on(recv_future).expect("failed to receive");
    assert_eq!(&buf, DATA1);

    // We should detect the peer closing the stream.
    drop(client);
    buf.clear();
    let buf = waker
        .block_on(stream.recv(buf, 0))
        .expect("failed to receive");
    assert!(buf.is_empty());
}

#[test]
fn recv_read_buf_pool() {
    const BUF_SIZE: usize = 4096;

    init();
    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();
    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = block_on(&mut ring, tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let mut connect_future = stream.connect(addr, addr_len);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    block_on(&mut ring, connect_future).expect("failed to connect");

    // Receive some data.
    let recv_future = stream.recv(buf_pool.get(), 0);
    client.write_all(DATA1).expect("failed to send data");
    let buf = block_on(&mut ring, recv_future).expect("failed to receive");
    assert_eq!(buf.as_slice(), DATA1);

    // We should detect the peer closing the stream.
    drop(client);
    let buf = block_on(&mut ring, stream.recv(buf_pool.get(), 0)).expect("failed to receive");
    assert!(buf.is_empty());
}

#[test]
fn recv_read_buf_pool_send_read_buf() {
    const BUF_SIZE: usize = 4096;

    init();
    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();
    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = block_on(&mut ring, tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let mut connect_future = stream.connect(addr, addr_len);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    block_on(&mut ring, connect_future).expect("failed to connect");

    // Receive some data.
    let recv_future = stream.recv(buf_pool.get(), 0);
    client.write_all(DATA1).expect("failed to send data");
    let buf = block_on(&mut ring, recv_future).expect("failed to receive");
    assert_eq!(buf.as_slice(), DATA1);

    // Send the data back.
    let n = block_on(&mut ring, stream.send(buf, 0)).expect("failed to send");
    assert_eq!(n, DATA1.len());
    let mut buf = vec![0; DATA1.len() + 1];
    let n = client.read(&mut buf).expect("failed to read data");
    assert_eq!(n, DATA1.len());
    assert_eq!(&buf[0..n], DATA1);

    // We should detect the peer closing the stream.
    drop(client);
    let buf = block_on(&mut ring, stream.recv(buf_pool.get(), 0)).expect("failed to receive");
    assert!(buf.is_empty());
}

#[test]
fn send() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let mut connect_future = stream.connect(addr, addr_len);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    waker.block_on(connect_future).expect("failed to connect");

    // Send some data.
    let n = waker
        .block_on(stream.send(DATA2, 0))
        .expect("failed to send");
    assert_eq!(n, DATA2.len());
    let mut buf = vec![0; DATA2.len() + 2];
    let n = client.read(&mut buf).expect("failed to send data");
    assert_eq!(&buf[0..n], DATA2);
}

#[test]
fn send_zc() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let mut connect_future = stream.connect(addr, addr_len);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    waker.block_on(connect_future).expect("failed to connect");

    // Send some data.
    let n = waker
        .block_on(stream.send_zc(DATA2, 0))
        .expect("failed to send");
    assert_eq!(n, DATA2.len());
    let mut buf = vec![0; DATA2.len() + 2];
    let n = client.read(&mut buf).expect("failed to send data");
    assert_eq!(&buf[0..n], DATA2);
}

#[test]
fn send_extractor() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let mut connect_future = stream.connect(addr, addr_len);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    waker.block_on(connect_future).expect("failed to connect");

    // Send some data.
    let (buf, n) = waker
        .block_on(stream.send(DATA2, 0).extract())
        .expect("failed to send");
    assert_eq!(buf, DATA2);
    assert_eq!(n, DATA2.len());
    let mut buf = vec![0; DATA2.len() + 2];
    let n = client.read(&mut buf).expect("failed to send data");
    assert_eq!(&buf[0..n], DATA2);
}

#[test]
fn send_zc_extractor() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let mut connect_future = stream.connect(addr, addr_len);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    waker.block_on(connect_future).expect("failed to connect");

    // Send some data.
    let (buf, n) = waker
        .block_on(stream.send_zc(DATA2, 0).extract())
        .expect("failed to send");
    assert_eq!(buf, DATA2);
    assert_eq!(n, DATA2.len());
    let mut buf = vec![0; DATA2.len() + 2];
    let n = client.read(&mut buf).expect("failed to send data");
    assert_eq!(&buf[0..n], DATA2);
}

#[test]
fn shutdown() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let mut connect_future = stream.connect(addr, addr_len);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    waker.block_on(connect_future).expect("failed to connect");

    waker
        .block_on(stream.shutdown(Shutdown::Write))
        .expect("failed to shutdown");
    let mut buf = vec![0; 10];
    let n = client.read(&mut buf).expect("failed to send data");
    assert_eq!(n, 0);
}

fn addr_storage(addres: &SocketAddrV4) -> libc::sockaddr_storage {
    // SAFETY: zeroed out `sockaddr_storage` is valid.
    let mut addr: libc::sockaddr_storage = unsafe { mem::zeroed() };
    addr.ss_family = libc::AF_INET as libc::sa_family_t;
    // SAFETY: `sockaddr_in` is a valid variant size we se `AF_INET` above.
    let a = unsafe { &mut *(&mut addr as *mut _ as *mut libc::sockaddr_in) };
    a.sin_port = addres.port().to_be();
    a.sin_addr = libc::in_addr {
        s_addr: u32::from_ne_bytes(addres.ip().octets()),
    };
    addr
}

fn peer_addr(fd: BorrowedFd) -> io::Result<SocketAddr> {
    let mut storage: libc::sockaddr_storage = unsafe { mem::zeroed() };
    let mut len = size_of::<libc::sockaddr_storage>() as u32;
    syscall!(getpeername(
        fd.as_raw_fd(),
        ptr::addr_of_mut!(storage).cast::<libc::sockaddr>(),
        &mut len
    ))?;
    if storage.ss_family == libc::AF_INET as libc::sa_family_t {
        let storage = unsafe { &mut *ptr::addr_of_mut!(storage).cast::<libc::sockaddr_in>() };
        let addr = Ipv4Addr::from(storage.sin_addr.s_addr.to_ne_bytes());
        let port = storage.sin_port.to_be();
        Ok(SocketAddr::V4(SocketAddrV4::new(addr, port)))
    } else {
        let storage = unsafe { &mut *ptr::addr_of_mut!(storage).cast::<libc::sockaddr_in6>() };
        let addr = Ipv6Addr::from(storage.sin6_addr.s6_addr);
        let port = storage.sin6_port.to_be();
        let flowinfo = storage.sin6_flowinfo;
        let scope_id = storage.sin6_scope_id;
        Ok(SocketAddr::V6(SocketAddrV6::new(
            addr, port, flowinfo, scope_id,
        )))
    }
}

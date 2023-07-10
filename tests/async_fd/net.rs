//! Tests for the networking operations.

use std::cell::Cell;
use std::io::{self, Read, Write};
use std::mem::{self, size_of};
use std::net::{
    Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV4, SocketAddrV6, TcpListener, TcpStream,
    UdpSocket,
};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::pin::Pin;
use std::ptr;

use a10::cancel::{Cancel, CancelResult};
use a10::io::ReadBufPool;
use a10::net::{
    Accept, MultishotAccept, MultishotRecv, NoAddress, Recv, RecvN, RecvNVectored, Send, SendAll,
    SendAllVectored, SendTo, Socket,
};
use a10::{Extract, Ring};

use crate::async_fd::io::{BadBuf, BadBufSlice, BadReadBuf, BadReadBufSlice};
use crate::util::{
    bind_and_listen_ipv4, bind_ipv4, block_on, expect_io_errno, expect_io_error_kind, init,
    is_send, is_sync, next, poll_nop, require_kernel, syscall, tcp_ipv4_socket, test_queue,
    udp_ipv4_socket, Waker,
};

const DATA1: &[u8] = b"Hello, World!";
const DATA2: &[u8] = b"Hello, Mars!";

#[test]
fn accept() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<Accept<libc::sockaddr_storage>>();
    is_sync::<Accept<libc::sockaddr_storage>>();

    // Bind a socket.
    let listener = waker.block_on(tcp_ipv4_socket(sq));
    let local_addr = bind_and_listen_ipv4(&listener);

    // Accept a connection.
    let mut stream = TcpStream::connect(local_addr).expect("failed to connect");
    let accept = listener.accept::<libc::sockaddr_in>();
    let (client, addr) = waker.block_on(accept).expect("failed to accept connection");
    let address = from_storage(addr);
    assert_eq!(stream.peer_addr().unwrap(), local_addr);
    assert_eq!(stream.local_addr().unwrap(), address.into());

    // Read some data.
    stream.write(DATA1).expect("failed to write");
    let mut buf = waker
        .block_on(client.read(Vec::with_capacity(DATA1.len() + 1)))
        .expect("failed to read");
    assert_eq!(buf, DATA1);

    // Write some data.
    let n = waker
        .block_on(client.write(Box::from(DATA2))) // Test Buf impl for Box<[u8]>.
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
fn accept_no_address() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<Accept<libc::sockaddr_storage>>();
    is_sync::<Accept<libc::sockaddr_storage>>();

    // Bind a socket.
    let listener = waker.block_on(tcp_ipv4_socket(sq));
    let local_addr = bind_and_listen_ipv4(&listener);

    // Accept a connection.
    let mut stream = TcpStream::connect(local_addr).expect("failed to connect");
    assert_eq!(stream.peer_addr().unwrap(), local_addr);
    let accept = listener.accept::<NoAddress>();
    let (client, _) = waker.block_on(accept).expect("failed to accept connection");

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
fn cancel_accept() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = waker.block_on(tcp_ipv4_socket(sq));
    bind_and_listen_ipv4(&listener);

    let accept = listener.accept::<(libc::sockaddr_storage, libc::socklen_t)>();
    let mut accept = std::pin::pin!(accept);

    // Poll once to start the operation.
    assert!(poll_nop(accept.as_mut()).is_pending());

    // Then cancel the accept multishot call.
    waker
        .block_on(accept.as_mut().cancel())
        .expect("failed to cancel");

    expect_io_errno(waker.block_on(accept), libc::ECANCELED);
}

#[test]
fn try_cancel_accept_before_poll() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = waker.block_on(tcp_ipv4_socket(sq));
    bind_and_listen_ipv4(&listener);

    let mut accept = listener.accept::<libc::sockaddr_storage>();

    // Before we accept we cancel the accept call.
    if !matches!(accept.try_cancel(), CancelResult::NotStarted) {
        panic!("failed to cancel");
    }
}

#[test]
fn multishot_accept() {
    test_multishot_accept(0);
    test_multishot_accept(1);
    test_multishot_accept(5);

    fn test_multishot_accept(n: usize) {
        let sq = test_queue();
        let waker = Waker::new();

        is_send::<MultishotAccept>();
        is_sync::<MultishotAccept>();

        // Bind a socket.
        let listener = waker.block_on(tcp_ipv4_socket(sq));
        let local_addr = bind_and_listen_ipv4(&listener);

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
    let local_addr = bind_and_listen_ipv4(&listener);

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
    waker
        .block_on(accept_stream.cancel())
        .expect("failed to cancel");

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
    require_kernel!(5, 19);

    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = waker.block_on(tcp_ipv4_socket(sq));
    let local_addr = bind_and_listen_ipv4(&listener);

    let mut accept_stream = listener.multishot_accept();

    // Start a connection.
    let _stream = TcpStream::connect(local_addr).expect("failed to connect");

    // But before we accept we cancel the accept call.
    if !matches!(accept_stream.try_cancel(), CancelResult::NotStarted) {
        panic!("failed to cancel");
    }
}

#[test]
fn cancel_multishot_accept_before_poll() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = waker.block_on(tcp_ipv4_socket(sq));
    bind_and_listen_ipv4(&listener);

    let mut accept_stream = listener.multishot_accept();

    expect_io_errno(waker.block_on(accept_stream.cancel()), libc::ENOENT)
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

    is_send::<Socket>();
    is_sync::<Socket>();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let mut connect_future = stream.connect(addr);
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
    let mut connect_future = stream.connect(addr).extract();
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

    is_send::<Recv<Vec<u8>>>();
    is_sync::<Recv<Vec<u8>>>();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let mut connect_future = stream.connect(addr);
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
    let mut connect_future = stream.connect(addr);
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
    let mut connect_future = stream.connect(addr);
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
fn multishot_recv() {
    const BUF_SIZE: usize = 512;
    const BUFS: usize = 2;
    init();

    is_send::<MultishotRecv>();
    is_sync::<MultishotRecv>();

    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();
    let buf_pool = ReadBufPool::new(sq.clone(), BUFS as u16, BUF_SIZE as u32).unwrap();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = block_on(&mut ring, tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let mut connect_future = stream.connect(addr);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());
    let (mut client, _) = listener.accept().expect("failed to accept connection");
    block_on(&mut ring, connect_future).expect("failed to connect");

    let mut stream_recv = stream.multishot_recv(buf_pool, 0);

    // Write some data and read it back.
    client.write_all(DATA1).expect("failed to write");
    let buf = block_on(&mut ring, next(&mut stream_recv))
        .unwrap()
        .expect("failed to receive");
    assert_eq!(&*buf, DATA1);

    client.shutdown(Shutdown::Write).unwrap();

    let buf = block_on(&mut ring, next(&mut stream_recv))
        .unwrap()
        .expect("failed to receive");
    assert!(buf.is_empty(), "unexpected buf: {buf:?}");
    let res = block_on(&mut ring, next(&mut stream_recv));
    assert!(res.is_none(), "unexpected result: {res:?}");
}

#[test]
fn multishot_recv_large_send() {
    const BUF_SIZE: usize = 512;
    const BUFS: usize = 2;
    const N: usize = 4;
    const DATA: &[u8] = &[123; N * 4];
    init();

    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();
    let buf_pool = ReadBufPool::new(sq.clone(), BUFS as u16, BUF_SIZE as u32).unwrap();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = block_on(&mut ring, tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let mut connect_future = stream.connect(addr);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());
    let (mut client, _) = listener.accept().expect("failed to accept connection");
    block_on(&mut ring, connect_future).expect("failed to connect");

    let mut stream_recv = stream.multishot_recv(buf_pool, 0);

    // Write some data and read it back.
    client.write_all(DATA).expect("failed to write");
    client.shutdown(Shutdown::Write).unwrap();
    let mut data_left = DATA;
    while !data_left.is_empty() {
        let buf = block_on(&mut ring, next(&mut stream_recv))
            .unwrap()
            .expect("failed to receive");
        assert_eq!(&*buf, &data_left[..buf.len()]);
        data_left = &data_left[buf.len()..];
    }

    let buf = block_on(&mut ring, next(&mut stream_recv))
        .unwrap()
        .expect("failed to receive");
    assert!(buf.is_empty(), "unexpected buf: {buf:?}");
    let res = block_on(&mut ring, next(&mut stream_recv));
    assert!(res.is_none(), "unexpected result: {res:?}");
}

#[test]
fn multishot_recv_all_buffers_used() {
    const BUF_SIZE: usize = 512;
    const BUFS: usize = 2;
    const N: usize = 2 + 10;
    const DATA: &[u8] = &[255; BUF_SIZE];
    init();

    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();
    let buf_pool = ReadBufPool::new(sq.clone(), BUFS as u16, BUF_SIZE as u32).unwrap();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = block_on(&mut ring, tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let mut connect_future = stream.connect(addr);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());
    let (mut client, _) = listener.accept().expect("failed to accept connection");
    block_on(&mut ring, connect_future).expect("failed to connect");

    let mut stream_recv = stream.multishot_recv(buf_pool, 0);

    // Write some much data that all buffers are used.
    for _ in 0..N {
        client.write_all(DATA).expect("failed to write");
    }
    client.shutdown(Shutdown::Write).unwrap();

    for i in 0..N {
        let result = block_on(&mut ring, next(&mut stream_recv)).unwrap();
        match result {
            Ok(buf) => assert_eq!(&*buf, DATA),
            Err(err) => {
                // Should have at least read `BUFS` times, after that we should
                // get a `ENOBUFS` error.
                // However depending on the timing and internal ordering (with
                // regards to other operation/processes/etc.) it's possible we
                // get more than `BUFS` buffers as they are put back into the
                // pool.
                assert!(i >= BUFS);
                assert_eq!(err.raw_os_error(), Some(libc::ENOBUFS));
                break;
            }
        }
    }
}

#[test]
fn recv_n() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<RecvN<Vec<u8>>>();
    is_sync::<RecvN<Vec<u8>>>();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let mut connect_future = stream.connect(addr);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());
    let (mut client, _) = listener.accept().expect("failed to accept connection");
    waker.block_on(connect_future).expect("failed to connect");

    // Receive some data.
    client.write_all(DATA1).expect("failed to send data");
    let buf = BadReadBuf {
        data: Vec::with_capacity(30),
    };
    let mut buf = waker.block_on(stream.recv_n(buf, DATA1.len())).unwrap();
    assert_eq!(&buf.data, DATA1);

    // We should detect the peer closing the stream.
    drop(client);
    buf.data.clear();
    let res = waker.block_on(stream.recv_n(buf, 5));
    expect_io_error_kind(res, io::ErrorKind::UnexpectedEof);
}

#[test]
fn recv_vectored() {
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
    let mut connect_future = stream.connect(addr);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    waker.block_on(connect_future).expect("failed to connect");

    // Receive some data.
    let bufs = [
        Vec::with_capacity(5),
        Vec::with_capacity(2),
        Vec::with_capacity(7),
    ];
    let recv_future = stream.recv_vectored(bufs, 0);
    client.write_all(DATA1).expect("failed to send data");
    let (mut bufs, flags) = waker.block_on(recv_future).expect("failed to receive");
    assert_eq!(&bufs[0], b"Hello");
    assert_eq!(&bufs[1], b", ");
    assert_eq!(&bufs[2], b"World!");
    assert_eq!(flags, 0);

    // We should detect the peer closing the stream.
    drop(client);
    for buf in bufs.iter_mut() {
        buf.clear();
    }
    let (bufs, flags) = waker
        .block_on(stream.recv_vectored(bufs, 0))
        .expect("failed to receive");
    assert!(bufs[0].is_empty());
    assert!(bufs[1].is_empty());
    assert!(bufs[2].is_empty());
    assert_eq!(flags, 0);
}

#[test]
fn recv_vectored_truncated() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind socket");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    let socket = waker.block_on(udp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    waker
        .block_on(socket.connect(addr))
        .expect("failed to connect");

    let socket_addr = sock_addr(socket.as_fd()).expect("failed to get local address");
    listener
        .send_to(DATA1, socket_addr)
        .expect("failed to send data");

    // Receive some data.
    let bufs = [Vec::with_capacity(5), Vec::with_capacity(2)];
    let (bufs, flags) = waker
        .block_on(socket.recv_vectored(bufs, 0))
        .expect("failed to receive");
    assert_eq!(&bufs[0], b"Hello");
    assert_eq!(&bufs[1], b", ");
    assert_eq!(flags, libc::MSG_TRUNC);
}

#[test]
fn recv_n_vectored() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<RecvNVectored<Vec<u8>, 1>>();
    is_sync::<RecvNVectored<Vec<u8>, 1>>();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let mut connect_future = stream.connect(addr);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());
    let (mut client, _) = listener.accept().expect("failed to accept connection");
    waker.block_on(connect_future).expect("failed to connect");

    // Receive some data.
    const DATA: &[u8] = b"Hello marsBooo!! Hi. How are you?";
    client.write_all(DATA).expect("failed to send data");
    let bufs = BadReadBufSlice {
        data: [Vec::with_capacity(15), Vec::with_capacity(20)],
    };
    let mut bufs = waker
        .block_on(stream.recv_n_vectored(bufs, DATA.len()))
        .unwrap();
    assert_eq!(&bufs.data[0], b"Hello mars! Hi.");
    assert_eq!(&bufs.data[1], b"Booo! How are you?");

    // We should detect the peer closing the stream.
    drop(client);
    for buf in bufs.data.iter_mut() {
        buf.clear();
    }
    let res = waker.block_on(stream.recv_n_vectored(bufs, 5));
    expect_io_error_kind(res, io::ErrorKind::UnexpectedEof);
}

#[test]
fn recvfrom() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind socket");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    let socket = waker.block_on(udp_ipv4_socket(sq));
    bind_ipv4(&socket);
    let socket_addr = sock_addr(socket.as_fd()).expect("failed to get local address");

    listener
        .send_to(DATA1, socket_addr)
        .expect("failed to send data");

    // Receive some data.
    let (buf, address, flags) = waker
        .block_on(socket.recvfrom(Vec::with_capacity(DATA1.len() + 1), 0))
        .expect("failed to receive");
    assert_eq!(buf, DATA1);
    let address = from_storage(address);
    assert_eq!(address, local_addr);
    assert_eq!(flags, 0);
}

#[test]
fn recvfrom_read_buf_pool() {
    const BUF_SIZE: usize = 4096;

    init();
    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();
    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind socket");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    let socket = block_on(&mut ring, udp_ipv4_socket(sq));
    bind_ipv4(&socket);
    let socket_addr = sock_addr(socket.as_fd()).expect("failed to get local address");

    listener
        .send_to(DATA1, socket_addr)
        .expect("failed to send data");

    // Receive some data.
    let (buf, address, flags) =
        block_on(&mut ring, socket.recvfrom(buf_pool.get(), 0)).expect("failed to receive");
    assert_eq!(&*buf, DATA1);
    let address = from_storage(address);
    assert_eq!(address, local_addr);
    assert_eq!(flags, 0);
}

#[test]
fn recvfrom_vectored() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind socket");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    let socket = waker.block_on(udp_ipv4_socket(sq));
    bind_ipv4(&socket);
    let socket_addr = sock_addr(socket.as_fd()).expect("failed to get local address");

    listener
        .send_to(DATA1, socket_addr)
        .expect("failed to send data");

    // Receive some data.
    let bufs = [
        Vec::with_capacity(5),
        Vec::with_capacity(2),
        Vec::with_capacity(7),
    ];
    let (bufs, address, flags) = waker
        .block_on(socket.recvfrom_vectored(bufs, 0))
        .expect("failed to receive");
    assert_eq!(&bufs[0], b"Hello");
    assert_eq!(&bufs[1], b", ");
    assert_eq!(&bufs[2], b"World!");
    let address = from_storage(address);
    assert_eq!(address, local_addr);
    assert_eq!(flags, 0);
}

#[test]
fn send() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<Send<Vec<u8>>>();
    is_sync::<Send<Vec<u8>>>();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let mut connect_future = stream.connect(addr);
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
    let mut connect_future = stream.connect(addr);
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
    let mut connect_future = stream.connect(addr);
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
    let mut connect_future = stream.connect(addr);
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
fn send_all() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<SendAll<Vec<u8>>>();
    is_sync::<SendAll<Vec<u8>>>();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let mut connect_future = stream.connect(addr);
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());
    let (mut client, _) = listener.accept().expect("failed to accept connection");
    waker.block_on(connect_future).expect("failed to connect");

    // Send all data.
    let buf = BadBuf {
        calls: Cell::new(0),
    };
    waker
        .block_on(stream.send_all(buf))
        .expect("failed to send");
    let mut buf = vec![0; BadBuf::DATA.len() + 1];
    let n = client.read(&mut buf).unwrap();
    assert_eq!(n, BadBuf::DATA.len());
    buf.resize(n, 0);
    assert_eq!(buf, BadBuf::DATA);
}

#[test]
fn send_all_extract() {
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
    let mut connect_future = stream.connect(addr);
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());
    let (mut client, _) = listener.accept().expect("failed to accept connection");
    waker.block_on(connect_future).expect("failed to connect");

    // Send all data.
    let buf = BadBuf {
        calls: Cell::new(0),
    };
    let buf = waker
        .block_on(stream.send_all(buf).extract())
        .expect("failed to send");
    assert_eq!(buf.calls.get(), 6);
    let mut buf = vec![0; BadBuf::DATA.len() + 1];
    let n = client.read(&mut buf).unwrap();
    assert_eq!(n, BadBuf::DATA.len());
    buf.resize(n, 0);
    assert_eq!(buf, BadBuf::DATA);
}

#[test]
fn send_vectored() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<Send<Vec<u8>>>();
    is_sync::<Send<Vec<u8>>>();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let mut connect_future = stream.connect(addr);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    waker.block_on(connect_future).expect("failed to connect");

    // Send some data.
    let bufs = ["Hello", ", ", "World!"];
    let n = waker
        .block_on(stream.send_vectored(bufs, 0))
        .expect("failed to send");
    assert_eq!(n, DATA1.len());
    let mut buf = vec![0; DATA1.len() + 2];
    let n = client.read(&mut buf).expect("failed to send data");
    assert_eq!(&buf[0..n], DATA1);
}

#[test]
fn send_vectored_zc() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<Send<Vec<u8>>>();
    is_sync::<Send<Vec<u8>>>();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let mut connect_future = stream.connect(addr);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    waker.block_on(connect_future).expect("failed to connect");

    // Send some data.
    let bufs = ["Hello", ", ", "World!"];
    let n = waker
        .block_on(stream.send_vectored_zc(bufs, 0))
        .expect("failed to send");
    assert_eq!(n, DATA1.len());
    let mut buf = vec![0; DATA1.len() + 2];
    let n = client.read(&mut buf).expect("failed to send data");
    assert_eq!(&buf[0..n], DATA1);
}

#[test]
fn send_vectored_extractor() {
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
    let mut connect_future = stream.connect(addr);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    waker.block_on(connect_future).expect("failed to connect");

    // Send some data.
    let bufs = ["Hello", ", ", "Mars!"];
    let (bufs, n) = waker
        .block_on(stream.send_vectored(bufs, 0).extract())
        .expect("failed to send");
    assert_eq!(bufs[0], "Hello");
    assert_eq!(n, DATA2.len());
    let mut buf = vec![0; DATA2.len() + 2];
    let n = client.read(&mut buf).expect("failed to send data");
    assert_eq!(&buf[0..n], DATA2);
}

#[test]
fn send_vectored_zc_extractor() {
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
    let mut connect_future = stream.connect(addr);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    waker.block_on(connect_future).expect("failed to connect");

    // Send some data.
    let bufs = ["Hello", ", ", "Mars!"];
    let (bufs, n) = waker
        .block_on(stream.send_vectored_zc(bufs, 0).extract())
        .expect("failed to send");
    assert_eq!(bufs[0], "Hello");
    assert_eq!(n, DATA2.len());
    let mut buf = vec![0; DATA2.len() + 2];
    let n = client.read(&mut buf).expect("failed to send data");
    assert_eq!(&buf[0..n], DATA2);
}

#[test]
fn send_all_vectored() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<SendAllVectored<Vec<u8>, 1>>();
    is_sync::<SendAllVectored<Vec<u8>, 1>>();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let mut connect_future = stream.connect(addr);
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());
    let (mut client, _) = listener.accept().expect("failed to accept connection");
    waker.block_on(connect_future).expect("failed to connect");

    // Send all data.
    let bufs = BadBufSlice {
        calls: Cell::new(0),
    };
    waker.block_on(stream.send_all_vectored(bufs)).unwrap();

    let mut buf = vec![0; 31];
    let n = client.read(&mut buf).unwrap();
    assert_eq!(n, 30);
    buf.resize(n, 0);
    assert_eq!(buf[..10], BadBufSlice::DATA1);
    assert_eq!(buf[10..20], BadBufSlice::DATA2);
    assert_eq!(buf[20..], BadBufSlice::DATA3);
}

#[test]
fn send_all_vectored_extract() {
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
    let mut connect_future = stream.connect(addr);
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());
    let (mut client, _) = listener.accept().expect("failed to accept connection");
    waker.block_on(connect_future).expect("failed to connect");

    // Send all data.
    let bufs = BadBufSlice {
        calls: Cell::new(0),
    };
    let bufs = waker
        .block_on(stream.send_all_vectored(bufs).extract())
        .unwrap();
    assert_eq!(bufs.calls.get(), 3);

    let mut buf = vec![0; 31];
    let n = client.read(&mut buf).unwrap();
    assert_eq!(n, 30);
    buf.resize(n, 0);
    assert_eq!(buf[..10], BadBufSlice::DATA1);
    assert_eq!(buf[10..20], BadBufSlice::DATA2);
    assert_eq!(buf[20..], BadBufSlice::DATA3);
}

#[test]
fn sendto() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<SendTo<Vec<u8>, ()>>();
    is_sync::<SendTo<Vec<u8>, ()>>();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    let socket = waker.block_on(udp_ipv4_socket(sq));

    // Send some data.
    let addr = addr_storage(&local_addr);
    let n = waker
        .block_on(socket.sendto(DATA1, addr, 0))
        .expect("failed to sendto");
    assert_eq!(n, DATA1.len());

    let mut buf = vec![0; DATA1.len() + 2];
    let (n, from_address) = listener.recv_from(&mut buf).expect("failed to recv data");
    assert_eq!(&buf[0..n], DATA1);
    assert!(from_address.ip().is_loopback());
}

#[test]
fn sendto_zc() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    let socket = waker.block_on(udp_ipv4_socket(sq));

    // Send some data.
    let addr = addr_storage(&local_addr);
    let n = waker
        .block_on(socket.sendto_zc(DATA1, addr, 0))
        .expect("failed to sendto");
    assert_eq!(n, DATA1.len());

    let mut buf = vec![0; DATA1.len() + 2];
    let (n, from_address) = listener.recv_from(&mut buf).expect("failed to recv data");
    assert_eq!(&buf[0..n], DATA1);
    assert!(from_address.ip().is_loopback());
}

#[test]
fn sendto_extractor() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    let socket = waker.block_on(udp_ipv4_socket(sq));

    // Send some data.
    let addr = addr_storage(&local_addr);
    let (buf, n) = waker
        .block_on(socket.sendto(DATA1, addr.clone(), 0).extract())
        .expect("failed to sendto");
    assert!(buf == DATA1);
    assert_eq!(n, DATA1.len());

    let mut buf = vec![0; DATA1.len() + 2];
    let (n, from_address) = listener.recv_from(&mut buf).expect("failed to recv data");
    assert_eq!(&buf[0..n], DATA1);
    assert!(from_address.ip().is_loopback());
}

#[test]
fn sendto_zc_extractor() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    let socket = waker.block_on(udp_ipv4_socket(sq));

    // Send some data.
    let addr = addr_storage(&local_addr);
    let (buf, n) = waker
        .block_on(socket.sendto_zc(DATA1, addr.clone(), 0).extract())
        .expect("failed to sendto");
    assert!(buf == DATA1);
    assert_eq!(n, DATA1.len());

    let mut buf = vec![0; DATA1.len() + 2];
    let (n, from_address) = listener.recv_from(&mut buf).expect("failed to recv data");
    assert_eq!(&buf[0..n], DATA1);
    assert!(from_address.ip().is_loopback());
}

#[test]
fn sendto_vectored() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<SendTo<Vec<u8>, ()>>();
    is_sync::<SendTo<Vec<u8>, ()>>();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    let socket = waker.block_on(udp_ipv4_socket(sq));

    // Send some data.
    let bufs = ["Hello", ", ", "World!"];
    let addr = addr_storage(&local_addr);
    let n = waker
        .block_on(socket.sendto_vectored(bufs, addr, 0))
        .expect("failed to sendto");
    assert_eq!(n, DATA1.len());

    let mut buf = vec![0; DATA1.len() + 2];
    let (n, from_address) = listener.recv_from(&mut buf).expect("failed to recv data");
    assert_eq!(&buf[0..n], DATA1);
    assert!(from_address.ip().is_loopback());
}

#[test]
fn sendto_vectored_zc() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    let socket = waker.block_on(udp_ipv4_socket(sq));

    // Send some data.
    let bufs = ["Hello", ", ", "World!"];
    let addr = addr_storage(&local_addr);
    let n = waker
        .block_on(socket.sendto_vectored_zc(bufs, addr, 0))
        .expect("failed to sendto");
    assert_eq!(n, DATA1.len());

    let mut buf = vec![0; DATA1.len() + 2];
    let (n, from_address) = listener.recv_from(&mut buf).expect("failed to recv data");
    assert_eq!(&buf[0..n], DATA1);
    assert!(from_address.ip().is_loopback());
}

#[test]
fn sendto_vectored_extractor() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    let socket = waker.block_on(udp_ipv4_socket(sq));

    // Send some data.
    let bufs = ["Hello", ", ", "Mars!"];
    let addr = addr_storage(&local_addr);
    let (buf, n) = waker
        .block_on(socket.sendto_vectored(bufs, addr.clone(), 0).extract())
        .expect("failed to sendto");
    assert!(buf[2] == "Mars!");
    assert_eq!(n, DATA2.len());

    let mut buf = vec![0; DATA2.len() + 2];
    let (n, from_address) = listener.recv_from(&mut buf).expect("failed to recv data");
    assert_eq!(&buf[0..n], DATA2);
    assert!(from_address.ip().is_loopback());
}

#[test]
fn sendto_vectored_zc_extractor() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    let socket = waker.block_on(udp_ipv4_socket(sq));

    // Send some data.
    let bufs = ["Hello", ", ", "Mars!"];
    let addr = addr_storage(&local_addr);
    let (bufs, n) = waker
        .block_on(socket.sendto_vectored_zc(bufs, addr.clone(), 0).extract())
        .expect("failed to sendto");
    assert!(bufs[0] == "Hello");
    assert_eq!(n, DATA2.len());

    let mut buf = vec![0; DATA2.len() + 2];
    let (n, from_address) = listener.recv_from(&mut buf).expect("failed to recv data");
    assert_eq!(&buf[0..n], DATA2);
    assert!(from_address.ip().is_loopback());
}

#[test]
fn shutdown() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<Shutdown>();
    is_sync::<Shutdown>();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let mut connect_future = stream.connect(addr);
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

fn addr_storage(address: &SocketAddrV4) -> libc::sockaddr_in {
    // SAFETY: a `sockaddr_in` of all zeros is valid.
    let mut storage: libc::sockaddr_in = unsafe { mem::zeroed() };
    storage.sin_family = libc::AF_INET as libc::sa_family_t;
    storage.sin_port = address.port().to_be();
    storage.sin_addr = libc::in_addr {
        s_addr: u32::from_ne_bytes(address.ip().octets()),
    };
    storage
}

fn from_storage(addr: libc::sockaddr_in) -> SocketAddrV4 {
    assert!(addr.sin_family as libc::c_int == libc::AF_INET);
    let ip = Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes());
    let port = u16::from_be(addr.sin_port);
    SocketAddrV4::new(ip, port)
}

fn peer_addr(fd: BorrowedFd) -> io::Result<SocketAddr> {
    let mut storage: libc::sockaddr_storage = unsafe { mem::zeroed() };
    let mut len = size_of::<libc::sockaddr_storage>() as u32;
    syscall!(getpeername(
        fd.as_raw_fd(),
        ptr::addr_of_mut!(storage).cast::<libc::sockaddr>(),
        &mut len
    ))?;
    Ok(convert_address(storage, len))
}

fn sock_addr(fd: BorrowedFd) -> io::Result<SocketAddr> {
    let mut storage: libc::sockaddr_storage = unsafe { mem::zeroed() };
    let mut len = size_of::<libc::sockaddr_storage>() as u32;
    syscall!(getsockname(
        fd.as_raw_fd(),
        ptr::addr_of_mut!(storage).cast::<libc::sockaddr>(),
        &mut len
    ))?;
    Ok(convert_address(storage, len))
}

fn convert_address(storage: libc::sockaddr_storage, len: libc::socklen_t) -> SocketAddr {
    if storage.ss_family == libc::AF_INET as libc::sa_family_t {
        assert!(len == size_of::<libc::sockaddr_in>() as libc::socklen_t);
        let storage = unsafe { &*ptr::addr_of!(storage).cast::<libc::sockaddr_in>() };
        let addr = Ipv4Addr::from(storage.sin_addr.s_addr.to_ne_bytes());
        let port = storage.sin_port.to_be();
        SocketAddr::V4(SocketAddrV4::new(addr, port))
    } else if storage.ss_family == libc::AF_INET6 as libc::sa_family_t {
        assert!(len == size_of::<libc::sockaddr_in6>() as libc::socklen_t);
        let storage = unsafe { &*ptr::addr_of!(storage).cast::<libc::sockaddr_in6>() };
        let addr = Ipv6Addr::from(storage.sin6_addr.s6_addr);
        let port = storage.sin6_port.to_be();
        let flowinfo = storage.sin6_flowinfo;
        let scope_id = storage.sin6_scope_id;
        SocketAddr::V6(SocketAddrV6::new(addr, port, flowinfo, scope_id))
    } else {
        panic!("invalid socket storage type: {}", storage.ss_family)
    }
}

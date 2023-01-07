//! Tests for the I/O operations.

#![feature(once_cell)]

use std::env::temp_dir;
use std::io;
use std::pin::Pin;

use a10::fixed::ReadBufPool;
use a10::fs::OpenOptions;
use a10::io::{stderr, stdout};
use a10::Ring;

mod util;
use util::{
    bind_ipv4, block_on, expect_io_errno, expect_io_error_kind, init, poll_nop, tcp_ipv4_socket,
    test_queue, Waker, LOREM_IPSUM_50,
};

const BUF_SIZE: usize = 4096;

#[test]
fn read_read_buf_pool() {
    init();
    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();

    let test_file = &LOREM_IPSUM_50;
    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    let path = test_file.path.into();
    let file = block_on(&mut ring, OpenOptions::new().open(sq, path)).unwrap();

    let buf = block_on(&mut ring, file.read_at(buf_pool.get(), 0)).unwrap();

    assert_eq!(buf.len(), BUF_SIZE);
    assert!(
        &*buf == &test_file.content[..buf.len()],
        "read content is different"
    );
}

#[test]
fn read_read_buf_pool_multiple_buffers() {
    init();
    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();

    let test_file = &LOREM_IPSUM_50;
    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    let path = test_file.path.into();
    let file = block_on(&mut ring, OpenOptions::new().open(sq, path)).unwrap();

    let mut read1 = file.read_at(buf_pool.get(), 0);
    let _ = poll_nop(Pin::new(&mut read1));
    let mut read2 = file.read_at(buf_pool.get(), 0);
    let _ = poll_nop(Pin::new(&mut read2));
    let mut read3 = file.read_at(buf_pool.get(), 0);
    let _ = poll_nop(Pin::new(&mut read3));
    let buf1 = block_on(&mut ring, read1).unwrap();
    let buf2 = block_on(&mut ring, read2).unwrap();

    for buf in [buf1, buf2] {
        assert_eq!(buf.len(), BUF_SIZE);
        assert!(
            &*buf == &test_file.content[..buf.len()],
            "read content is different"
        );
    }
}

#[test]
fn read_read_buf_pool_reuse_buffers() {
    init();
    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();

    let test_file = &LOREM_IPSUM_50;
    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    let path = test_file.path.into();
    let file = block_on(&mut ring, OpenOptions::new().open(sq, path)).unwrap();

    for _ in 0..4 {
        let buf = block_on(&mut ring, file.read_at(buf_pool.get(), 0)).unwrap();
        assert_eq!(buf.len(), BUF_SIZE);
        assert!(
            &*buf == &test_file.content[..buf.len()],
            "read content is different"
        );
    }
}

#[test]
fn read_read_buf_pool_out_of_buffers() {
    init();
    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();

    let test_file = &LOREM_IPSUM_50;
    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    let path = test_file.path.into();
    let file = block_on(&mut ring, OpenOptions::new().open(sq, path)).unwrap();

    let futures = (0..8)
        .map(|_| {
            let mut read = file.read_at(buf_pool.get(), 0);
            let _ = poll_nop(Pin::new(&mut read));
            read
        })
        .collect::<Vec<_>>();

    for future in futures {
        let buf = match block_on(&mut ring, future) {
            Ok(buf) => buf,
            Err(err) => {
                if let Some(libc::ENOBUFS) = err.raw_os_error() {
                    continue;
                }
                panic!("unexpected {err}");
            }
        };
        assert_eq!(buf.len(), BUF_SIZE);
        assert!(
            &*buf == &test_file.content[..buf.len()],
            "read content is different"
        );
    }
}

#[test]
fn two_read_buf_pools() {
    init();
    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();
    let test_file = &LOREM_IPSUM_50;

    let buf_pool1 = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();
    let buf_pool2 = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    let path = test_file.path.into();
    let file = block_on(&mut ring, OpenOptions::new().open(sq, path)).unwrap();

    for buf_pool in [buf_pool1, buf_pool2] {
        let buf = block_on(&mut ring, file.read_at(buf_pool.get(), 0)).unwrap();
        assert_eq!(buf.len(), BUF_SIZE);
        assert!(
            &*buf == &test_file.content[..buf.len()],
            "read content is different"
        );
    }
}

#[test]
fn cancel_previous_accept() {
    let sq = test_queue();
    let waker = Waker::new();

    let listener = waker.block_on(tcp_ipv4_socket(sq));
    bind_ipv4(&listener);

    let mut accept = listener.accept();
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut accept)).is_pending());

    waker
        .block_on(listener.cancel_previous())
        .expect("failed to cancel accept call");

    expect_io_errno(waker.block_on(accept), libc::ECANCELED);
}

#[test]
fn cancel_all_accept() {
    let sq = test_queue();
    let waker = Waker::new();

    let listener = waker.block_on(tcp_ipv4_socket(sq));
    bind_ipv4(&listener);

    let mut accept = listener.accept();
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut accept)).is_pending());

    waker
        .block_on(listener.cancel_all())
        .expect("failed to cancel all calls");

    expect_io_errno(waker.block_on(accept), libc::ECANCELED);
}

#[test]
fn cancel_previous_accept_twice() {
    let sq = test_queue();
    let waker = Waker::new();

    let listener = waker.block_on(tcp_ipv4_socket(sq));
    bind_ipv4(&listener);

    let mut accept = listener.accept();
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut accept)).is_pending());

    waker
        .block_on(listener.cancel_previous())
        .expect("failed to cancel accept call");
    // And again.
    expect_io_error_kind(
        waker.block_on(listener.cancel_previous()),
        io::ErrorKind::NotFound,
    );

    expect_io_errno(waker.block_on(accept), libc::ECANCELED);
}

#[test]
fn cancel_all_accept_twice() {
    let sq = test_queue();
    let waker = Waker::new();

    let listener = waker.block_on(tcp_ipv4_socket(sq));
    bind_ipv4(&listener);

    let mut accept = listener.accept();
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut accept)).is_pending());

    waker
        .block_on(listener.cancel_all())
        .expect("failed to cancel all calls");
    // And again. Unlike `cancel_previous`, this should return ok.
    waker
        .block_on(listener.cancel_all())
        .expect("failed to cancel all calls");

    expect_io_errno(waker.block_on(accept), libc::ECANCELED);
}

#[test]
fn cancel_previous_no_operation_in_progress() {
    let sq = test_queue();
    let waker = Waker::new();

    let socket = waker.block_on(tcp_ipv4_socket(sq));

    let cancel = socket.cancel_previous();
    expect_io_error_kind(waker.block_on(cancel), io::ErrorKind::NotFound);
}

#[test]
fn cancel_all_no_operation_in_progress() {
    let sq = test_queue();
    let waker = Waker::new();

    let socket = waker.block_on(tcp_ipv4_socket(sq));

    let cancel = socket.cancel_all();
    waker.block_on(cancel).expect("failed to cancel");
}

#[test]
fn close_socket_fd() {
    let sq = test_queue();
    let waker = Waker::new();

    let socket = waker.block_on(tcp_ipv4_socket(sq));
    waker.block_on(socket.close()).expect("failed to close fd");
}

#[test]
fn close_fs_fd() {
    let sq = test_queue();
    let waker = Waker::new();

    let open_file = OpenOptions::new().open(sq, "tests/data/lorem_ipsum_5.txt".into());
    let file = waker.block_on(open_file).unwrap();
    waker.block_on(file.close()).expect("failed to close fd");
}

#[test]
fn dropped_futures_do_not_leak_buffers() {
    // NOTE: run this test with the `leak` or `address` sanitizer, see the
    // test_sanitizer Make target, and it shouldn't cause any errors.

    let sq = test_queue();
    let waker = Waker::new();

    let open_file = OpenOptions::new().write().open_temp_file(sq, temp_dir());
    let file = waker.block_on(open_file).unwrap();

    let buf = vec![123; 64 * 1024];
    let write = file.write(buf);
    drop(write);
}

#[test]
fn stdout_write() {
    let sq = test_queue();
    let waker = Waker::new();

    let stdout = stdout(sq);
    waker.block_on(stdout.write("Hello, stdout!\n")).unwrap();
}

#[test]
fn stderr_write() {
    let sq = test_queue();
    let waker = Waker::new();

    let stderr = stderr(sq);
    waker.block_on(stderr.write("Hello, stderr!\n")).unwrap();
}

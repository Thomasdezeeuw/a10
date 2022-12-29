//! Tests for the I/O operations.

#![feature(once_cell)]

use std::env::temp_dir;
use std::io;
use std::pin::Pin;

use a10::fs::OpenOptions;

mod util;
use util::{
    bind_ipv4, expect_io_errno, expect_io_error_kind, poll_nop, tcp_ipv4_socket, test_queue, Waker,
};

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

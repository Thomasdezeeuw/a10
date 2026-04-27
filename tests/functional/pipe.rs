use std::io;
use std::pin::pin;

use a10::fd;
use a10::pipe::{Pipe, pipe};

use crate::util::{Waker, expect_io_error_kind, is_send, is_sync, poll_nop, test_queue};

const DATA1: &[u8] = b"Hello from the other side";

#[test]
fn pipe_is_send_and_sync() {
    is_send::<Pipe>();
    is_sync::<Pipe>();
}

#[test]
fn pipe_file_descriptor() {
    test_pipe(fd::Kind::File)
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn pipe_direct_descriptor() {
    test_pipe(fd::Kind::Direct)
}

fn test_pipe(fd_kind: fd::Kind) {
    let sq = test_queue();
    let waker = Waker::new();

    let [receiver, sender] = waker
        .block_on(pipe(sq).kind(fd_kind))
        .expect("failed to create pipe");

    // Send some data.
    waker
        .block_on(sender.write_all(DATA1))
        .expect("failed to write");

    // Received it on the other side.
    let received = waker
        .block_on(receiver.read_n(Vec::with_capacity(DATA1.len() + 1), DATA1.len()))
        .expect("failed to read");
    assert_eq!(received, DATA1);
}

#[test]
fn cancel_pipe() {
    let sq = test_queue();

    let mut pipe = pin!(pipe(sq));
    _ = poll_nop(pipe.as_mut());
    drop(pipe);
}

#[test]
fn writing_to_closed_pipe() {
    // Older versions of macOS (OS X 10.11 and 10.10 have been witnessed) can
    // return EPIPE when registering a pipe file descriptor where the other end
    // has already disappeared. For example code that creates a pipe, closes a
    // file descriptor, and then registers the other end will see an EPIPE
    // returned.
    //
    // More info can be found at https://github.com/tokio-rs/mio#582.
    //
    // We've removed the work around for this (as OS X 10.11 and 10.10 are no
    // longer supported), but we explicitly want to test the behaviour doesn't
    // return.

    let sq = test_queue();
    let waker = Waker::new();

    let [receiver, sender] = waker.block_on(pipe(sq)).expect("failed to create pipe");

    drop(receiver); // Close the fd.

    let res = waker.block_on(sender.write_all(DATA1));
    expect_io_error_kind(res, io::ErrorKind::BrokenPipe);
}

#[test]
fn reading_from_closed_pipe() {
    // See writing_to_closed_pipe test above for why this is tested.

    let sq = test_queue();
    let waker = Waker::new();

    let [receiver, sender] = waker.block_on(pipe(sq)).expect("failed to create pipe");

    drop(sender); // Close the fd.

    let buf = waker
        .block_on(receiver.read(Vec::with_capacity(8)))
        .expect("failed to read");
    assert!(buf.is_empty());
}

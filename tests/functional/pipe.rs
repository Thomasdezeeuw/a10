use a10::fd;
use a10::pipe::{Pipe, pipe};

use crate::util::{Waker, cancel, is_send, is_sync, start_op, test_queue};

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
fn pipe_direct_descriptor() {
    test_pipe(fd::Kind::Direct)
}

fn test_pipe(fd_kind: fd::Kind) {
    let sq = test_queue();
    let waker = Waker::new();

    let [receiver, sender] = waker
        .block_on(pipe(sq, None).kind(fd_kind))
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
    let waker = Waker::new();

    let mut pipe = pipe(sq, None);
    cancel(&waker, &mut pipe, start_op);
}

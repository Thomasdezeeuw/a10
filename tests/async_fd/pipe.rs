//! Tests for the Unix pipe.

use std::sync::Arc;

use a10::fd::{self, AsyncFd};
use a10::pipe::{pipe, Pipe};

use crate::util::{cancel, is_send, is_sync, require_kernel, start_op, test_queue, Waker};

const DATA1: &[u8] = b"Hello from the other side";

#[test]
fn pipe_file_descriptor() {
    require_kernel!(6, 16);

    let sq = test_queue();
    let waker = Waker::new();

    is_send::<Pipe>();
    is_sync::<Pipe>();

    let [receiver, sender] = waker.block_on(pipe(sq, 0)).expect("failed to create pipe");
    test_pipe(waker, receiver, sender);
}

#[test]
fn pipe_direct_descriptor() {
    require_kernel!(6, 16);

    let sq = test_queue();
    let waker = Waker::new();

    let [receiver, sender] = waker
        .block_on(pipe(sq, 0).kind(fd::Kind::Direct))
        .expect("failed to create pipe");
    test_pipe(waker, receiver, sender);
}

#[test]
fn cancel_pipe() {
    require_kernel!(6, 16);

    let sq = test_queue();
    let waker = Waker::new();

    let mut pipe = pipe(sq, 0);
    cancel(&waker, &mut pipe, start_op);
}

fn test_pipe(waker: Arc<Waker>, receiver: AsyncFd, sender: AsyncFd) {
    dbg!(&sender, &receiver);

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

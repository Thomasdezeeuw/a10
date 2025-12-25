use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, FromRawFd, RawFd};
use std::pin::pin;

use a10::poll::{Event, Interest, MultishotPoll, OneshotPoll, multishot_poll, oneshot_poll};

use crate::util::{
    pipe, Waker, cancel, expect_io_errno, is_send, is_sync, next, start_mulitshot_op, start_op,
    test_queue,
};

const DATA: &[u8] = b"Hello, World!";

#[test]
fn oneshot_poll_is_send_and_sync() {
    is_send::<OneshotPoll>();
    is_sync::<OneshotPoll>();
}

#[test]
fn multishot_poll_is_send_and_sync() {
    is_send::<MultishotPoll>();
    is_sync::<MultishotPoll>();
}

#[test]
fn interest_is_send_and_sync() {
    is_send::<Interest>();
    is_sync::<Interest>();
}

#[test]
fn event_is_send_and_sync() {
    is_send::<Event>();
    is_sync::<Event>();
}

#[test]
fn test_oneshot_poll() {
    let sq = test_queue();
    let waker = Waker::new();

    let (mut receiver, mut sender) = pipe2().unwrap();

    let sender_write = pin!(oneshot_poll(sq.clone(), sender.as_fd(), Interest::WRITABLE));
    let receiver_read = pin!(oneshot_poll(sq, receiver.as_fd(), Interest::READABLE));

    let event = waker.block_on(sender_write).unwrap();
    assert!(event.is_writable());
    sender.write_all(DATA).unwrap();

    let event = waker.block_on(receiver_read).unwrap();
    assert!(event.is_readable());
    let mut buf = vec![0; DATA.len() + 1];
    let n = receiver.read(&mut buf).unwrap();
    assert_eq!(n, DATA.len());
    assert_eq!(&buf[0..n], DATA);
}

#[test]
fn drop_oneshot_poll() {
    let sq = test_queue();

    let (receiver, sender) = pipe2().unwrap();

    let mut receiver_read = oneshot_poll(sq, receiver.as_fd(), Interest::READABLE);

    start_op(&mut receiver_read);

    drop(receiver_read);
    drop(receiver);
    drop(sender);
}

#[test]
fn cancel_oneshot_poll() {
    let sq = test_queue();
    let waker = Waker::new();

    let (receiver, sender) = pipe2().unwrap();

    let mut receiver_read = oneshot_poll(sq, receiver.as_fd(), Interest::READABLE);

    cancel(&waker, &mut receiver_read, start_op);
    expect_io_errno(waker.block_on(receiver_read), libc::ECANCELED);
    drop(sender);
}

#[test]
fn test_multishot_poll() {
    let sq = test_queue();
    let waker = Waker::new();

    let (mut receiver, mut sender) = pipe2().unwrap();

    let mut receiver_read = pin!(multishot_poll(sq, receiver.as_fd(), Interest::READABLE));
    start_mulitshot_op(&mut receiver_read);

    let mut buf = vec![0; DATA.len() + 1];
    for _ in 0..3 {
        // Writing should trigger a readable event for the other side.
        sender.write_all(DATA).unwrap();
        let event = waker
            .block_on(next(receiver_read.as_mut()))
            .unwrap()
            .unwrap();
        assert!(event.is_readable());

        let n = receiver.read(&mut buf).unwrap();
        assert_eq!(n, DATA.len());
        assert_eq!(&buf[0..n], DATA);
    }
}

#[test]
fn cancel_multishot_poll() {
    let sq = test_queue();
    let waker = Waker::new();

    let (receiver, sender) = pipe2().unwrap();

    let mut receiver_read = multishot_poll(sq, receiver.as_fd(), Interest::READABLE);

    cancel(&waker, &mut receiver_read, start_mulitshot_op);
    assert!(waker.block_on(next(receiver_read)).is_none());
    drop(sender);
}

#[test]
fn drop_multishot_poll() {
    let sq = test_queue();

    let (receiver, sender) = pipe2().unwrap();

    let mut receiver_read = multishot_poll(sq, receiver.as_fd(), Interest::READABLE);

    start_mulitshot_op(&mut receiver_read);

    drop(receiver_read);
    drop(receiver);
    drop(sender);
}

fn pipe2() -> io::Result<(File, File)> {
    let [r, w] = pipe()
    // SAFETY: we just initialised the `fds` above.
    let r = unsafe { File::from_raw_fd(r) };
    let w = unsafe { File::from_raw_fd(w) };
    Ok((r, w))
}

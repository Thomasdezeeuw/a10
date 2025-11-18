//! Tests for [`a10::Ring`].

#![cfg_attr(feature = "nightly", feature(async_iterator))]

use std::alloc::{Layout, alloc, dealloc};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, FromRawFd, RawFd};
use std::pin::{Pin, pin};
use std::process::Command;

use a10::mem::{self, AdviseFlag};
use a10::msg;
use a10::poll::{Interest, MultishotPoll, OneshotPoll, multishot_poll, oneshot_poll};
use a10::process::{self, ChildStatus, Signal, WaitOption};

mod util;
use util::{
    Waker, cancel, defer, expect_io_errno, is_send, is_sync, next, poll_nop, require_kernel,
    start_mulitshot_op, start_op, test_queue,
};

const DATA: &[u8] = b"Hello, World!";

#[test]
fn message_sending() {
    require_kernel!(5, 18);

    const DATA1: u32 = 123;
    const DATA2: u32 = u32::MAX;

    let sq = test_queue();
    let waker = Waker::new();

    is_send::<msg::Listener>();
    is_sync::<msg::Listener>();
    is_send::<msg::Sender>();
    is_sync::<msg::Sender>();
    is_send::<msg::SendMsg>();
    is_sync::<msg::SendMsg>();

    let (listener, sender) = msg::listener(sq.clone()).unwrap();
    let mut listener = pin!(listener);
    start_mulitshot_op(&mut listener);

    // Send some messages.
    sender.try_send(DATA1).unwrap();
    waker.block_on(pin!(sender.send(DATA2))).unwrap();

    assert_eq!(waker.block_on(next(listener.as_mut())), Some(DATA1));
    assert_eq!(waker.block_on(next(listener.as_mut())), Some(DATA2));

    assert!(poll_nop(Pin::new(&mut next(listener.as_mut()))).is_pending());
}

#[test]
fn test_oneshot_poll() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<OneshotPoll>();
    is_sync::<OneshotPoll>();

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

    is_send::<MultishotPoll>();
    is_sync::<MultishotPoll>();

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
    let mut fds: [RawFd; 2] = [-1, -1];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: we just initialised the `fds` above.
    let r = unsafe { File::from_raw_fd(fds[0]) };
    let w = unsafe { File::from_raw_fd(fds[1]) };
    Ok((r, w))
}

#[test]
fn madvise() {
    let sq = test_queue();
    let waker = Waker::new();

    // The address in `madvise(2)` needs to be page aligned.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
    let layout =
        Layout::from_size_align(2 * page_size, page_size).expect("failed to create alloc layout");
    let ptr = unsafe { alloc(layout) };
    let _d = defer(|| unsafe { dealloc(ptr, layout) });

    is_send::<mem::Advise>();
    is_sync::<mem::Advise>();

    waker
        .block_on(mem::advise(
            sq,
            ptr.cast(),
            page_size as u32,
            AdviseFlag::WILL_NEED,
        ))
        .expect("failed madvise");
}

#[test]
fn process_wait_on() {
    require_kernel!(6, 7);
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<process::WaitId>();
    is_sync::<process::WaitId>();

    let process = Command::new("true").spawn().unwrap();
    let pid = process.id();

    let info = waker
        .block_on(process::wait_on(sq, &process, Some(WaitOption::EXITED)))
        .expect("failed wait");

    assert_eq!(info.signal(), Signal::CHILD);
    assert_eq!(info.code(), ChildStatus::EXITED);
    assert_eq!(info.pid(), pid as i32);
    assert_eq!(info.status().code(), Some(libc::EXIT_SUCCESS));
}

#[test]
fn process_wait_on_cancel() {
    require_kernel!(6, 7);
    let sq = test_queue();
    let waker = Waker::new();

    let mut process = Command::new("sleep").arg("1000").spawn().unwrap();

    let mut future = process::wait_on(sq, &process, Some(WaitOption::EXITED));

    cancel(&waker, &mut future, |future| {
        // NOTE: can't use `start_op` as `siginfo_t` doesn't implemented
        // `fmt::Debug`.
        let result = poll_nop(Pin::new(future));
        if !result.is_pending() {
            panic!("unexpected result, expected it to return Poll::Pending");
        }
    });

    process.kill().unwrap();
    process.wait().unwrap();
}

#[test]
fn process_wait_on_drop_before_complete() {
    require_kernel!(6, 7);
    let sq = test_queue();

    let process = Command::new("sleep").arg("1000").spawn().unwrap();

    let mut future = process::wait_on(sq, &process, Some(WaitOption::EXITED));
    let result = poll_nop(Pin::new(&mut future));
    if !result.is_pending() {
        panic!("unexpected result, expected it to return Poll::Pending");
    }
    drop(future);
}

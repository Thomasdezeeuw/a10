//! Tests for [`a10::Ring`].

#![cfg_attr(feature = "nightly", feature(async_iterator))]

use std::alloc::{alloc, dealloc, Layout};
use std::fs::File;
use std::future::Future;
use std::io::{self, Read, Write};
use std::mem::take;
use std::os::fd::{AsFd, FromRawFd, RawFd};
use std::pin::{pin, Pin};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::task::{self, Poll, Wake};
use std::thread;
use std::time::{Duration, Instant};

use a10::cancel::Cancel;
use a10::fs::OpenOptions;
use a10::io::ReadBufPool;
use a10::msg::{MsgListener, MsgToken, SendMsg};
use a10::poll::{oneshot_poll, MultishotPoll, OneshotPoll};
use a10::{mem, process, AsyncFd, Config, Ring, SubmissionQueue};

mod util;
use util::{
    defer, expect_io_errno, init, is_send, is_sync, next, poll_nop, require_kernel,
    start_mulitshot_op, start_op, test_queue, Waker,
};

const DATA: &[u8] = b"Hello, World!";
const BUF_SIZE: usize = 4096;

#[test]
fn polling_timeout() -> io::Result<()> {
    const TIMEOUT: Duration = Duration::from_millis(100);
    const MARGIN: Duration = Duration::from_millis(10);

    init();
    let mut ring = Ring::new(1).unwrap();

    is_send::<Ring>();
    is_sync::<Ring>();
    is_send::<Config>();
    is_sync::<Config>();
    is_send::<SubmissionQueue>();
    is_sync::<SubmissionQueue>();
    is_send::<AsyncFd>();
    is_sync::<AsyncFd>();

    let start = Instant::now();
    ring.poll(Some(TIMEOUT)).unwrap();
    let elapsed = start.elapsed();
    assert!(
        elapsed >= TIMEOUT && elapsed <= (TIMEOUT + MARGIN),
        "polling elapsed: {elapsed:?}, expected: {TIMEOUT:?}",
    );
    Ok(())
}

#[test]
fn dropping_unmaps_queues() {
    init();
    let ring = Ring::new(64).unwrap();
    drop(ring);
}

#[test]
fn submission_queue_full_is_handle_internally() {
    const PATH: &str = "tests/data/lorem_ipsum_50.txt";
    const EXPECTED: &[u8] = include_bytes!("data/lorem_ipsum_50.txt");
    const SIZE: usize = 31396;
    const N: usize = (usize::BITS as usize) + 10;
    const BUF_SIZE: usize = SIZE / N;

    init();
    let mut ring = Ring::new(2).unwrap();
    let sq = ring.submission_queue();

    let mut future = OpenOptions::new().open(sq.clone(), PATH.into());
    let file = loop {
        match poll_nop(Pin::new(&mut future)) {
            Poll::Ready(result) => break result.unwrap(),
            Poll::Pending => ring.poll(None).unwrap(),
        }
    };

    let indices = Arc::new(Mutex::new(Vec::new()));

    struct Waker {
        index: usize,
        /// Indices of task that are awoken.
        indices: Arc<Mutex<Vec<usize>>>,
    }

    impl Wake for Waker {
        fn wake(self: Arc<Self>) {
            self.indices.lock().unwrap().push(self.index);
        }
    }

    let mut futures = (0..N)
        .into_iter()
        .map(|i| {
            let fut = file.read_at(Vec::with_capacity(BUF_SIZE), (i * BUF_SIZE) as u64);
            let waker = Arc::new(Waker {
                index: i,
                indices: indices.clone(),
            });
            Some((fut, task::Waker::from(waker)))
        })
        .collect::<Vec<_>>();

    // Run all futures once to register the operation or them waiting for a
    // submission slot.
    for (i, fut) in futures.iter_mut().enumerate() {
        if let Some((future, waker)) = fut {
            let mut ctx = task::Context::from_waker(&waker);
            match Pin::new(future).poll(&mut ctx) {
                Poll::Ready(result) => {
                    *fut = None;
                    let read_buf = result.unwrap();
                    assert_eq!(read_buf, &EXPECTED[i * BUF_SIZE..(i + 1) * BUF_SIZE]);
                }
                Poll::Pending => {}
            }
        }
    }

    loop {
        // Poll all futures that got a wake up.
        for i in take(&mut *indices.lock().unwrap()).into_iter() {
            if let Some((future, waker)) = &mut futures[i] {
                let mut ctx = task::Context::from_waker(&waker);
                match Pin::new(future).poll(&mut ctx) {
                    Poll::Ready(result) => {
                        futures[i] = None;
                        let read_buf = result.unwrap();
                        assert_eq!(read_buf, &EXPECTED[i * BUF_SIZE..(i + 1) * BUF_SIZE]);
                    }
                    Poll::Pending => {}
                }
            }
        }

        if futures.iter().all(Option::is_some) {
            break;
        }

        ring.poll(None).unwrap();
    }
}

#[test]
fn config_disabled() {
    require_kernel!(5, 10);
    init();
    let mut ring = Ring::config(1).disable().build().unwrap();

    // In a disabled state, so we expect an error.
    let err = ring.poll(None).unwrap_err();
    assert_eq!(err.raw_os_error(), Some(libc::EBADFD));

    // Enabling it should allow us to poll.
    ring.enable().unwrap();
    ring.poll(Some(Duration::from_millis(1))).unwrap();
}

#[test]
fn config_single_issuer() {
    require_kernel!(6, 0);
    init();

    let ring = Ring::config(1).single_issuer().build().unwrap();

    // This is fine.
    let buf_pool = ReadBufPool::new(ring.submission_queue().clone(), 2, BUF_SIZE as u32).unwrap();
    drop(buf_pool);

    thread::spawn(move || {
        // This is not (we're on a different thread).
        let err =
            ReadBufPool::new(ring.submission_queue().clone(), 2, BUF_SIZE as u32).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EEXIST));
    })
    .join()
    .unwrap();
}

#[test]
fn config_single_issuer_disabled_ring() {
    require_kernel!(6, 0);
    init();

    let mut ring = Ring::config(1).single_issuer().disable().build().unwrap();

    thread::spawn(move || {
        ring.enable().unwrap();

        // Since this thread enabled the ring, this is now fine.
        let buf_pool =
            ReadBufPool::new(ring.submission_queue().clone(), 2, BUF_SIZE as u32).unwrap();
        drop(buf_pool);
    })
    .join()
    .unwrap();
}

#[test]
fn config_defer_task_run() {
    require_kernel!(6, 1);
    init();

    let mut ring = Ring::config(1)
        .single_issuer()
        .defer_task_run()
        .with_kernel_thread(false)
        .build()
        .unwrap();

    ring.poll(Some(Duration::ZERO)).unwrap();
}

#[test]
fn wake_ring() {
    init();
    let mut ring = Ring::config(2)
        .with_idle_timeout(Duration::from_millis(1))
        .build()
        .unwrap();
    let sq = ring.submission_queue().clone();

    let handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(10));
        sq.wake();
    });

    // Should be awoken by the wake call above.
    ring.poll(None).unwrap();
    handle.join().unwrap();
}

#[test]
fn wake_ring_before_poll_nop() {
    init();
    let mut ring = Ring::new(2).unwrap();
    let sq = ring.submission_queue().clone();

    sq.wake();

    // Should be awoken by the wake call above.
    ring.poll(None).unwrap();
}

#[test]
fn wake_ring_after_ring_dropped() {
    init();
    let ring = Ring::new(2).unwrap();
    let sq = ring.submission_queue().clone();

    drop(ring);
    sq.wake();
}

#[test]
fn message_sending() {
    require_kernel!(5, 18);

    const DATA1: u32 = 123;
    const DATA2: u32 = u32::MAX;

    let sq = test_queue();
    let waker = Waker::new();

    is_send::<MsgListener>();
    is_sync::<MsgListener>();
    is_send::<SendMsg>();
    is_sync::<SendMsg>();
    is_send::<MsgToken>();
    is_sync::<MsgToken>();

    let (msg_listener, msg_token) = sq.clone().msg_listener().unwrap();
    let mut msg_listener = pin!(msg_listener);
    start_mulitshot_op(msg_listener.as_mut());

    // Send some messages.
    sq.try_send_msg(msg_token, DATA1).unwrap();
    waker.block_on(pin!(sq.send_msg(msg_token, DATA2))).unwrap();

    assert_eq!(waker.block_on(next(msg_listener.as_mut())), Some(DATA1));
    assert_eq!(waker.block_on(next(msg_listener.as_mut())), Some(DATA2));

    assert!(poll_nop(Pin::new(&mut next(msg_listener.as_mut()))).is_pending());
}

#[test]
fn test_oneshot_poll() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<OneshotPoll>();
    is_sync::<OneshotPoll>();

    let (mut receiver, mut sender) = pipe2().unwrap();

    let sender_write = pin!(oneshot_poll(&sq, sender.as_fd(), libc::POLLOUT as _));
    let receiver_read = pin!(oneshot_poll(&sq, receiver.as_fd(), libc::POLLIN as _));

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

    let mut receiver_read = oneshot_poll(&sq, receiver.as_fd(), libc::POLLIN as _);

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

    let mut receiver_read = pin!(oneshot_poll(&sq, receiver.as_fd(), libc::POLLIN as _));
    start_op(&mut receiver_read);

    waker.block_on(receiver_read.cancel()).unwrap();
    expect_io_errno(waker.block_on(receiver_read), libc::ECANCELED);
    drop(sender);
}

#[test]
fn multishot_poll() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<MultishotPoll>();
    is_sync::<MultishotPoll>();

    let (mut receiver, mut sender) = pipe2().unwrap();

    let mut receiver_read = pin!(sq.multishot_poll(receiver.as_fd(), libc::POLLIN as _));
    start_mulitshot_op(Pin::new(&mut receiver_read));

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

    let mut receiver_read = pin!(sq.multishot_poll(receiver.as_fd(), libc::POLLIN as _));
    start_mulitshot_op(receiver_read.as_mut());

    waker.block_on(receiver_read.cancel()).unwrap();
    assert!(waker.block_on(next(receiver_read)).is_none());
    drop(sender);
}

#[test]
fn drop_multishot_poll() {
    let sq = test_queue();

    let (receiver, sender) = pipe2().unwrap();

    let mut receiver_read = sq.multishot_poll(receiver.as_fd(), libc::POLLIN as _);

    start_mulitshot_op(Pin::new(&mut receiver_read));

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
            libc::MADV_WILLNEED,
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
        .block_on(process::wait_on(sq, &process, libc::WEXITED))
        .expect("failed wait");

    assert_eq!(info.si_signo, libc::SIGCHLD);
    assert_eq!(info.si_code, libc::CLD_EXITED);
    // SAFETY: these fields are set if `si_signo == SIGCHLD`.
    assert_eq!(unsafe { info.si_pid() }, pid as i32);
    assert_eq!(unsafe { info.si_status() }, libc::EXIT_SUCCESS);
    assert!(unsafe { info.si_utime() } >= 1);
    assert!(unsafe { info.si_stime() } >= 1);
}

#[test]
fn process_wait_on_cancel() {
    require_kernel!(6, 7);
    let sq = test_queue();
    let waker = Waker::new();

    let mut process = Command::new("sleep").arg("1000").spawn().unwrap();

    let mut future = process::wait_on(sq, &process, libc::WEXITED);
    let result = poll_nop(Pin::new(&mut future));
    if !result.is_pending() {
        panic!("unexpected result, expected it to return Poll::Pending");
    }

    waker.block_on(future.cancel()).unwrap();

    process.kill().unwrap();
    process.wait().unwrap();
}

#[test]
fn process_wait_on_drop_before_complete() {
    require_kernel!(6, 7);
    let sq = test_queue();

    let process = Command::new("sleep").arg("1000").spawn().unwrap();

    let mut future = process::wait_on(sq, &process, libc::WEXITED);
    let result = poll_nop(Pin::new(&mut future));
    if !result.is_pending() {
        panic!("unexpected result, expected it to return Poll::Pending");
    }
    drop(future);
}

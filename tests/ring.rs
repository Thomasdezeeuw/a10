//! Tests for [`a10::Ring`].

#![feature(async_iterator)]

use std::fs::File;
use std::future::Future;
use std::io::{self, Read, Write};
use std::mem::take;
use std::os::fd::{AsFd, FromRawFd, RawFd};
use std::pin::{pin, Pin};
use std::sync::{Arc, Mutex};
use std::task::{self, Poll, Wake};
use std::thread;
use std::time::{Duration, Instant};

use a10::cancel::Cancel;
use a10::fs::OpenOptions;
use a10::poll::OneshotPoll;
use a10::{AsyncFd, Config, Ring, SubmissionQueue};

mod util;
use util::{expect_io_errno, init, is_send, is_sync, poll_nop, test_queue, Waker};

const DATA: &[u8] = b"Hello, World!";

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
fn oneshot_poll() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<OneshotPoll>();
    is_sync::<OneshotPoll>();

    let (mut receiver, mut sender) = pipe2().unwrap();

    let mut sender_write = pin!(sq.oneshot_poll(sender.as_fd(), libc::POLLOUT as _));
    let mut receiver_read = pin!(sq.oneshot_poll(receiver.as_fd(), libc::POLLIN as _));

    // Poll once to start the operations.
    assert!(poll_nop(sender_write.as_mut()).is_pending());
    assert!(poll_nop(receiver_read.as_mut()).is_pending());

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
fn cancel_oneshot_poll() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<OneshotPoll>();
    is_sync::<OneshotPoll>();

    let (receiver, sender) = pipe2().unwrap();

    let mut receiver_read = pin!(sq.oneshot_poll(receiver.as_fd(), libc::POLLIN as _));
    // Poll once to start the operations.
    assert!(poll_nop(receiver_read.as_mut()).is_pending());

    waker.block_on(receiver_read.cancel()).unwrap();
    expect_io_errno(waker.block_on(receiver_read), libc::ECANCELED);
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

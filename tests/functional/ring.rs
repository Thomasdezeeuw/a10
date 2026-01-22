use std::future::Future;
use std::io;
use std::mem::take;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{self, Poll, Wake};
#[cfg(any(target_os = "android", target_os = "linux"))]
use std::thread;
use std::time::{Duration, Instant};

use a10::fs::{Open, OpenOptions};
use a10::{Ring, SubmissionQueue};

use crate::util::{LOREM_IPSUM_50, init, is_send, is_sync, poll_nop};

#[test]
fn ring_size() {
    #[cfg(any(target_os = "android", target_os = "linux"))]
    const SIZE: usize = 48;
    #[cfg(any(
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "ios",
        target_os = "macos",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "tvos",
        target_os = "visionos",
        target_os = "watchos",
    ))]
    const SIZE: usize = 32;
    assert_eq!(std::mem::size_of::<Ring>(), SIZE);
    assert_eq!(std::mem::size_of::<Option<Ring>>(), SIZE);
}

#[test]
fn ring_is_send_and_sync() {
    is_send::<Ring>();
    is_sync::<Ring>();
}

#[test]
fn sq_is_send_and_sync() {
    is_send::<SubmissionQueue>();
    is_sync::<SubmissionQueue>();
}

#[test]
fn sq_size() {
    assert_eq!(std::mem::size_of::<SubmissionQueue>(), 8);
    assert_eq!(std::mem::size_of::<Option<SubmissionQueue>>(), 8);
}

#[test]
fn dropping_ring_unmaps_queues() {
    init();
    let ring = Ring::new().unwrap();
    drop(ring);
}

#[test]
fn polling_with_timeout() -> io::Result<()> {
    const TIMEOUT: Duration = Duration::from_millis(100);
    const MARGIN: Duration = Duration::from_millis(50);

    init();
    let mut ring = Ring::new().unwrap();

    let start = Instant::now();
    ring.poll(Some(TIMEOUT)).unwrap();
    let elapsed = start.elapsed();
    assert!(
        elapsed <= (TIMEOUT + MARGIN),
        "polling elapsed: {elapsed:?}, expected: {TIMEOUT:?}",
    );
    Ok(())
}

#[test]
fn submission_queue_full_is_handled_internally() {
    const SIZE: usize = 31396;
    const N: usize = (usize::BITS as usize) + 10;
    const BUF_SIZE: usize = SIZE / N;

    init();
    let mut ring = Ring::new().unwrap();
    let sq = ring.sq();
    let path = LOREM_IPSUM_50.path;
    let expected = LOREM_IPSUM_50.content;

    let mut future: Open = OpenOptions::new().open(sq, path.into());
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
        .map(|i| {
            let fut = file
                .read(Vec::with_capacity(BUF_SIZE))
                .from((i * BUF_SIZE) as u64);
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
            let mut ctx = task::Context::from_waker(waker);
            match Pin::new(future).poll(&mut ctx) {
                Poll::Ready(result) => {
                    *fut = None;
                    let read_buf = result.unwrap();
                    assert_eq!(read_buf, &expected[i * BUF_SIZE..(i + 1) * BUF_SIZE]);
                }
                Poll::Pending => {}
            }
        }
    }

    loop {
        // Poll all futures that got a wake up.
        for i in take(&mut *indices.lock().unwrap()).into_iter() {
            if let Some((future, waker)) = &mut futures[i] {
                let mut ctx = task::Context::from_waker(waker);
                match Pin::new(future).poll(&mut ctx) {
                    Poll::Ready(result) => {
                        futures[i] = None;
                        let read_buf = result.unwrap();
                        assert_eq!(read_buf, &expected[i * BUF_SIZE..(i + 1) * BUF_SIZE]);
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
#[cfg(any(target_os = "android", target_os = "linux"))]
fn wake_ring_with_kernel_thread() {
    init();
    let mut ring = Ring::config()
        .with_kernel_thread()
        .with_idle_timeout(Duration::from_millis(1))
        .build()
        .unwrap();
    let sq = ring.sq();

    let handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(10));
        sq.wake();
    });

    // Should be awoken by the wake call above.
    ring.poll(None).unwrap();
    handle.join().unwrap();
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn wake_ring_no_kernel_thread() {
    init();
    // Defaults to no kernel thread.
    let mut ring = Ring::config()
        .with_idle_timeout(Duration::from_millis(1))
        .build()
        .unwrap();
    let sq = ring.sq();

    let handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(10));
        sq.wake();
    });

    // Should be awoken by the wake call above.
    ring.poll(None).unwrap();
    handle.join().unwrap();
}

#[test]
fn wake_ring_after_ring_dropped() {
    init();
    let ring = Ring::new().unwrap();
    let sq = ring.sq();

    drop(ring);
    sq.wake();
}

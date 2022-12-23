#![allow(dead_code)] // Not all tests use all code here.

use std::any::Any;
use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::LazyLock;
use std::task::{self, Poll};
use std::thread::{self, Thread};
use std::{io, panic, process, str};

use a10::{Ring, SubmissionQueue};

/// Size of a single page in bytes.
pub(crate) const PAGE_SIZE: usize = 4096;

/// Start a single background thread for polling and return the submission
/// queue.
/// Create a [`Ring`] for testing.
pub(crate) fn test_queue() -> SubmissionQueue {
    static TEST_SQ: LazyLock<SubmissionQueue> = LazyLock::new(|| {
        let mut ring = Ring::new(128).expect("failed to create test ring");
        let sq = ring.submission_queue();
        thread::spawn(move || {
            let res = panic::catch_unwind(move || loop {
                ring.poll(None).expect("failed to poll ring");
            });
            match res {
                Ok(()) => (),
                Err(err) => {
                    let msg = panic_message(&err);
                    let msg = format!("Polling thread panicked: {msg}\n");

                    // Bypass the buffered output and write directly to standard
                    // error.
                    let stderr = io::stderr();
                    let guard = stderr.lock();
                    let _ =
                        unsafe { libc::write(libc::STDERR_FILENO, msg.as_ptr().cast(), msg.len()) };
                    drop(guard);

                    // Since all the tests depend on this thread we'll abort the
                    // process since otherwise they'll wait for ever.
                    process::abort()
                }
            }
        });
        sq
    });
    TEST_SQ.clone()
}

/// Waker that blocks the current thread.
pub(crate) struct Waker {
    handle: Thread,
}

impl task::Wake for Waker {
    fn wake(self: Arc<Self>) {
        self.handle.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.handle.unpark();
    }
}

impl Waker {
    /// Create a new `Waker`.
    pub(crate) fn new() -> Arc<Waker> {
        Arc::new(Waker {
            handle: thread::current(),
        })
    }

    /// Poll the `future` until completion, blocking when it can't make
    /// progress.
    pub(crate) fn block_on<Fut>(self: &Arc<Waker>, future: Fut) -> Fut::Output
    where
        Fut: IntoFuture,
    {
        // Pin the `Future` to stack.
        let mut future = future.into_future();
        let mut future = unsafe { Pin::new_unchecked(&mut future) };

        let task_waker = task::Waker::from(self.clone());
        let mut task_ctx = task::Context::from_waker(&task_waker);
        loop {
            match Future::poll(future.as_mut(), &mut task_ctx) {
                Poll::Ready(res) => return res,
                // The waking implementation will `unpark` us.
                Poll::Pending => thread::park(),
            }
        }
    }
}

/// Defer execution of function `f`.
pub(crate) fn defer<F: FnOnce()>(f: F) -> Defer<F> {
    Defer { f: Some(f) }
}

pub(crate) struct Defer<F: FnOnce()> {
    f: Option<F>,
}

impl<F: FnOnce()> Drop for Defer<F> {
    fn drop(&mut self) {
        let f = self.f.take().unwrap();
        if thread::panicking() {
            if let Err(err) = panic::catch_unwind(panic::AssertUnwindSafe(f)) {
                let msg = panic_message(&err);
                eprintln!("panic while already panicking: {msg}");
            }
        } else {
            f()
        }
    }
}

fn panic_message<'a>(err: &'a (dyn Any + Send + 'static)) -> &'a str {
    match err.downcast_ref::<&'static str>() {
        Some(s) => *s,
        None => match err.downcast_ref::<String>() {
            Some(s) => &**s,
            None => "<unknown>",
        },
    }
}

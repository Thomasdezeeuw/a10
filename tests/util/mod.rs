//! Test utilities.

#![allow(dead_code)] // Not all tests use all code here.

use std::any::Any;

#[cfg(feature = "nightly")]
use std::async_iter::AsyncIterator;
use std::fs::{remove_dir, remove_file};
use std::future::{Future, IntoFuture};
use std::io::{self, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::os::fd::{AsFd, AsRawFd};
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Once, OnceLock};
use std::task::{self, Poll};
use std::thread::{self, Thread};
use std::{fmt, mem, panic, process, ptr, str};

use a10::net::socket;
use a10::{AsyncFd, Ring, SubmissionQueue};

/// Initialise logging.
///
/// Automatically called when [`test_queue`] is used.
pub(crate) fn init() {
    static START: Once = Once::new();
    START.call_once(|| {
        std_logger::Config::logfmt().with_call_location(true).init();
    });
}

/// Size of a single page, often 4096.
#[allow(clippy::cast_sign_loss)] // Page size shouldn't be negative.
pub(crate) fn page_size() -> usize {
    static PAGE_SIZE: OnceLock<usize> = OnceLock::new();
    *PAGE_SIZE.get_or_init(|| unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize })
}

/// Start a single background thread for polling and return the submission
/// queue.
/// Create a [`Ring`] for testing.
pub(crate) fn test_queue() -> SubmissionQueue {
    static TEST_SQ: OnceLock<SubmissionQueue> = OnceLock::new();
    TEST_SQ
        .get_or_init(|| {
            init();

            let config = Ring::config(128).with_kernel_thread(true);
            let mut ring = match config.clone().build() {
                Ok(ring) => ring,
                Err(err) => {
                    log::warn!("creating test ring without a kernel thread");
                    if let Ok(ring) = config.with_kernel_thread(false).build() {
                        ring
                    } else {
                        panic!("failed to create test ring: {err}");
                    }
                }
            };
            let sq = ring.submission_queue().clone();
            thread::spawn(move || {
                let res = panic::catch_unwind(move || loop {
                    match ring.poll(None) {
                        Ok(()) => continue,
                        Err(ref err) if err.kind() == io::ErrorKind::Interrupted => continue,
                        Err(err) => panic!("unexpected error polling: {err}"),
                    }
                });
                match res {
                    Ok(()) => (),
                    Err(err) => {
                        let msg = panic_message(&*err);
                        let msg = format!("Polling thread panicked: {msg}\n");

                        // Bypass the buffered output and write directly to standard
                        // error.
                        let stderr = io::stderr();
                        let mut guard = stderr.lock();
                        let _ = guard.flush();
                        let _ = unsafe {
                            libc::write(libc::STDERR_FILENO, msg.as_ptr().cast(), msg.len())
                        };
                        drop(guard);

                        // Since all the tests depend on this thread we'll abort the
                        // process since otherwise they'll wait for ever.
                        process::abort()
                    }
                }
            });
            sq
        })
        .clone()
}

pub(crate) fn is_sync<T: Sync>() {}
pub(crate) fn is_send<T: Send>() {}

pub(crate) struct TestFile {
    pub(crate) path: &'static str,
    pub(crate) content: &'static [u8],
}

pub(crate) static LOREM_IPSUM_5: TestFile = TestFile {
    path: "tests/data/lorem_ipsum_5.txt",
    content: include_bytes!("../data/lorem_ipsum_5.txt"),
};

pub(crate) static LOREM_IPSUM_50: TestFile = TestFile {
    path: "tests/data/lorem_ipsum_50.txt",
    content: include_bytes!("../data/lorem_ipsum_50.txt"),
};

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

/// Poll the `future` once with a no-op waker.
pub(crate) fn poll_nop<Fut>(future: Pin<&mut Fut>) -> Poll<Fut::Output>
where
    Fut: Future,
{
    const NOP_WAKER: task::RawWakerVTable = task::RawWakerVTable::new(
        |_| task::RawWaker::new(ptr::null(), &NOP_WAKER), // clone.
        |_| {},                                           // wake.
        |_| {},                                           // wake_by_ref.
        |_| {},                                           // drop.
    );
    let task_waker = unsafe { task::Waker::from_raw(task::RawWaker::new(ptr::null(), &NOP_WAKER)) };
    let mut task_ctx = task::Context::from_waker(&task_waker);
    Future::poll(future, &mut task_ctx)
}

/// Poll the `future` until completion, polling `ring` when it can't make
/// progress.
pub(crate) fn block_on<Fut>(ring: &mut Ring, future: Fut) -> Fut::Output
where
    Fut: IntoFuture,
{
    // Pin the `Future` to stack.
    let mut future = future.into_future();
    let mut future = unsafe { Pin::new_unchecked(&mut future) };

    let waker = nop_waker();
    let mut task_ctx = task::Context::from_waker(&waker);
    loop {
        match Future::poll(future.as_mut(), &mut task_ctx) {
            Poll::Ready(res) => return res,
            // The waking implementation will `unpark` us.
            Poll::Pending => ring.poll(None).expect("failed to poll ring"),
        }
    }
}

fn nop_waker() -> task::Waker {
    static VTABLE: task::RawWakerVTable = task::RawWakerVTable::new(
        |_| task::RawWaker::new(ptr::null(), &VTABLE),
        |_| {},
        |_| {},
        |_| {},
    );
    unsafe {
        let raw_waker = task::RawWaker::new(ptr::null(), &VTABLE);
        task::Waker::from_raw(raw_waker)
    }
}

/// Fake definition of `std::async_iter::AsyncIterator` for rustc stable.
#[cfg(not(feature = "nightly"))]
pub(crate) trait AsyncIterator {
    type Item;

    fn poll_next(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Option<Self::Item>>;
}

#[cfg(not(feature = "nightly"))]
impl<I: ?Sized + AsyncIterator + Unpin> AsyncIterator for &mut I {
    type Item = I::Item;

    fn poll_next(
        mut self: Pin<&mut Self>,
        ctx: &mut task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        I::poll_next(Pin::new(&mut **self), ctx)
    }
}

#[cfg(not(feature = "nightly"))]
impl<P> AsyncIterator for Pin<P>
where
    P: std::ops::DerefMut,
    P::Target: AsyncIterator,
{
    type Item = <P::Target as AsyncIterator>::Item;

    fn poll_next(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Option<Self::Item>> {
        // SAFETY: this is the same as the unstable `Pin::as_deref_mut` impl.
        <P::Target as AsyncIterator>::poll_next(unsafe { self.get_unchecked_mut() }.as_mut(), ctx)
    }
}

macro_rules! op_async_iter {
    ($name: ty => $item: ty) => {
        #[cfg(not(feature = "nightly"))]
        impl AsyncIterator for $name {
            type Item = io::Result<$item>;

            fn poll_next(
                self: Pin<&mut Self>,
                ctx: &mut task::Context<'_>,
            ) -> Poll<Option<Self::Item>> {
                self.poll_next(ctx)
            }
        }
    };
}

op_async_iter!(a10::net::MultishotAccept<'_> => AsyncFd);
op_async_iter!(a10::net::MultishotRecv<'_> => a10::io::ReadBuf);
op_async_iter!(a10::poll::MultishotPoll<'_> => a10::poll::PollEvent);

/// Return a [`Future`] that return the next item in the `iter` or `None`.
pub(crate) fn next<I: AsyncIterator>(iter: I) -> Next<I> {
    Next { iter }
}

pub(crate) struct Next<I> {
    iter: I,
}

impl<I: AsyncIterator + Unpin> Future for Next<I> {
    type Output = Option<I::Item>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.iter).poll_next(ctx)
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
                let msg = panic_message(&*err);
                eprintln!("panic while already panicking: {msg}");
            }
        } else {
            f()
        }
    }
}

pub(crate) fn remove_test_file(path: &Path) {
    match remove_file(path) {
        Ok(()) => {}
        Err(ref err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => panic!("unexpected error removing test file: {err}"),
    }
}

pub(crate) fn remove_test_dir(path: &Path) {
    match remove_dir(path) {
        Ok(()) => {}
        Err(ref err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => panic!("unexpected error removing test directory: {err}"),
    }
}

fn panic_message<'a>(err: &'a (dyn Any + Send + 'static)) -> &'a str {
    match err.downcast_ref::<&str>() {
        Some(s) => *s,
        None => match err.downcast_ref::<String>() {
            Some(s) => &**s,
            None => "<unknown>",
        },
    }
}

/// Expect `result` to contain an [`io::Error`] with `expected` error kind.
#[track_caller]
pub(crate) fn expect_io_error_kind<T: fmt::Debug>(
    result: Result<T, io::Error>,
    expected: io::ErrorKind,
) {
    match result {
        Ok(value) => panic!("unexpected ok result, value: {value:?}"),
        Err(ref err) if err.kind() == expected => return,
        Err(err) => panic!("unexpected error result, error: {err:?}"),
    }
}

/// Expect `result` to contain an [`io::Error`] with `expected` error number.
#[track_caller]
pub(crate) fn expect_io_errno<T>(result: Result<T, io::Error>, expected: libc::c_int) {
    match result {
        Ok(_) => panic!("unexpected ok result"),
        Err(ref err) if err.raw_os_error() == Some(expected) => return,
        Err(err) => panic!("unexpected error result, error: {err:?}"),
    }
}

/// Create an IPv4, TCP socket.
pub(crate) async fn tcp_ipv4_socket(sq: SubmissionQueue) -> AsyncFd {
    let domain = libc::AF_INET;
    let r#type = libc::SOCK_STREAM | libc::SOCK_CLOEXEC;
    let protocol = 0;
    let flags = 0;
    socket(sq, domain, r#type, protocol, flags)
        .await
        .expect("failed to create socket")
}

/// Create an IPv4, UPD socket.
pub(crate) async fn udp_ipv4_socket(sq: SubmissionQueue) -> AsyncFd {
    let domain = libc::AF_INET;
    let r#type = libc::SOCK_DGRAM | libc::SOCK_CLOEXEC;
    let protocol = libc::IPPROTO_UDP;
    let flags = 0;
    socket(sq, domain, r#type, protocol, flags)
        .await
        .expect("failed to create socket")
}

/// Bind `socket` to a local IPv4 addres with a random port and starts listening
/// on it. Returns the bound address.
pub(crate) fn bind_and_listen_ipv4(socket: &AsyncFd) -> SocketAddr {
    let address = bind_ipv4(socket);
    let fd = socket.as_fd().as_raw_fd();
    syscall!(listen(fd, 128)).expect("failed to listen on socket");
    address
}

pub(crate) fn bind_ipv4(socket: &AsyncFd) -> SocketAddr {
    let fd = socket.as_fd().as_raw_fd();
    let mut addr = libc::sockaddr_in {
        sin_family: libc::AF_INET as libc::sa_family_t,
        sin_port: 0,
        sin_addr: libc::in_addr {
            s_addr: u32::from_ne_bytes([127, 0, 0, 1]),
        },
        ..unsafe { mem::zeroed() }
    };
    let mut addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    syscall!(bind(fd, &addr as *const _ as *const _, addr_len)).expect("failed to bind socket");

    syscall!(getsockname(
        fd,
        &mut addr as *mut _ as *mut _,
        &mut addr_len
    ))
    .expect("failed to get socket address");

    SocketAddr::V4(SocketAddrV4::new(
        Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes()),
        u16::from_be(addr.sin_port),
    ))
}

/// Helper macro to execute a system call that returns an `io::Result`.
macro_rules! syscall {
    ($fn: ident ( $($arg: expr),* $(,)? ) ) => {{
        let res = unsafe { libc::$fn($( $arg, )*) };
        if res == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(res)
        }
    }};
}

pub(crate) use syscall;

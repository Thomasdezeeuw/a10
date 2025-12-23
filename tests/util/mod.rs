//! Test utilities.

#![allow(dead_code)] // Not all tests use all code here.

use std::any::Any;
#[cfg(feature = "nightly")]
use std::async_iter::AsyncIterator;
use std::cell::Cell;
use std::ffi::CStr;
use std::fs::{remove_dir, remove_file};
use std::future::{Future, IntoFuture};
use std::io::{self, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::os::fd::{AsRawFd, BorrowedFd};
use std::path::{Path, PathBuf};
use std::pin::{Pin, pin};
use std::sync::{Arc, Once, OnceLock};
use std::task::{self, Poll};
use std::thread::{self, Thread};
use std::{fmt, mem, panic, process, str};

use a10::io::{Buf, BufMut, BufMutSlice, BufSlice, IoMutSlice, IoSlice};
use a10::net::{Domain, Protocol, Type, socket};
use a10::{AsyncFd, Cancel, Ring, SubmissionQueue};

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

/// Return the major.minor version of the kernel.
pub(crate) fn kernel_version() -> (u32, u32) {
    static KERNEL_VERSION: OnceLock<(u32, u32)> = OnceLock::new();
    *KERNEL_VERSION.get_or_init(|| {
        // SAFETY: all zeros for `utsname` is valid.
        let mut uname: libc::utsname = unsafe { mem::zeroed() };
        syscall!(uname(&mut uname)).expect("failed to get kernel info");
        // SAFETY: kernel initialed `uname.release` for us with a NULL
        // terminating string.
        let release = unsafe { CStr::from_ptr(&uname.release as *const _) };
        let release = str::from_utf8(release.to_bytes()).expect("version not valid UTF-8");
        let mut parts = release.split('.');
        let major = parts.next().expect("missing major verison");
        let minor = parts.next().expect("missing minor verison");
        (
            major.parse().expect("invalid major version"),
            minor.parse().expect("invalid major version"),
        )
    })
}

/// Returns true if the kernel version is greater than major.minor, false
/// otherwise.
pub(crate) fn has_kernel_version(major: u32, minor: u32) -> bool {
    let got = kernel_version();
    got.0 > major || (got.0 == major && got.1 >= minor)
}

/// This `return`s from the function if the kernel version is smaller than
/// major.minor, continues if the kernel version is larger.
#[allow(unused_macros)]
macro_rules! require_kernel {
    ($major: expr, $minor: expr) => {{
        let major = $major;
        let minor = $minor;
        if !crate::util::has_kernel_version(major, minor) {
            println!("skipping test, kernel doesn't have required version {major}.{minor}");
            return;
        }
    }};
}

#[allow(unused_imports)]
pub(crate) use require_kernel;

/// Start a single background thread for polling and return the submission
/// queue.
/// Create a [`Ring`] for testing.
pub(crate) fn test_queue() -> SubmissionQueue {
    static TEST_SQ: OnceLock<SubmissionQueue> = OnceLock::new();
    TEST_SQ
        .get_or_init(|| {
            init();

            let config = Ring::config(128)
                .with_kernel_thread(true)
                .with_direct_descriptors(1024);
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
            let sq = ring.sq().clone();
            thread::spawn(move || {
                let res = panic::catch_unwind(move || {
                    loop {
                        match ring.poll(None) {
                            Ok(()) => continue,
                            Err(ref err) if err.kind() == io::ErrorKind::Interrupted => continue,
                            Err(err) => panic!("unexpected error polling: {err}"),
                        }
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
        let mut future = pin!(future.into_future());
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

/// Cancel `operation`.
///
/// `Future`s are inert and we can't determine if an operation has actually been
/// started or the submission queue was full. This means that we can't ensure
/// that the operation has been queued with the Kernel. This means that in some
/// cases we won't actually cancel the operation simply because it hasn't
/// started. This introduces flakyness in the tests.
///
/// To work around this we have this cancel function. If we fail to cancel the
/// operation we try to start the operation using `start_op`, before canceling t
/// again. Looping until the operation is canceled.
#[track_caller]
pub(crate) fn cancel<O, F>(waker: &Arc<Waker>, operation: &mut O, start_op: F)
where
    O: Cancel,
    F: Fn(&mut O),
{
    for _ in 0..100 {
        match waker.block_on(operation.cancel()) {
            Ok(()) => return,
            Err(ref err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => panic!("unexpected error canceling operation: {err}"),
        }

        start_op(operation);
    }
    panic!("couldn't cancel operation");
}

/// Cancel all operations of `fd`.
///
/// `Future`s are inert and we can't determine if an operation has actually been
/// started or the submission queue was full. This means that we can't ensure
/// that the operation has been queued with the Kernel. This means that in some
/// cases we won't actually cancel any operations simply because they haven't
/// started. This introduces flakyness in the tests.
///
/// To work around this we have this cancel all function. If we fail to get the
/// `expected` number of canceled operations we try to start the operations
/// using `start_op`, before canceling them again. Looping until we get the
/// expected number of canceled operations.
#[track_caller]
pub(crate) fn cancel_all<F: FnMut()>(
    waker: &Arc<Waker>,
    fd: &AsyncFd,
    mut start_op: F,
    expected: usize,
) {
    let mut canceled = 0;
    for _ in 0..100 {
        let n = waker
            .block_on(fd.cancel_all())
            .expect("failed to cancel all operations");
        canceled += n;
        if canceled >= expected {
            return;
        }

        start_op();
    }
    panic!("couldn't cancel all expected operations");
}

/// Start an A10 operation, assumes `future` is a A10 `Future`.
#[track_caller]
pub(crate) fn start_op<Fut>(future: &mut Fut)
where
    Fut: Future + Unpin,
    Fut::Output: fmt::Debug,
{
    let result = poll_nop(Pin::new(future));
    if !result.is_pending() {
        panic!("unexpected result: {result:?}, expected it to return Poll::Pending");
    }
}

/// Start an A10 multishot operation, assumes `iter` is a A10 `AsyncIterator`.
#[track_caller]
pub(crate) fn start_mulitshot_op<I>(iter: &mut I)
where
    I: AsyncIterator + Unpin,
    I::Item: fmt::Debug,
{
    start_op(&mut next(iter));
}

/// Poll the `future` once with a no-op waker.
pub(crate) fn poll_nop<Fut>(future: Pin<&mut Fut>) -> Poll<Fut::Output>
where
    Fut: Future,
{
    let task_waker = task::Waker::noop();
    let mut task_ctx = task::Context::from_waker(&task_waker);
    Future::poll(future, &mut task_ctx)
}

/// Poll the `future` until completion, polling `ring` when it can't make
/// progress.
pub(crate) fn block_on<Fut>(ring: &mut Ring, future: Fut) -> Fut::Output
where
    Fut: IntoFuture,
{
    let mut future = pin!(future.into_future());

    let waker = task::Waker::noop();
    let mut task_ctx = task::Context::from_waker(&waker);
    loop {
        match Future::poll(future.as_mut(), &mut task_ctx) {
            Poll::Ready(res) => return res,
            // The waking implementation will `unpark` us.
            Poll::Pending => ring.poll(None).expect("failed to poll ring"),
        }
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
        <P::Target as AsyncIterator>::poll_next(Pin::as_deref_mut(self), ctx)
    }
}

macro_rules! op_async_iter {
    ($name: ty => $item: ty) => {
        #[cfg(not(feature = "nightly"))]
        impl AsyncIterator for $name {
            type Item = $item;

            fn poll_next(
                self: Pin<&mut Self>,
                ctx: &mut task::Context<'_>,
            ) -> Poll<Option<Self::Item>> {
                self.poll_next(ctx)
            }
        }
    };
}

op_async_iter!(a10::msg::Listener => u32);
op_async_iter!(a10::io::MultishotRead<'_> => io::Result<a10::io::ReadBuf>);
op_async_iter!(a10::net::MultishotAccept<'_> => io::Result<AsyncFd>);
op_async_iter!(a10::net::MultishotRecv<'_> => io::Result<a10::io::ReadBuf>);
op_async_iter!(a10::poll::MultishotPoll => io::Result<a10::poll::Event>);

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

pub(crate) fn tmp_path() -> PathBuf {
    static CREATE_TEMP_DIR: Once = Once::new();
    let mut tmp_dir = std::env::temp_dir();
    tmp_dir.push("a10_tests");
    CREATE_TEMP_DIR.call_once(|| {
        std::fs::create_dir_all(&tmp_dir).expect("failed to create temporary directory");
    });
    let n = getrandom::u64().expect("failed to get random data");
    tmp_dir.push(&format!("{n}"));
    tmp_dir
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
    new_socket(sq, Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).await
}

/// Create an IPv4, UPD socket.
pub(crate) async fn udp_ipv4_socket(sq: SubmissionQueue) -> AsyncFd {
    new_socket(sq, Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).await
}

pub(crate) async fn new_socket(
    sq: SubmissionQueue,
    domain: Domain,
    r#type: Type,
    protocol: Option<Protocol>,
) -> AsyncFd {
    if !has_kernel_version(5, 19) {
        // IORING_OP_SOCKET is only available since 5.19, fall back to a
        // blocking system call.
        unsafe {
            // SAFETY: these transmutes aren't safe.
            let domain = std::mem::transmute(domain);
            let r#type = std::mem::transmute(r#type);
            let protocol = match protocol {
                Some(protocol) => std::mem::transmute(protocol),
                None => 0,
            };
            // SAFETY: kernel initialises the socket for us.
            syscall!(socket(domain, r#type, protocol)).map(|fd| AsyncFd::from_raw_fd(fd, sq))
        }
    } else {
        socket(sq, domain, r#type, protocol).await
    }
    .expect("failed to create socket")
}

/// Bind `socket` to a local IPv4 addres with a random port and starts listening
/// on it. Returns the bound address.
pub(crate) async fn bind_and_listen_ipv4(socket: &AsyncFd) -> SocketAddr {
    let address = bind_ipv4(socket).await;
    let fd = fd(&socket).as_raw_fd();
    let backlog = 128;
    if !has_kernel_version(6, 11) {
        // IORING_OP_LISTEN is only available since 6.11, fall back to a
        // blocking system call.
        syscall!(listen(fd, backlog)).expect("failed to listen on socket");
    } else {
        socket
            .listen(backlog)
            .await
            .expect("failed to listen on socket");
    }
    address
}

pub(crate) async fn bind_ipv4(socket: &AsyncFd) -> SocketAddr {
    let fd = fd(&socket).as_raw_fd();
    let mut addr = libc::sockaddr_in {
        sin_family: libc::AF_INET as libc::sa_family_t,
        sin_port: 0,
        sin_addr: libc::in_addr {
            s_addr: u32::from_ne_bytes([127, 0, 0, 1]),
        },
        ..unsafe { mem::zeroed() }
    };
    let mut addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    if !has_kernel_version(6, 11) {
        // IORING_OP_BIND is only available since 6.11, fall back to a blocking
        // system call.
        syscall!(bind(fd, &addr as *const _ as *const _, addr_len)).expect("failed to bind socket");
    } else {
        socket
            .bind::<SocketAddr>(([127, 0, 0, 1], 0).into())
            .await
            .expect("failed to bind socket");
    }

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

pub(crate) fn fd<'fd>(fd: &'fd AsyncFd) -> BorrowedFd<'fd> {
    fd.as_fd().expect("not a file descriptor")
}

/// Helper macro to execute a system call that returns an `io::Result`.
macro_rules! syscall {
    ($fn: ident ( $($arg: expr),* $(,)? ) ) => {{
        #[allow(unused_unsafe)]
        let res = unsafe { libc::$fn($( $arg, )*) };
        if res == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(res)
        }
    }};
}

pub(crate) use syscall;

// NOTE: this implementation is BROKEN! It's only used to test the write_all
// method.
#[derive(Debug)]
pub(crate) struct BadBuf {
    pub(crate) calls: Cell<usize>,
}

impl BadBuf {
    pub(crate) const DATA: [u8; 30] = [
        123, 123, 123, 123, 123, 123, 123, 123, 123, 123, 200, 200, 200, 200, 200, 200, 200, 200,
        200, 200, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    ];
}

unsafe impl Buf for BadBuf {
    unsafe fn parts(&self) -> (*const u8, u32) {
        let calls = self.calls.get();
        self.calls.set(calls + 1);

        let ptr = BadBuf::DATA.as_slice().as_ptr();
        // NOTE: we don't increase the pointer offset as the `SkipBuf` internal
        // to the WriteAll future already does that for us.
        match calls {
            // Per system/io_uring call we call `Buf::parts` for:
            // 1. passing to the kernel,
            // 2. to poison the memory,
            // 3. to unpoison the memory (once the call is clomplete).
            0..3 => (ptr, 10),
            3..6 => (ptr, 20),
            _ => (ptr, 30),
        }
    }
}

// NOTE: this implementation is BROKEN! It's only used to test the write_all
// method.
#[derive(Debug)]
pub(crate) struct BadReadBuf {
    pub(crate) data: Vec<u8>,
}

unsafe impl BufMut for BadReadBuf {
    unsafe fn parts_mut(&mut self) -> (*mut u8, u32) {
        let (ptr, size) = unsafe { self.data.parts_mut() };
        if size >= 10 { (ptr, 10) } else { (ptr, size) }
    }

    unsafe fn set_init(&mut self, n: usize) {
        unsafe { self.data.set_init(n) };
    }
}

// NOTE: this implementation is BROKEN! It's only used to test the write_all
// method.
#[derive(Debug)]
pub(crate) struct BadReadBufSlice {
    pub(crate) data: [Vec<u8>; 2],
}

unsafe impl BufMutSlice<2> for BadReadBufSlice {
    unsafe fn as_iovecs_mut(&mut self) -> [IoMutSlice; 2] {
        unsafe {
            let mut iovecs = self.data.as_iovecs_mut();
            if iovecs[0].len() >= 10 {
                iovecs[0].set_len(10);
                iovecs[1].set_len(5);
            }
            iovecs
        }
    }

    unsafe fn set_init(&mut self, n: usize) {
        if n == 0 {
            return;
        }

        unsafe {
            if self.as_iovecs_mut()[0].len() == 10 {
                self.data[0].set_init(10);
                self.data[1].set_init(n - 10);
            } else {
                self.data.set_init(n);
            }
        }
    }
}

// NOTE: this implementation is BROKEN! It's only used to test the write_all
// method.
pub(crate) struct GrowingBufSlice {
    pub(crate) data: [Vec<u8>; 2],
}

unsafe impl BufMutSlice<2> for GrowingBufSlice {
    unsafe fn as_iovecs_mut(&mut self) -> [IoMutSlice; 2] {
        unsafe { self.data.as_iovecs_mut() }
    }

    unsafe fn set_init(&mut self, n: usize) {
        unsafe { self.data.set_init(n) };
        self.data[1].reserve(200);
    }
}

// NOTE: this implementation is BROKEN! It's only used to test the
// write_all_vectored method.
#[derive(Debug)]
pub(crate) struct BadBufSlice {
    pub(crate) calls: Cell<usize>,
}

impl BadBufSlice {
    pub(crate) const DATA1: &'static [u8] = &[123, 123, 123, 123, 123, 123, 123, 123, 123, 123];
    pub(crate) const DATA2: &'static [u8] = &[200, 200, 200, 200, 200, 200, 200, 200, 200, 200];
    pub(crate) const DATA3: &'static [u8] = &[255, 255, 255, 255, 255, 255, 255, 255, 255, 255];
}

unsafe impl BufSlice<3> for BadBufSlice {
    unsafe fn as_iovecs(&self) -> [IoSlice; 3] {
        let calls = self.calls.get();
        self.calls.set(calls + 1);
        let max_length = if calls == 0 { 5 } else { 10 };
        unsafe {
            [
                IoSlice::new(&Self::DATA1),
                IoSlice::new(&Self::DATA2),
                IoSlice::new(&&Self::DATA3[0..max_length]),
            ]
        }
    }
}

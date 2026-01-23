//! Test utilities.

#![allow(dead_code)] // Not all tests use all code here.

use std::any::Any;
#[cfg(feature = "nightly")]
use std::async_iter::AsyncIterator;
use std::cell::Cell;
use std::fs::{remove_dir_all, remove_file};
use std::future::{Future, IntoFuture};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::os::fd::{AsRawFd, BorrowedFd, RawFd};
use std::path::{Path, PathBuf};
use std::pin::{Pin, pin};
use std::sync::{Arc, Once, OnceLock};
use std::task::{self, Poll};
use std::thread::{self, Thread};
use std::{fmt, io, mem, panic, ptr, str};

use a10::io::{Buf, BufMut, BufMutSlice, BufSlice, IoMutSlice, IoSlice};
use a10::net::{Domain, Protocol, Type, socket};
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

/// This `return`s from the function if the operation is unsupported (see
/// [`is_unsupported`]).
#[allow(unused_macros)]
macro_rules! ignore_unsupported {
    ($expr: expr) => {
        match $expr {
            Ok(ok) => Ok(ok),
            Err(ref err) if $crate::util::is_unsupported(err) => return,
            Err(err) => Err(err),
        }
    };
}

#[allow(unused_imports)]
pub(crate) use ignore_unsupported;

pub(crate) fn is_unsupported(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(libc::EOPNOTSUPP))
}

/// Start a single background thread for polling and return the submission
/// queue.
/// Create a [`Ring`] for testing.
pub(crate) fn test_queue() -> SubmissionQueue {
    static TEST_SQ: OnceLock<SubmissionQueue> = OnceLock::new();
    TEST_SQ
        .get_or_init(|| {
            init();

            let config = Ring::config();
            #[cfg(any(target_os = "android", target_os = "linux"))]
            let config = config.with_direct_descriptors(1024);
            let mut ring = match config.build() {
                Ok(ring) => ring,
                Err(err) => panic!("failed to create test ring: {err}"),
            };
            let sq = ring.sq();
            thread::Builder::new()
                .name("test_queue".into())
                .spawn(move || {
                    loop {
                        if let Err(err) = ring.poll(None) {
                            panic!("unexpected error polling: {err}");
                        }
                    }
                })
                .expect("failed to spawn test_queue thread");
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
                Poll::Pending => {
                    // Wake up the thread that polls the shared ring to ensure
                    // we make progress.
                    test_queue().wake();
                    thread::park();
                }
            }
        }
    }
}

/// Start an A10 operation, assumes `future` is a A10 `Future`.
#[track_caller]
pub(crate) fn start_op<Fut>(future: &mut Fut)
where
    Fut: Future + Unpin,
    Fut::Output: fmt::Debug,
{
    let result = poll_nop(Pin::new(future));
    assert!(
        result.is_pending(),
        "unexpected result: {result:?}, expected it to return Poll::Pending"
    );
}

/// Poll the `future` once with a no-op waker.
pub(crate) fn poll_nop<Fut>(future: Pin<&mut Fut>) -> Poll<Fut::Output>
where
    Fut: Future,
{
    let task_waker = task::Waker::noop();
    let mut task_ctx = task::Context::from_waker(task_waker);
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
    let mut task_ctx = task::Context::from_waker(waker);
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

op_async_iter!(a10::io::MultishotRead<'_> => io::Result<a10::io::ReadBuf>);
op_async_iter!(a10::net::MultishotAccept<'_> => io::Result<AsyncFd>);
op_async_iter!(a10::net::MultishotRecv<'_> => io::Result<a10::io::ReadBuf>);

#[cfg(not(feature = "nightly"))]
#[cfg(any(target_os = "android", target_os = "linux"))]
impl<'w> AsyncIterator for a10::fs::notify::Events<'w> {
    type Item = io::Result<&'w a10::fs::notify::Event>;

    fn poll_next(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Option<Self::Item>> {
        self.poll_next(ctx)
    }
}

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
            f();
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
    match remove_dir_all(path) {
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
    tmp_dir.push(format!("{n}"));
    tmp_dir
}

pub(crate) fn tmp_file_path(extension: &str) -> PathBuf {
    let mut path = tmp_path();
    path.set_extension(extension);
    path
}

fn panic_message<'a>(err: &'a (dyn Any + Send + 'static)) -> &'a str {
    match err.downcast_ref::<&str>() {
        Some(s) => s,
        None => match err.downcast_ref::<String>() {
            Some(s) => s,
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
        Err(ref err) if err.kind() == expected => {}
        Err(err) => panic!("unexpected error result, error: {err:?}"),
    }
}

/// Expect `result` to contain an [`io::Error`] with `expected` error number.
#[track_caller]
pub(crate) fn expect_io_errno<T>(result: Result<T, io::Error>, expected: libc::c_int) {
    match result {
        Ok(_) => panic!("unexpected ok result"),
        Err(ref err) if err.raw_os_error() == Some(expected) => {}
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

#[allow(clippy::missing_transmute_annotations)]
pub(crate) async fn new_socket(
    sq: SubmissionQueue,
    domain: Domain,
    r#type: Type,
    protocol: Option<Protocol>,
) -> AsyncFd {
    match socket(sq.clone(), domain, r#type, protocol).await {
        Ok(fd) => Ok(fd),
        Err(ref err) if is_unsupported(err) => unsafe {
            // IORING_OP_SOCKET is only available since 5.19, fall back to a
            // blocking system call.
            // SAFETY: these transmutes aren't safe.
            let domain = std::mem::transmute(domain);
            let r#type = std::mem::transmute(r#type);
            let protocol = match protocol {
                Some(protocol) => std::mem::transmute(protocol),
                None => 0,
            };
            // SAFETY: kernel initialises the socket for us.
            syscall!(socket(domain, r#type, protocol)).map(|fd| AsyncFd::from_raw_fd(fd, sq))
        },
        Err(err) => Err(err),
    }
    .expect("failed to create socket")
}

/// Bind `socket` to a local IPv4 addres with a random port and starts listening
/// on it. Returns the bound address.
pub(crate) async fn bind_and_listen_ipv4(socket: &AsyncFd) -> SocketAddr {
    let address = bind_ipv4(socket).await;
    let fd = fd(socket).as_raw_fd();
    let backlog = 128;

    match socket.listen(backlog).await {
        Ok(()) => Ok(()),
        Err(ref err) if is_unsupported(err) => {
            // IORING_OP_LISTEN is only available since 6.11, fall back to a
            // blocking system call.
            syscall!(listen(fd, backlog)).map(|_| ())
        }
        Err(err) => Err(err),
    }
    .expect("failed to listen on socket");
    address
}

pub(crate) async fn bind_ipv4(socket: &AsyncFd) -> SocketAddr {
    let fd = fd(socket).as_raw_fd();
    let mut addr = libc::sockaddr_in {
        sin_family: libc::AF_INET as libc::sa_family_t,
        sin_port: 0,
        sin_addr: libc::in_addr {
            s_addr: u32::from_ne_bytes([127, 0, 0, 1]),
        },
        ..unsafe { mem::zeroed() }
    };
    let mut addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;

    match socket.bind::<SocketAddr>(([127, 0, 0, 1], 0).into()).await {
        Ok(()) => Ok(()),
        Err(ref err) if is_unsupported(err) => {
            // IORING_OP_BIND is only available since 6.11, fall back to a
            // blocking system call.
            syscall!(bind(fd, ptr::from_ref(&addr).cast(), addr_len)).map(|_| ())
        }
        Err(err) => Err(err),
    }
    .expect("failed to bind socket");
    syscall!(getsockname(
        fd,
        ptr::from_mut(&mut addr).cast(),
        &raw mut addr_len
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

pub(crate) fn raw_pipe() -> [RawFd; 2] {
    let mut fds: [RawFd; 2] = [-1, -1];
    #[cfg(any(target_os = "android", target_os = "linux"))]
    syscall!(pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC)).expect("failed to create pipe");
    #[cfg(not(any(target_os = "android", target_os = "linux")))]
    {
        syscall!(pipe(fds.as_mut_ptr())).expect("failed to create pipe");
        syscall!(fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC)).unwrap();
        syscall!(fcntl(fds[1], libc::F_SETFD, libc::FD_CLOEXEC)).unwrap();
    }
    debug_assert!(fds[0] != -1);
    debug_assert!(fds[1] != -1);
    fds
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
    calls: Cell<usize>,
}

impl BadBuf {
    pub(crate) const fn new() -> BadBuf {
        BadBuf {
            calls: Cell::new(0),
        }
    }

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
        #[cfg(any(target_os = "android", target_os = "linux"))]
        return match calls {
            // Per system/io_uring call we call `Buf::parts` for:
            // 1. passing to the kernel & to poison the memory,
            // 2. to unpoison the memory (once the call is clomplete).
            0..2 => (ptr, 10),
            2..4 => (ptr, 20),
            _ => (ptr, 30),
        };
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
        return match calls {
            0..1 => (ptr, 10),
            1..2 => (ptr, 20),
            _ => (ptr, 30),
        };
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

    fn spare_capacity(&self) -> u32 {
        unreachable!("not implemented");
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
    pub(crate) const DATA3: &'static [u8] = &[255, 255, 255, 255, 255, 111, 111, 111, 111, 111];
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

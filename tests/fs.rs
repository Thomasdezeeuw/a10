//! Tests for the filesystem types

#![feature(once_cell)]

use std::env::temp_dir;
use std::fs::remove_file;
use std::future::Future;
use std::lazy::SyncLazy;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{self, Poll};
use std::thread::{self, Thread};
use std::{io, str};

use a10::fs::File;
use a10::{Ring, SubmissionQueue};

const PAGE_SIZE: usize = 4096;

struct TestFile {
    path: &'static str,
    content: &'static [u8],
}

static LOREM_IPSUM_5: TestFile = TestFile {
    path: "tests/data/lorem_ipsum_5.txt",
    content: include_bytes!("data/lorem_ipsum_5.txt"),
};

static LOREM_IPSUM_50: TestFile = TestFile {
    path: "tests/data/lorem_ipsum_50.txt",
    content: include_bytes!("data/lorem_ipsum_50.txt"),
};

/// Start a single background thread for polling and return the submission
/// queue.
/// Create a [`Ring`] for testing.
fn test_queue() -> SubmissionQueue {
    static TEST_SQ: SyncLazy<SubmissionQueue> = SyncLazy::new(|| {
        let mut ring = Ring::new(128).expect("failed to create test ring");
        let sq = ring.submission_queue();
        thread::spawn(move || loop {
            ring.poll(None).expect("failed to poll ring");
        });
        sq
    });
    TEST_SQ.clone()
}

#[test]
fn read_one_page() -> io::Result<()> {
    let sq = test_queue();
    test_read(sq, &LOREM_IPSUM_5, LOREM_IPSUM_5.content.len() + 1)
}

#[test]
fn read_multiple_pages_one_read() -> io::Result<()> {
    let sq = test_queue();
    test_read(sq, &LOREM_IPSUM_50, LOREM_IPSUM_50.content.len() + 1)
}

#[test]
fn read_multiple_pages_multiple_reads() -> io::Result<()> {
    // Tests that multiple reads work like expected w.r.t. things like offset
    // advancement.
    let sq = test_queue();
    test_read(sq, &LOREM_IPSUM_50, 4096)
}

#[test]
fn read_multiple_pages_multiple_reads_unaligned() -> io::Result<()> {
    let sq = test_queue();
    test_read(sq, &LOREM_IPSUM_50, 3000)
}

fn test_read(sq: SubmissionQueue, test_file: &TestFile, buf_size: usize) -> io::Result<()> {
    let waker = Waker::new();

    let path = test_file.path.into();
    let open_file = File::open(sq, path)?;
    let file = waker.block_on(open_file)?;

    let mut buf = Vec::with_capacity(buf_size);
    let mut read_bytes = 0;
    loop {
        buf.clear();
        let read = file.read(buf)?;
        buf = waker.block_on(read)?;
        if buf.is_empty() {
            panic!("read zero bytes");
        }

        assert!(
            buf == &test_file.content[read_bytes..read_bytes + buf.len()],
            "read content is different"
        );
        read_bytes += buf.len();
        if read_bytes >= test_file.content.len() {
            return Ok(());
        }
    }
}

#[test]
fn read_at_one_page() -> io::Result<()> {
    let sq = test_queue();
    test_read_at(sq, &LOREM_IPSUM_5, LOREM_IPSUM_5.content.len() + 1, 100)
}

#[test]
fn read_at_multiple_pages_one_read() -> io::Result<()> {
    let sq = test_queue();
    let offset = 8192;
    let buf_len = LOREM_IPSUM_50.content.len() + 1 - offset as usize;
    test_read_at(sq, &LOREM_IPSUM_50, buf_len, offset)
}

#[test]
fn read_at_multiple_pages_multiple_reads() -> io::Result<()> {
    // Tests that multiple reads work like expected w.r.t. things like offset
    // advancement.
    let sq = test_queue();
    test_read_at(sq, &LOREM_IPSUM_50, 4096, 16384)
}

fn test_read_at(
    sq: SubmissionQueue,
    test_file: &TestFile,
    buf_size: usize,
    mut offset: u64,
) -> io::Result<()> {
    let waker = Waker::new();

    let path = test_file.path.into();
    let open_file = File::open(sq, path)?;
    let file = waker.block_on(open_file)?;

    let mut buf = Vec::with_capacity(buf_size);
    let mut expected = &test_file.content[offset as usize..];
    loop {
        buf.clear();
        let read = file.read_at(buf, offset)?;
        buf = waker.block_on(read)?;

        if buf.is_empty() {
            panic!("read zero bytes");
        }

        assert!(buf == &expected[..buf.len()], "read content is different");
        expected = &expected[buf.len()..];
        offset += buf.len() as u64;
        if expected.is_empty() {
            return Ok(());
        }
    }
}

#[test]
fn write_hello_world() -> io::Result<()> {
    let sq = test_queue();
    let bufs = vec![b"Hello world".to_vec()];
    test_write("a10.write_hello_world", sq, bufs)
}

#[test]
fn write_one_page() -> io::Result<()> {
    let sq = test_queue();
    let bufs = vec![b"a".repeat(PAGE_SIZE)];
    test_write("a10.write_one_page", sq, bufs)
}

#[test]
fn write_multiple_pages_one_write() -> io::Result<()> {
    let sq = test_queue();
    let bufs = vec![b"b".repeat(4 * PAGE_SIZE)];
    test_write("a10.write_multiple_pages_one_write", sq, bufs)
}

#[test]
fn write_multiple_pages_mulitple_writes() -> io::Result<()> {
    let sq = test_queue();
    let bufs = vec![b"b".repeat(PAGE_SIZE), b"c".repeat(PAGE_SIZE)];
    test_write("a10.write_multiple_pages_mulitple_writes", sq, bufs)
}

#[test]
fn write_multiple_pages_mulitple_writes_unaligned() -> io::Result<()> {
    let sq = test_queue();
    let bufs = vec![
        b"Hello unalignment!".to_vec(),
        b"b".repeat(PAGE_SIZE),
        b"c".repeat(PAGE_SIZE),
    ];
    test_write(
        "a10.write_multiple_pages_mulitple_writes_unaligned",
        sq,
        bufs,
    )
}

fn test_write(name: &str, sq: SubmissionQueue, bufs: Vec<Vec<u8>>) -> io::Result<()> {
    let waker = Waker::new();

    let mut path = temp_dir();
    path.push(name);

    let p = path.clone();
    let _d = defer(move || remove_file(p).unwrap());

    let open_file = File::config()
        .write()
        .create()
        .truncate()
        .open(sq, path.clone())?;
    let file = waker.block_on(open_file)?;

    let mut expected = Vec::new();
    for buf in bufs {
        expected.extend(&buf);
        let expected_len = buf.len();
        let write = file.write(buf)?;
        let (buf, n) = waker.block_on(write)?;

        assert_eq!(n, expected_len);
        assert_eq!(buf.len(), expected_len);
    }
    drop(file);

    let got = std::fs::read(path)?;
    assert!(got == expected, "file can't be read back");

    Ok(())
}

#[test]
fn sync_all() -> io::Result<()> {
    let sq = test_queue();
    let waker = Waker::new();

    let mut path = temp_dir();
    path.push("sync_all");

    let p = path.clone();
    let _d = defer(move || remove_file(p).unwrap());

    let open_file = File::config()
        .write()
        .create()
        .truncate()
        .open(sq, path.clone())?;
    let file = waker.block_on(open_file)?;

    let write = file.write(b"Hello world".to_vec())?;
    let (buf, n) = waker.block_on(write)?;
    assert_eq!(n, 11);

    waker.block_on(file.sync_all()?)?;
    drop(file);

    let got = std::fs::read(path)?;
    assert!(got == buf, "file can't be read back");

    Ok(())
}

#[test]
fn sync_data() -> io::Result<()> {
    let sq = test_queue();
    let waker = Waker::new();

    let mut path = temp_dir();
    path.push("sync_all");

    let p = path.clone();
    let _d = defer(move || remove_file(p).unwrap());

    let open_file = File::config()
        .write()
        .create()
        .truncate()
        .open(sq, path.clone())?;
    let file = waker.block_on(open_file)?;

    let write = file.write(b"Hello world".to_vec())?;
    let (buf, n) = waker.block_on(write)?;
    assert_eq!(n, 11);

    waker.block_on(file.sync_data()?)?;
    drop(file);

    let got = std::fs::read(path)?;
    assert!(got == buf, "file can't be read back");

    Ok(())
}

/// Waker that blocks the current thread.
struct Waker {
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
    fn new() -> Arc<Waker> {
        Arc::new(Waker {
            handle: thread::current(),
        })
    }

    /// Poll the `future` until completion, blocking when it can't make
    /// progress.
    fn block_on<Fut>(self: &Arc<Waker>, future: Fut) -> Fut::Output
    where
        Fut: Future,
    {
        // Pin the `Future` to stack.
        let mut future = future;
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

fn defer<F: FnOnce()>(f: F) -> Defer<F> {
    Defer { f: Some(f) }
}

struct Defer<F: FnOnce()> {
    f: Option<F>,
}

impl<F: FnOnce()> Drop for Defer<F> {
    fn drop(&mut self) {
        (self.f.take().unwrap())()
    }
}

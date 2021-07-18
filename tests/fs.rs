//! Tests for the filesystem types

#![feature(once_cell)]

use std::lazy::SyncLazy;
use std::task::Poll;
use std::{io, str};

use a10::fs::File;
use a10::Ring;

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

/// Create a [`Ring`] for testing.
fn test_ring(entries: u32) -> io::Result<Ring> {
    static TEST_RING: SyncLazy<Ring> =
        SyncLazy::new(|| Ring::new(1).expect("failed to create test ring"));

    // Attach to a shared ring so that kernel side resources can be reused.
    Ring::config(entries).attach(&TEST_RING).build()
}

#[test]
fn read_one_page() -> io::Result<()> {
    let mut ring = test_ring(1)?;
    test_read(&mut ring, &LOREM_IPSUM_5, LOREM_IPSUM_5.content.len() + 1)
}

#[test]
fn read_multiple_pages_one_read() -> io::Result<()> {
    let mut ring = test_ring(1)?;
    test_read(&mut ring, &LOREM_IPSUM_50, LOREM_IPSUM_50.content.len() + 1)
}

#[test]
fn read_multiple_pages_multiple_reads() -> io::Result<()> {
    // Tests that multiple reads work like expected w.r.t. things like offset
    // advancement.
    let mut ring = test_ring(1)?;
    test_read(&mut ring, &LOREM_IPSUM_50, 4096)
}

#[test]
fn read_multiple_pages_multiple_reads_unaligned() -> io::Result<()> {
    let mut ring = test_ring(1)?;
    test_read(&mut ring, &LOREM_IPSUM_50, 3000)
}

fn test_read(ring: &mut Ring, test_file: &TestFile, buf_size: usize) -> io::Result<()> {
    let path = test_file.path.into();
    let mut open_file = File::open(ring.submission_queue(), path)?;

    ring.poll()?;
    let file = match open_file.check() {
        Poll::Ready(Ok(file)) => file,
        Poll::Ready(Err(err)) => return Err(err),
        Poll::Pending => panic!("opening the file is not done"),
    };

    let mut buf = Vec::with_capacity(buf_size);
    let mut read_bytes = 0;
    loop {
        buf.clear();
        let mut read = file.read(buf)?;

        ring.poll()?;
        buf = match read.check() {
            Poll::Ready(Ok(buf)) => buf,
            Poll::Ready(Err(err)) => return Err(err),
            Poll::Pending => panic!("reading the file is not done"),
        };

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
    let mut ring = test_ring(1)?;
    test_read_at(
        &mut ring,
        &LOREM_IPSUM_5,
        LOREM_IPSUM_5.content.len() + 1,
        100,
    )
}

#[test]
fn read_at_multiple_pages_one_read() -> io::Result<()> {
    let mut ring = test_ring(1)?;
    let offset = 8192;
    let buf_len = LOREM_IPSUM_50.content.len() + 1 - offset as usize;
    test_read_at(&mut ring, &LOREM_IPSUM_50, buf_len, offset)
}

#[test]
fn read_at_multiple_pages_multiple_reads() -> io::Result<()> {
    // Tests that multiple reads work like expected w.r.t. things like offset
    // advancement.
    let mut ring = test_ring(1)?;
    test_read_at(&mut ring, &LOREM_IPSUM_50, 4096, 16384)
}

fn test_read_at(
    ring: &mut Ring,
    test_file: &TestFile,
    buf_size: usize,
    mut offset: u64,
) -> io::Result<()> {
    let path = test_file.path.into();
    let mut open_file = File::open(ring.submission_queue(), path)?;

    ring.poll()?;
    let file = match open_file.check() {
        Poll::Ready(Ok(file)) => file,
        Poll::Ready(Err(err)) => return Err(err),
        Poll::Pending => panic!("opening the file is not done"),
    };

    let mut buf = Vec::with_capacity(buf_size);
    let mut expected = &test_file.content[offset as usize..];
    loop {
        buf.clear();
        let mut read = file.read_at(buf, offset)?;

        ring.poll()?;
        buf = match read.check() {
            Poll::Ready(Ok(buf)) => buf,
            Poll::Ready(Err(err)) => return Err(err),
            Poll::Pending => panic!("reading the file is not done"),
        };

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

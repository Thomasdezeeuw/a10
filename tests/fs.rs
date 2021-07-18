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
    test_read(&mut ring, &LOREM_IPSUM_5)
}

#[test]
fn read_multiple_pages() -> io::Result<()> {
    let mut ring = test_ring(1)?;
    test_read(&mut ring, &LOREM_IPSUM_50)
}

fn test_read(ring: &mut Ring, test_file: &TestFile) -> io::Result<()> {
    let path = test_file.path.into();
    let mut open_file = File::open(ring.submission_queue(), path)?;

    ring.poll()?;
    let file = match open_file.check() {
        Poll::Ready(Ok(file)) => file,
        Poll::Ready(Err(err)) => return Err(err),
        Poll::Pending => panic!("opening the file is not done"),
    };

    let buf = Vec::with_capacity(test_file.content.len() + 1);
    let mut read = file.read(buf)?;

    ring.poll()?;
    let buf = match read.check() {
        Poll::Ready(Ok(file)) => file,
        Poll::Ready(Err(err)) => return Err(err),
        Poll::Pending => panic!("reading the file is not done"),
    };

    assert!(buf == test_file.content, "read content is different");
    Ok(())
}

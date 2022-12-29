//! Tests for the filesystem operations.

#![feature(once_cell)]

use std::env::temp_dir;
use std::fs::remove_file;
use std::path::Path;
use std::time::{Duration, SystemTime};
use std::{io, panic, str};

use a10::fs::OpenOptions;
use a10::{Extract, SubmissionQueue};

mod util;
use util::{defer, test_queue, Waker, PAGE_SIZE};

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

#[test]
fn open_extractor() {
    let sq = test_queue();
    let waker = Waker::new();

    let open_file = OpenOptions::new().open(sq, LOREM_IPSUM_5.path.into());
    // Extract the file path.
    let open_file = open_file.extract();
    let (_, path) = waker.block_on(open_file).unwrap();

    let got: &Path = path.as_ref();
    let expected: &Path = LOREM_IPSUM_5.path.as_ref();
    assert_eq!(got, expected);
}

#[test]
fn read_one_page() {
    let sq = test_queue();
    test_read(sq, &LOREM_IPSUM_5, LOREM_IPSUM_5.content.len() + 1)
}

#[test]
fn read_multiple_pages_one_read() {
    let sq = test_queue();
    test_read(sq, &LOREM_IPSUM_50, LOREM_IPSUM_50.content.len() + 1)
}

#[test]
fn read_multiple_pages_multiple_reads() {
    // Tests that multiple reads work like expected w.r.t. things like offset
    // advancement.
    let sq = test_queue();
    test_read(sq, &LOREM_IPSUM_50, 4096)
}

#[test]
fn read_multiple_pages_multiple_reads_unaligned() {
    let sq = test_queue();
    test_read(sq, &LOREM_IPSUM_50, 3000)
}

fn test_read(sq: SubmissionQueue, test_file: &TestFile, buf_size: usize) {
    let waker = Waker::new();

    let path = test_file.path.into();
    let open_file = OpenOptions::new().open(sq, path);
    let file = waker.block_on(open_file).unwrap();

    let mut buf = Vec::with_capacity(buf_size);
    let mut read_bytes = 0;
    loop {
        buf.clear();
        let read = file.read(buf);
        buf = waker.block_on(read).unwrap();
        if buf.is_empty() {
            panic!("read zero bytes");
        }

        assert!(
            buf == &test_file.content[read_bytes..read_bytes + buf.len()],
            "read content is different"
        );
        read_bytes += buf.len();
        if read_bytes >= test_file.content.len() {
            break;
        }
    }
}

#[test]
fn read_at_one_page() {
    let sq = test_queue();
    test_read_at(sq, &LOREM_IPSUM_5, LOREM_IPSUM_5.content.len() + 1, 100)
}

#[test]
fn read_at_multiple_pages_one_read() {
    let sq = test_queue();
    let offset = 8192;
    let buf_len = LOREM_IPSUM_50.content.len() + 1 - offset as usize;
    test_read_at(sq, &LOREM_IPSUM_50, buf_len, offset)
}

#[test]
fn read_at_multiple_pages_multiple_reads() {
    // Tests that multiple reads work like expected w.r.t. things like offset
    // advancement.
    let sq = test_queue();
    test_read_at(sq, &LOREM_IPSUM_50, 4096, 16384)
}

fn test_read_at(sq: SubmissionQueue, test_file: &TestFile, buf_size: usize, mut offset: u64) {
    let waker = Waker::new();

    let path = test_file.path.into();
    let open_file = OpenOptions::new().open(sq, path);
    let file = waker.block_on(open_file).unwrap();

    let mut buf = Vec::with_capacity(buf_size);
    let mut expected = &test_file.content[offset as usize..];
    loop {
        buf.clear();
        let read = file.read_at(buf, offset);
        buf = waker.block_on(read).unwrap();

        if buf.is_empty() {
            panic!("read zero bytes");
        }

        assert!(buf == &expected[..buf.len()], "read content is different");
        expected = &expected[buf.len()..];
        offset += buf.len() as u64;
        if expected.is_empty() {
            break;
        }
    }
}

#[test]
fn write_hello_world() {
    let sq = test_queue();
    let bufs = vec![b"Hello world".to_vec()];
    test_write("a10.write_hello_world", sq, bufs)
}

#[test]
fn write_one_page() {
    let sq = test_queue();
    let bufs = vec![b"a".repeat(PAGE_SIZE)];
    test_write("a10.write_one_page", sq, bufs)
}

#[test]
fn write_multiple_pages_one_write() {
    let sq = test_queue();
    let bufs = vec![b"b".repeat(4 * PAGE_SIZE)];
    test_write("a10.write_multiple_pages_one_write", sq, bufs)
}

#[test]
fn write_multiple_pages_mulitple_writes() {
    let sq = test_queue();
    let bufs = vec![b"b".repeat(PAGE_SIZE), b"c".repeat(PAGE_SIZE)];
    test_write("a10.write_multiple_pages_mulitple_writes", sq, bufs)
}

#[test]
fn write_multiple_pages_mulitple_writes_unaligned() {
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

fn test_write(name: &str, sq: SubmissionQueue, bufs: Vec<Vec<u8>>) {
    let waker = Waker::new();

    let mut path = temp_dir();
    path.push(name);

    let _d = defer(|| remove_test_file(&path));

    let open_file = OpenOptions::new()
        .write()
        .create()
        .truncate()
        .open(sq, path.clone());
    let file = waker.block_on(open_file).unwrap();

    let mut expected = Vec::new();
    for buf in bufs {
        expected.extend(&buf);
        let expected_len = buf.len();
        let write = file.write(buf);
        let n = waker.block_on(write).unwrap();
        assert_eq!(n, expected_len);
    }
    drop(file);

    let got = std::fs::read(&path).unwrap();
    assert!(got == expected, "file can't be read back");
}

#[test]
fn sync_all() {
    let sq = test_queue();
    let waker = Waker::new();

    let mut path = temp_dir();
    path.push("sync_all");

    let _d = defer(|| remove_test_file(&path));

    let open_file = OpenOptions::new()
        .write()
        .create()
        .truncate()
        .open(sq, path.clone());
    let file = waker.block_on(open_file).unwrap();

    let write = file.write(b"Hello world".to_vec()).extract();
    let (buf, n) = waker.block_on(write).unwrap();
    assert_eq!(n, 11);

    waker.block_on(file.sync_all()).unwrap();
    drop(file);

    let got = std::fs::read(&path).unwrap();
    assert!(got == buf, "file can't be read back");
}

#[test]
fn sync_data() {
    let sq = test_queue();
    let waker = Waker::new();

    let mut path = temp_dir();
    path.push("sync_data");

    let _d = defer(|| remove_test_file(&path));

    let open_file = OpenOptions::new()
        .write()
        .create()
        .truncate()
        .open(sq, path.clone());
    let file = waker.block_on(open_file).unwrap();

    let write = file.write(b"Hello world".to_vec()).extract();
    let (buf, n) = waker.block_on(write).unwrap();
    assert_eq!(n, 11);

    waker.block_on(file.sync_data()).unwrap();
    drop(file);

    let got = std::fs::read(&path).unwrap();
    assert!(got == buf, "file can't be read back");
}

#[test]
fn metadata_small() {
    let created = SystemTime::UNIX_EPOCH + Duration::new(1664033209, 759488874);
    test_metadata(&LOREM_IPSUM_5, created)
}

#[test]
fn metadata_big() {
    let created = SystemTime::UNIX_EPOCH + Duration::new(1664033209, 759488874);
    test_metadata(&LOREM_IPSUM_50, created)
}

fn test_metadata(test_file: &TestFile, created: SystemTime) {
    let sq = test_queue();
    let waker = Waker::new();

    let open_file = OpenOptions::new().open(sq, test_file.path.into());
    let file = waker.block_on(open_file).unwrap();

    let metadata = waker.block_on(file.metadata()).unwrap();
    assert!(metadata.file_type().is_file());
    assert!(metadata.is_file());
    assert!(!metadata.is_dir());
    assert!(!metadata.is_symlink());
    assert_eq!(metadata.len(), test_file.content.len() as u64);
    let permissions = metadata.permissions();
    assert!(permissions.owner_can_read());
    assert!(permissions.owner_can_write());
    assert!(!permissions.owner_can_execute());
    assert!(permissions.group_can_read());
    assert!(!permissions.group_can_write());
    assert!(!permissions.group_can_execute());
    assert!(permissions.others_can_read());
    assert!(!permissions.others_can_write());
    assert!(!permissions.others_can_execute());
    // Can never get `accessed` right and `modified` is too much of a moving
    // target.
    assert_eq!(metadata.created(), created);
}

fn remove_test_file(path: &Path) {
    match remove_file(path) {
        Ok(()) => {}
        Err(ref err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => panic!("unexpected error removing test file: {err}"),
    }
}

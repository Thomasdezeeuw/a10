//! Tests for the filesystem operations.

use std::env::temp_dir;
use std::path::Path;
use std::{panic, str};

use a10::fs::{self, OpenOptions};
use a10::io::{Read, ReadVectored, Write, WriteVectored};
use a10::{Extract, SubmissionQueue};

use crate::util::{
    defer, is_send, is_sync, remove_test_file, test_queue, TestFile, Waker, LOREM_IPSUM_5,
    LOREM_IPSUM_50, PAGE_SIZE,
};

#[test]
fn open_extractor() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<OpenOptions>();
    is_sync::<OpenOptions>();

    let open_file = OpenOptions::new().open(sq, LOREM_IPSUM_5.path.into());
    // Extract the file path.
    let open_file = open_file.extract();
    let (_, path) = waker.block_on(open_file).unwrap();

    let got: &Path = path.as_ref();
    let expected: &Path = LOREM_IPSUM_5.path.as_ref();
    assert_eq!(got, expected);
}

#[test]
fn create_temp_file() {
    let sq = test_queue();
    let waker = Waker::new();

    let open_file = OpenOptions::new().write().open_temp_file(sq, "/tmp".into());
    let file = waker.block_on(open_file).expect("failed to open temp file");

    let expected = LOREM_IPSUM_5.content;
    let n = waker
        .block_on(file.write(LOREM_IPSUM_5.content))
        .expect("failed to write");
    assert_eq!(n, expected.len());

    let buf = Vec::with_capacity(expected.len());
    let buf = waker
        .block_on(file.read_at(buf, 0))
        .expect("failed to read");
    assert!(buf == expected, "read content is different");
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

    is_send::<Read<Vec<u8>>>();
    is_sync::<Read<Vec<u8>>>();

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
fn read_vectored_array() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<ReadVectored<Vec<u8>, 2>>();
    is_sync::<ReadVectored<Vec<u8>, 1>>();

    let test_file = &LOREM_IPSUM_50;
    let path = test_file.path.into();
    let open_file = OpenOptions::new().open(sq, path);
    let file = waker.block_on(open_file).unwrap();

    let bufs = [
        Vec::with_capacity(4096),
        Vec::with_capacity(32),
        Vec::with_capacity(4096 - 32),
    ];
    let read = file.read_vectored(bufs);
    let bufs = waker.block_on(read).unwrap();
    drop(file);

    let mut start = 0;
    for buf in bufs.iter() {
        assert!(
            buf == &test_file.content[start..start + buf.len()],
            "read content is different"
        );
        start += buf.len();
    }
    assert_eq!(start, 2 * 4096);
}

#[test]
fn read_vectored_tuple() {
    let sq = test_queue();
    let waker = Waker::new();

    let test_file = &LOREM_IPSUM_50;
    let path = test_file.path.into();
    let open_file = OpenOptions::new().open(sq, path);
    let file = waker.block_on(open_file).unwrap();

    let bufs = (
        Vec::with_capacity(4096),
        Vec::with_capacity(32),
        Vec::with_capacity(4096 - 32),
    );
    let read = file.read_vectored(bufs);
    let bufs = waker.block_on(read).unwrap();
    drop(file);

    let mut start = 0;
    for buf in [bufs.0, bufs.1, bufs.2] {
        assert!(
            buf == &test_file.content[start..start + buf.len()],
            "read content is different"
        );
        start += buf.len();
    }
    assert_eq!(start, 2 * 4096);
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

    is_send::<Write<Vec<u8>>>();
    is_sync::<Write<Vec<u8>>>();

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
fn write_vectored() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<WriteVectored<Vec<u8>, 1>>();
    is_sync::<WriteVectored<Vec<u8>, 1>>();

    let mut path = temp_dir();
    path.push("write_vectored");
    let _d = defer(|| remove_test_file(&path));

    let open_file = OpenOptions::new()
        .write()
        .create()
        .truncate()
        .open(sq, path.clone());
    let file = waker.block_on(open_file).unwrap();

    let bufs = ["hello", ", ", "world!"];
    let write = file.write_vectored(bufs);
    let n = waker.block_on(write).unwrap();
    assert_eq!(n, 13);
    drop(file);

    let got = std::fs::read(&path).unwrap();
    assert!(got == b"hello, world!", "file can't be read back");
}

#[test]
fn write_vectored_extractor() {
    let sq = test_queue();
    let waker = Waker::new();

    let mut path = temp_dir();
    path.push("write_vectored_extractor");
    let _d = defer(|| remove_test_file(&path));

    let open_file = OpenOptions::new()
        .write()
        .create()
        .truncate()
        .open(sq, path.clone());
    let file = waker.block_on(open_file).unwrap();

    let bufs = ("hello", vec![b',', b' '], b"world!".as_slice());
    let write = file.write_vectored(bufs).extract();
    let (bufs, n) = waker.block_on(write).unwrap();
    assert_eq!(n, 13);
    assert_eq!(bufs.0, "hello");
    assert_eq!(bufs.1, b", ");
    assert_eq!(bufs.2, b"world!");
    drop(file);

    let got = std::fs::read(&path).unwrap();
    assert!(got == b"hello, world!", "file can't be read back");
}

#[test]
fn sync_all() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<fs::SyncData>();
    is_sync::<fs::SyncData>();

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
    test_metadata(&LOREM_IPSUM_5)
}

#[test]
fn metadata_big() {
    test_metadata(&LOREM_IPSUM_50)
}

fn test_metadata(test_file: &TestFile) {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<fs::Stat>();
    is_sync::<fs::Stat>();

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
    // NOTE: we don't check the access, modification or creation timestamp are
    // those re to different between test runs.
}

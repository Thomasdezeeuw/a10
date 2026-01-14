use std::env::temp_dir;
use std::path::Path;
#[cfg(any(
    target_os = "android",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "linux",
    target_os = "netbsd"
))]
use std::path::PathBuf;
#[cfg(any(target_os = "android", target_os = "linux"))]
use std::time::SystemTime;
use std::{panic, str};

#[cfg(any(target_os = "android", target_os = "linux"))]
use a10::fd;
#[cfg(any(target_os = "android", target_os = "linux"))]
use a10::fs::MetadataInterest;
use a10::fs::{self, CreateDir, Delete, Open, OpenOptions, Rename, Truncate};
#[cfg(any(
    target_os = "android",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "linux",
    target_os = "netbsd",
))]
use a10::fs::{Advise, AdviseFlag, Allocate, AllocateFlag};
use a10::io::{Read, ReadVectored, Write, WriteVectored};
use a10::{Extract, SubmissionQueue};

use crate::util::{
    LOREM_IPSUM_5, LOREM_IPSUM_50, TestFile, Waker, defer, is_send, is_sync, page_size,
    remove_test_dir, remove_test_file, require_kernel, test_queue,
};

const DATA: &[u8] = b"Hello, World";

#[test]
fn open_extractor() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<OpenOptions>();
    is_sync::<OpenOptions>();

    let open_file: Open = OpenOptions::new().open(sq, LOREM_IPSUM_5.path.into());

    // Extract the file path.
    let open_file = open_file.extract();
    let (_, path) = waker.block_on(open_file).unwrap();

    let got: &Path = path.as_ref();
    let expected: &Path = LOREM_IPSUM_5.path.as_ref();
    assert_eq!(got, expected);
}

#[test]
#[cfg(any(
    target_os = "android",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "linux",
    target_os = "netbsd"
))]
fn open_direct_io() {
    let sq = test_queue();
    let waker = Waker::new();

    // NOTE: we can't use the temp directory as tmpfs (as of v6.1) doesn't
    // support O_DIRECT.
    let path = PathBuf::from("target/tmp_test.open_direct.txt");
    let _d = defer(|| remove_test_file(&path));

    let open_file: Open = OpenOptions::new()
        .read()
        .write()
        .create_new()
        .data_sync()
        .direct()
        .open(sq, path.clone());
    let file = waker.block_on(open_file).expect("failed to open file");

    // Using O_DIRECT has various alignment requirements for writing.
    // For the purpose of this test we'll assume that the page size is
    // sufficient, but in practice this might not be the case.
    let page_size = page_size();

    // Determine the amount of bytes we need to skip for we get a page aligned
    // buffer.
    let offset = LOREM_IPSUM_50.content.as_ptr().align_offset(page_size);
    assert!(offset < page_size);
    let content = &LOREM_IPSUM_50.content[offset..offset + (2 * page_size)];

    waker
        .block_on(file.write_all(content))
        .expect("failed to write file");
    waker.block_on(file.close()).expect("failed to close file");

    let got = std::fs::read(&path).expect("failed to read file");
    assert_eq!(got, content);
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn open_direct_fd() {
    let sq = test_queue();
    let waker = Waker::new();

    let open_file: Open = OpenOptions::new()
        .kind(fd::Kind::Direct)
        .open(sq, LOREM_IPSUM_5.path.into());

    // Extract the file path.
    let open_file = open_file.extract();
    let (_, path) = waker.block_on(open_file).unwrap();

    let got: &Path = path.as_ref();
    let expected: &Path = LOREM_IPSUM_5.path.as_ref();
    assert_eq!(got, expected);
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn create_temp_file() {
    let sq = test_queue();
    let waker = Waker::new();

    let open_file: Open = OpenOptions::new().write().open_temp_file(sq, "/tmp".into());
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
    let open_file: Open = OpenOptions::new().open(sq, path);
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
    let open_file: Open = OpenOptions::new().open(sq, path);
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

    is_send::<ReadVectored<[Vec<u8>; 2], 2>>();
    is_sync::<ReadVectored<[Vec<u8>; 2], 2>>();

    let test_file = &LOREM_IPSUM_50;
    let path = test_file.path.into();
    let open_file: Open = OpenOptions::new().open(sq, path);
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
    let open_file: Open = OpenOptions::new().open(sq, path);
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
    let bufs = vec![b"a".repeat(page_size())];
    test_write("a10.write_one_page", sq, bufs)
}

#[test]
fn write_multiple_pages_one_write() {
    let sq = test_queue();
    let bufs = vec![b"b".repeat(4 * page_size())];
    test_write("a10.write_multiple_pages_one_write", sq, bufs)
}

#[test]
fn write_multiple_pages_mulitple_writes() {
    let sq = test_queue();
    let bufs = vec![b"b".repeat(page_size()), b"c".repeat(page_size())];
    test_write("a10.write_multiple_pages_mulitple_writes", sq, bufs)
}

#[test]
fn write_multiple_pages_mulitple_writes_unaligned() {
    let sq = test_queue();
    let bufs = vec![
        b"Hello unalignment!".to_vec(),
        b"b".repeat(page_size()),
        b"c".repeat(page_size()),
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

    let open_file: Open = OpenOptions::new()
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

    is_send::<WriteVectored<[Vec<u8>; 2], 2>>();
    is_sync::<WriteVectored<[Vec<u8>; 2], 2>>();

    let mut path = temp_dir();
    path.push("write_vectored");
    let _d = defer(|| remove_test_file(&path));

    let open_file: Open = OpenOptions::new()
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

    let open_file: Open = OpenOptions::new()
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

    let open_file: Open = OpenOptions::new()
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

    let open_file: Open = OpenOptions::new()
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

    let open_file: Open = OpenOptions::new().open(sq, test_file.path.into());
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
    // This fails on certain setups
    //assert!(!permissions.group_can_write());
    assert!(!permissions.group_can_execute());
    assert!(permissions.others_can_read());
    assert!(!permissions.others_can_write());
    assert!(!permissions.others_can_execute());
    // NOTE: we don't check the access, modification or creation timestamp are
    // those re to different between test runs.
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn metadata_select_fields() {
    let sq = test_queue();
    let waker = Waker::new();
    let test_file = &LOREM_IPSUM_5;

    let open_file = fs::open_file(sq, test_file.path.into());
    let file = waker.block_on(open_file).unwrap();

    let metadata = file.metadata().only(MetadataInterest::TYPE);
    let metadata = waker.block_on(metadata).unwrap();

    assert!(metadata.file_type().is_file());
    assert!(metadata.is_file());
    assert!(!metadata.is_dir());
    assert!(!metadata.is_symlink());
    // Random field that should not be field (but is unlikely to be actually
    // zero).
    assert_eq!(metadata.created(), SystemTime::UNIX_EPOCH);
}

#[test]
#[cfg(any(
    target_os = "android",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "linux",
    target_os = "netbsd",
))]
fn fadvise() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<Advise>();
    is_sync::<Advise>();

    let test_file = &LOREM_IPSUM_50;

    let open_file: Open = OpenOptions::new().open(sq, test_file.path.into());
    let file = waker.block_on(open_file).unwrap();

    waker
        .block_on(file.advise(0, 0, AdviseFlag::SEQUENTIAL))
        .expect("failed fadvise");

    let buf = Vec::with_capacity(test_file.content.len() + 1);
    let buf = waker
        .block_on(file.read_n(buf, test_file.content.len()))
        .unwrap();
    assert!(buf == test_file.content, "read content is different");
}

#[test]
fn ftruncate() {
    require_kernel!(6, 9);

    let sq = test_queue();
    let waker = Waker::new();

    is_send::<Truncate>();
    is_sync::<Truncate>();

    let mut path = temp_dir();
    path.push("ftruncate");
    let _d = defer(|| remove_test_file(&path));

    let open_file: Open = OpenOptions::new()
        .write()
        .create()
        .truncate()
        .open(sq, path.clone());
    let file = waker.block_on(open_file).unwrap();

    waker
        .block_on(file.write_all(LOREM_IPSUM_50.content))
        .unwrap();

    let metadata = waker.block_on(file.metadata()).unwrap();
    assert_eq!(metadata.len(), LOREM_IPSUM_50.content.len() as u64);

    const SIZE: u64 = 4096;

    waker
        .block_on(file.truncate(SIZE))
        .expect("failed truncate");

    let metadata = waker.block_on(file.metadata()).unwrap();
    assert_eq!(metadata.len(), SIZE);
}

#[test]
#[cfg(any(
    target_os = "android",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "linux",
    target_os = "netbsd",
))]
fn fallocate() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<Allocate>();
    is_sync::<Allocate>();

    let mut path = temp_dir();
    path.push("fallocate");
    let _d = defer(|| remove_test_file(&path));

    let open_file: Open = OpenOptions::new()
        .write()
        .create()
        .truncate()
        .open(sq, path.clone());
    let file = waker.block_on(open_file).unwrap();

    waker
        .block_on(file.allocate(0, 4096, Some(AllocateFlag::KEEP_SIZE)))
        .expect("failed fallocate");

    let write = file.write(b"Hello world".to_vec()).extract();
    let (buf, n) = waker.block_on(write).unwrap();
    assert_eq!(n, 11);

    waker.block_on(file.sync_data()).unwrap();
    drop(file);

    let got = std::fs::read(&path).unwrap();
    assert!(got == buf, "file can't be read back");
}

#[test]
fn rename() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<Rename>();
    is_sync::<Rename>();

    let path = temp_dir();
    let mut from = path.clone();
    from.push("rename.1");
    let mut to = path.clone();
    to.push("rename.2");
    let _d = defer(|| remove_test_file(&from));
    let _d = defer(|| remove_test_file(&to));

    std::fs::write(&from, DATA).expect("failed to write file");

    waker
        .block_on(fs::rename(sq, from.clone(), to.clone()))
        .expect("failed to rename file");

    let got = std::fs::read(&to).expect("failed to read file");
    assert!(got == DATA, "file can't be read back");
}

#[test]
fn rename_extract() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<Rename>();
    is_sync::<Rename>();

    let path = temp_dir();
    let mut from = path.clone();
    from.push("rename_extract.1");
    let mut to = path.clone();
    to.push("rename_extract.2");
    let _d = defer(|| remove_test_file(&from));
    let _d = defer(|| remove_test_file(&to));

    std::fs::write(&from, DATA).expect("failed to write file");

    let (got_from, got_to) = waker
        .block_on(fs::rename(sq, from.clone(), to.clone()).extract())
        .expect("failed to rename file");
    assert_eq!(got_from, from);
    assert_eq!(got_to, to);

    let got = std::fs::read(&to).expect("failed to read file");
    assert!(got == DATA, "file can't be read back");
}

#[test]
fn create_dir() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<CreateDir>();
    is_sync::<CreateDir>();

    let mut path = temp_dir();
    path.push("create_dir");
    let _d = defer(|| remove_test_dir(&path));

    waker
        .block_on(fs::create_dir(sq, path.clone()))
        .expect("failed to create dir");

    let entries = std::fs::read_dir(&path).expect("failed to read created dir");
    for entry in entries {
        entry.expect("failed to read directory entry");
    }
}

#[test]
fn create_dir_extract() {
    let sq = test_queue();
    let waker = Waker::new();

    let mut path = temp_dir();
    path.push("create_dir_extract");
    let _d = defer(|| remove_test_dir(&path));

    let got = waker
        .block_on(fs::create_dir(sq, path.clone()).extract())
        .expect("failed to create dir");
    assert_eq!(got, path);

    let entries = std::fs::read_dir(&path).expect("failed to read created dir");
    for entry in entries {
        entry.expect("failed to read directory entry");
    }
}

#[test]
fn remove_file() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<Delete>();
    is_sync::<Delete>();

    let mut path = temp_dir();
    path.push("remove_file");
    let _d = defer(|| remove_test_file(&path));

    std::fs::write(&path, DATA).expect("failed to create test file");

    waker
        .block_on(fs::remove_file(sq, path.clone()))
        .expect("failed to remove file");
}

#[test]
fn remove_file_extract() {
    let sq = test_queue();
    let waker = Waker::new();

    let mut path = temp_dir();
    path.push("remove_file_extract");
    let _d = defer(|| remove_test_file(&path));

    std::fs::write(&path, DATA).expect("failed to create test file");

    let got = waker
        .block_on(fs::remove_file(sq, path.clone()).extract())
        .expect("failed to remove file");
    assert_eq!(got, path);
}

#[test]
fn remove_dir() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<Delete>();
    is_sync::<Delete>();

    let mut path = temp_dir();
    path.push("remove_dir");
    let _d = defer(|| remove_test_dir(&path));

    std::fs::create_dir(&path).expect("failed to create test dir");

    waker
        .block_on(fs::remove_dir(sq, path.clone()))
        .expect("failed to remove dir");
}

#[test]
fn remove_dir_extract() {
    let sq = test_queue();
    let waker = Waker::new();

    let mut path = temp_dir();
    path.push("remove_dir_extract");
    let _d = defer(|| remove_test_dir(&path));

    std::fs::create_dir(&path).expect("failed to create test dir");

    let got = waker
        .block_on(fs::remove_dir(sq, path.clone()).extract())
        .expect("failed to remove dir");
    assert_eq!(got, path);
}

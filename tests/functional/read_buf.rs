use std::ops::Bound;
use std::panic::{self, AssertUnwindSafe};

use a10::Ring;
use a10::fs::OpenOptions;
use a10::io::{BufMut, ReadBuf, ReadBufPool};

use crate::util::{LOREM_IPSUM_50, block_on, init, is_send, is_sync, start_op};

const BUF_SIZE: usize = 4096;

#[test]
fn read_buf_pool_size_assertion() {
    assert_eq!(std::mem::size_of::<ReadBufPool>(), 8);
}

#[test]
fn read_buf_size_assertion() {
    assert_eq!(std::mem::size_of::<ReadBuf>(), 24);
}

#[test]
fn read_buf_pool_is_send_and_sync() {
    is_send::<ReadBufPool>();
    is_sync::<ReadBufPool>();
}

#[test]
fn read_buf_is_send_and_sync() {
    is_send::<ReadBuf>();
    is_sync::<ReadBuf>();
}

#[test]
fn read_read_buf_pool() {
    init();

    let mut ring = Ring::new().expect("failed to create test ring");
    let sq = ring.sq().clone();

    let test_file = &LOREM_IPSUM_50;
    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    let path = test_file.path.into();
    let file = block_on(&mut ring, OpenOptions::new().open(sq, path)).unwrap();

    let buf = block_on(&mut ring, file.read(buf_pool.get()).from(0)).unwrap();
    assert_eq!(buf.len(), BUF_SIZE);
    assert!(!buf.is_empty());
    assert!(
        &*buf == &test_file.content[..buf.len()],
        "read content is different"
    );
}

#[test]
fn read_buf() {
    init();

    let mut ring = Ring::new().expect("failed to create test ring");
    let sq = ring.sq().clone();

    let test_file = &LOREM_IPSUM_50;
    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    let path = test_file.path.into();
    let file = block_on(&mut ring, OpenOptions::new().open(sq, path)).unwrap();

    let mut buf = block_on(&mut ring, file.read(buf_pool.get()).from(0)).unwrap();
    assert_eq!(buf.len(), BUF_SIZE);
    assert!(!buf.is_empty());
    assert!(
        &*buf == &test_file.content[..buf.len()],
        "read content is different"
    );
    assert_eq!(buf.spare_capacity(), 0);
    assert!(!buf.has_spare_capacity());

    buf.truncate(1024);
    assert_eq!(buf.len(), 1024);
    assert!(!buf.is_empty());
    assert!(
        &*buf == &test_file.content[..buf.len()],
        "read content is different"
    );
    assert_eq!(buf.spare_capacity(), BUF_SIZE as u32 - 1024);
    assert!(buf.has_spare_capacity());

    unsafe { buf.set_len(512) };
    assert_eq!(buf.len(), 512);
    assert!(!buf.is_empty());
    assert!(
        &*buf == &test_file.content[..buf.len()],
        "read content is different"
    );
    assert_eq!(buf.spare_capacity(), BUF_SIZE as u32 - 512);
    assert!(buf.has_spare_capacity());
    unsafe { buf.set_len(1024) };
    assert_eq!(buf.len(), 1024);
    assert!(!buf.is_empty());
    assert!(
        &*buf == &test_file.content[..buf.len()],
        "read content is different"
    );
    assert_eq!(buf.spare_capacity(), BUF_SIZE as u32 - 1024);
    assert!(buf.has_spare_capacity());

    buf.clear();
    assert_eq!(buf.len(), 0);
    assert!(buf.is_empty());
    assert_eq!(buf.spare_capacity(), BUF_SIZE as u32);
    assert!(buf.has_spare_capacity());

    const DATA1: &[u8] = b"hello world";
    buf.extend_from_slice(DATA1).unwrap();
    assert_eq!(buf.len(), DATA1.len());
    assert!(!buf.is_empty());
    assert_eq!(&*buf, DATA1);

    buf.extend_from_slice(DATA1).unwrap();
    assert_eq!(buf.len(), 2 * DATA1.len());
    assert!(!buf.is_empty());
    assert_eq!(&buf[0..DATA1.len()], DATA1);
    assert_eq!(&buf[DATA1.len()..2 * DATA1.len()], DATA1);

    let rest = buf.spare_capacity_mut();
    assert_eq!(rest.len(), BUF_SIZE - (2 * DATA1.len()));
    rest[0].write(b'!');
    rest[1].write(b'!');
    unsafe { buf.set_len(buf.len() + 2) };
    assert_eq!(&buf[2 * DATA1.len()..], b"!!");
}

#[test]
fn read_read_buf_pool_multiple_buffers() {
    init();

    let mut ring = Ring::new().expect("failed to create test ring");
    let sq = ring.sq().clone();

    let test_file = &LOREM_IPSUM_50;
    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    let path = test_file.path.into();
    let file = block_on(&mut ring, OpenOptions::new().open(sq, path)).unwrap();

    let mut read1 = file.read(buf_pool.get()).from(0);
    let mut read2 = file.read(buf_pool.get()).from(0);
    let mut read3 = file.read(buf_pool.get()).from(0);
    start_op(&mut read1);
    start_op(&mut read2);
    start_op(&mut read3);
    let buf1 = block_on(&mut ring, read1).unwrap();
    let buf2 = block_on(&mut ring, read2).unwrap();

    for buf in [buf1, buf2] {
        assert_eq!(buf.len(), BUF_SIZE);
        assert!(
            &*buf == &test_file.content[..buf.len()],
            "read content is different"
        );
    }
}

#[test]
fn read_read_buf_pool_reuse_buffers() {
    init();

    let mut ring = Ring::new().expect("failed to create test ring");
    let sq = ring.sq().clone();

    let test_file = &LOREM_IPSUM_50;
    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    let path = test_file.path.into();
    let file = block_on(&mut ring, OpenOptions::new().open(sq, path)).unwrap();

    for _ in 0..4 {
        let buf = block_on(&mut ring, file.read(buf_pool.get()).from(0)).unwrap();
        assert_eq!(buf.len(), BUF_SIZE);
        assert!(
            &*buf == &test_file.content[..buf.len()],
            "read content is different"
        );
    }
}

#[test]
fn read_read_buf_pool_reuse_same_buffer() {
    init();

    let mut ring = Ring::new().expect("failed to create test ring");
    let sq = ring.sq().clone();

    let test_file = &LOREM_IPSUM_50;
    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    let path = test_file.path.into();
    let file = block_on(&mut ring, OpenOptions::new().open(sq, path)).unwrap();

    let mut buf = block_on(&mut ring, file.read(buf_pool.get()).from(0)).unwrap();
    assert_eq!(buf.len(), BUF_SIZE);
    assert_eq!(&*buf, &test_file.content[..buf.len()]);

    // When reusing the buffer it shouldn't overwrite the existing data.
    buf.truncate(100);
    let mut buf = block_on(&mut ring, file.read(buf).from(0)).unwrap();
    assert_eq!(buf.len(), BUF_SIZE);
    assert_eq!(&buf[0..100], &test_file.content[0..100]);
    assert_eq!(&buf[100..], &test_file.content[0..BUF_SIZE - 100]);

    // After releasing the buffer to the pool it should "overwrite" everything
    // again.
    buf.release();
    let buf = block_on(&mut ring, file.read(buf).from(0)).unwrap();
    assert_eq!(buf.len(), BUF_SIZE);
    assert_eq!(&*buf, &test_file.content[..buf.len()]);
}

#[test]
fn read_read_buf_pool_out_of_buffers() {
    init();

    let mut ring = Ring::new().expect("failed to create test ring");
    let sq = ring.sq().clone();

    let test_file = &LOREM_IPSUM_50;
    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    let path = test_file.path.into();
    let file = block_on(&mut ring, OpenOptions::new().open(sq, path)).unwrap();

    let futures = (0..8)
        .map(|_| {
            let mut read = file.read(buf_pool.get()).from(0);
            start_op(&mut read);
            read
        })
        .collect::<Vec<_>>();

    for future in futures {
        let buf = match block_on(&mut ring, future) {
            Ok(buf) => buf,
            Err(err) => {
                if let Some(libc::ENOBUFS) = err.raw_os_error() {
                    continue;
                }
                panic!("unexpected {err}");
            }
        };
        assert_eq!(buf.len(), BUF_SIZE);
        assert!(
            &*buf == &test_file.content[..buf.len()],
            "read content is different"
        );
    }
}

#[test]
fn two_read_buf_pools() {
    init();

    let mut ring = Ring::new().expect("failed to create test ring");
    let sq = ring.sq().clone();
    let test_file = &LOREM_IPSUM_50;

    let buf_pool1 = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();
    let buf_pool2 = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    let path = test_file.path.into();
    let file = block_on(&mut ring, OpenOptions::new().open(sq, path)).unwrap();

    for buf_pool in [buf_pool1, buf_pool2] {
        let buf = block_on(&mut ring, file.read(buf_pool.get()).from(0)).unwrap();
        assert_eq!(buf.len(), BUF_SIZE);
        assert!(
            &*buf == &test_file.content[..buf.len()],
            "read content is different"
        );
    }
}

#[test]
fn read_buf_remove() {
    const BUF_SIZE: usize = 64;

    init();

    let mut ring = Ring::new().expect("failed to create test ring");
    let sq = ring.sq().clone();

    let buf_pool = ReadBufPool::new(sq.clone(), 1, BUF_SIZE as u32).unwrap();

    let path = LOREM_IPSUM_50.path.into();
    let file = block_on(&mut ring, OpenOptions::new().open(sq, path)).unwrap();

    let mut buf = block_on(&mut ring, file.read(buf_pool.get())).unwrap();
    assert!(!buf.is_empty());

    let tests = &[
        (
            // RangeToInclusive, `..=end`.
            Bound::Unbounded,
            Bound::Included(5),
        ),
        (
            // RangeTo, `..end`.
            Bound::Unbounded,
            Bound::Excluded(5),
        ),
        (
            // RangeFull, `..`.
            Bound::Unbounded,
            Bound::Unbounded,
        ),
        (
            // RangeFrom, `start..`.
            Bound::Included(1),
            Bound::Unbounded,
        ),
        (
            // RangeInclusive, `start..=end`.
            Bound::Included(1),
            Bound::Included(4),
        ),
        (
            // Range, `start..end`.
            Bound::Included(1),
            Bound::Excluded(5),
        ),
        // The following are unused.
        (Bound::Excluded(1), Bound::Unbounded),
        (Bound::Excluded(1), Bound::Included(5)),
        (Bound::Excluded(1), Bound::Excluded(5)),
    ];

    buf.clear();
    const DATA: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
    buf.extend_from_slice(&[255; 11]).unwrap(); // Detect errors more easily.
    for (lower, upper) in tests.iter().copied() {
        // We'll use the `Vec::drain` implementation as expected value as it's
        // the API we're trying to match.
        let mut expected = Vec::from(DATA);
        expected.drain((lower, upper));

        buf.clear();
        buf.extend_from_slice(DATA).unwrap();
        buf.remove((lower, upper));

        assert_eq!(
            buf.as_slice(),
            expected,
            "lower: {lower:?}, upper: {upper:?}"
        );
    }
}

#[test]
fn read_buf_remove_invalid_range() {
    const BUF_SIZE: usize = 64;

    init();

    let mut ring = Ring::new().expect("failed to create test ring");
    let sq = ring.sq().clone();

    let buf_pool = ReadBufPool::new(sq.clone(), 1, BUF_SIZE as u32).unwrap();

    let path = LOREM_IPSUM_50.path.into();
    let file = block_on(&mut ring, OpenOptions::new().open(sq, path)).unwrap();

    let mut buf = block_on(&mut ring, file.read(buf_pool.get())).unwrap();
    assert!(!buf.is_empty());

    buf.truncate(10);
    let tests = &[
        (Bound::Unbounded, Bound::Included(20)),
        (Bound::Unbounded, Bound::Excluded(20)),
        (Bound::Included(20), Bound::Unbounded),
        (Bound::Excluded(20), Bound::Unbounded),
    ];

    for (lower, upper) in tests.iter().copied() {
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            buf.remove((lower, upper));
        }));
        let _ = result.unwrap_err();
    }
}

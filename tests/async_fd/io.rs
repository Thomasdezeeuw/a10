//! Tests for the I/O operations.

use std::cell::Cell;
use std::env::temp_dir;
use std::io;
use std::ops::Bound;
use std::os::fd::{AsFd, AsRawFd, RawFd};
use std::panic::{self, AssertUnwindSafe};
use std::pin::Pin;

use a10::fs::OpenOptions;
use a10::io::{
    stderr, stdout, Buf, BufMut, BufMutSlice, BufSlice, Close, ReadBuf, ReadBufPool, Splice,
    Stderr, Stdout,
};
use a10::{AsyncFd, Extract, Ring, SubmissionQueue};

use crate::util::{
    bind_and_listen_ipv4, block_on, defer, expect_io_errno, init, is_send, is_sync, poll_nop,
    remove_test_file, require_kernel, tcp_ipv4_socket, test_queue, Waker, LOREM_IPSUM_5,
    LOREM_IPSUM_50,
};

const BUF_SIZE: usize = 4096;
const NO_OFFSET: u64 = u64::MAX;

#[test]
fn try_clone() {
    let sq = test_queue();
    let waker = Waker::new();

    let (r, w) = pipe2(sq).expect("failed to create pipe");
    let w2 = w.try_clone().expect("failed to clone fd");

    const DATA: &[u8] = b"hello world";
    waker.block_on(w.write(&DATA[..5])).unwrap();
    waker.block_on(w2.write(&DATA[5..])).unwrap();

    let buf = BadReadBuf {
        data: Vec::with_capacity(30),
    };
    let buf = waker.block_on(r.read_n(buf, DATA.len())).unwrap();
    assert_eq!(&buf.data, DATA);
}

#[test]
fn read_read_buf_pool() {
    require_kernel!(5, 19);
    init();

    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();

    is_send::<ReadBufPool>();
    is_sync::<ReadBufPool>();
    is_send::<ReadBuf>();
    is_sync::<ReadBuf>();

    let test_file = &LOREM_IPSUM_50;
    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    let path = test_file.path.into();
    let file = block_on(&mut ring, OpenOptions::new().open(sq, path)).unwrap();

    let buf = block_on(&mut ring, file.read_at(buf_pool.get(), 0)).unwrap();
    assert_eq!(buf.len(), BUF_SIZE);
    assert!(!buf.is_empty());
    assert!(
        &*buf == &test_file.content[..buf.len()],
        "read content is different"
    );
}

#[test]
fn read_buf() {
    require_kernel!(5, 19);
    init();

    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();

    let test_file = &LOREM_IPSUM_50;
    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    let path = test_file.path.into();
    let file = block_on(&mut ring, OpenOptions::new().open(sq, path)).unwrap();

    let mut buf = block_on(&mut ring, file.read_at(buf_pool.get(), 0)).unwrap();
    assert_eq!(buf.len(), BUF_SIZE);
    assert!(!buf.is_empty());
    assert!(
        &*buf == &test_file.content[..buf.len()],
        "read content is different"
    );

    buf.truncate(1024);
    assert_eq!(buf.len(), 1024);
    assert!(!buf.is_empty());
    assert!(
        &*buf == &test_file.content[..buf.len()],
        "read content is different"
    );

    unsafe { buf.set_len(512) };
    assert_eq!(buf.len(), 512);
    assert!(!buf.is_empty());
    assert!(
        &*buf == &test_file.content[..buf.len()],
        "read content is different"
    );
    unsafe { buf.set_len(1024) };
    assert_eq!(buf.len(), 1024);
    assert!(!buf.is_empty());
    assert!(
        &*buf == &test_file.content[..buf.len()],
        "read content is different"
    );

    buf.clear();
    assert_eq!(buf.len(), 0);
    assert!(buf.is_empty());

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
    require_kernel!(5, 19);
    init();

    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();

    let test_file = &LOREM_IPSUM_50;
    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    let path = test_file.path.into();
    let file = block_on(&mut ring, OpenOptions::new().open(sq, path)).unwrap();

    let mut read1 = file.read_at(buf_pool.get(), 0);
    let _ = poll_nop(Pin::new(&mut read1));
    let mut read2 = file.read_at(buf_pool.get(), 0);
    let _ = poll_nop(Pin::new(&mut read2));
    let mut read3 = file.read_at(buf_pool.get(), 0);
    let _ = poll_nop(Pin::new(&mut read3));
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
    require_kernel!(5, 19);
    init();

    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();

    let test_file = &LOREM_IPSUM_50;
    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    let path = test_file.path.into();
    let file = block_on(&mut ring, OpenOptions::new().open(sq, path)).unwrap();

    for _ in 0..4 {
        let buf = block_on(&mut ring, file.read_at(buf_pool.get(), 0)).unwrap();
        assert_eq!(buf.len(), BUF_SIZE);
        assert!(
            &*buf == &test_file.content[..buf.len()],
            "read content is different"
        );
    }
}

#[test]
fn read_read_buf_pool_reuse_same_buffer() {
    require_kernel!(5, 19);
    init();

    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();

    let test_file = &LOREM_IPSUM_50;
    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    let path = test_file.path.into();
    let file = block_on(&mut ring, OpenOptions::new().open(sq, path)).unwrap();

    let mut buf = block_on(&mut ring, file.read_at(buf_pool.get(), 0)).unwrap();
    assert_eq!(buf.len(), BUF_SIZE);
    assert_eq!(&*buf, &test_file.content[..buf.len()]);

    // When reusing the buffer it shouldn't overwrite the existing data.
    buf.truncate(100);
    let mut buf = block_on(&mut ring, file.read_at(buf, 0)).unwrap();
    assert_eq!(buf.len(), BUF_SIZE);
    assert_eq!(&buf[0..100], &test_file.content[0..100]);
    assert_eq!(&buf[100..], &test_file.content[0..BUF_SIZE - 100]);

    // After releasing the buffer to the pool it should "overwrite" everything
    // again.
    buf.release();
    let buf = block_on(&mut ring, file.read_at(buf, 0)).unwrap();
    assert_eq!(buf.len(), BUF_SIZE);
    assert_eq!(&*buf, &test_file.content[..buf.len()]);
}

#[test]
fn read_read_buf_pool_out_of_buffers() {
    require_kernel!(5, 19);
    init();

    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();

    let test_file = &LOREM_IPSUM_50;
    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    let path = test_file.path.into();
    let file = block_on(&mut ring, OpenOptions::new().open(sq, path)).unwrap();

    let futures = (0..8)
        .map(|_| {
            let mut read = file.read_at(buf_pool.get(), 0);
            let _ = poll_nop(Pin::new(&mut read));
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
    require_kernel!(5, 19);
    init();

    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();
    let test_file = &LOREM_IPSUM_50;

    let buf_pool1 = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();
    let buf_pool2 = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    let path = test_file.path.into();
    let file = block_on(&mut ring, OpenOptions::new().open(sq, path)).unwrap();

    for buf_pool in [buf_pool1, buf_pool2] {
        let buf = block_on(&mut ring, file.read_at(buf_pool.get(), 0)).unwrap();
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

    require_kernel!(5, 19);
    init();

    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();

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

    require_kernel!(5, 19);
    init();

    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();

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

#[test]
fn write_all() {
    let sq = test_queue();
    let waker = Waker::new();

    let (r, w) = pipe2(sq).expect("failed to create pipe");

    let buf = BadBuf {
        calls: Cell::new(0),
    };
    waker.block_on(w.write_all(buf)).unwrap();

    let buf = waker
        .block_on(r.read(Vec::with_capacity(BadBuf::DATA.len() + 1)))
        .unwrap();
    assert_eq!(buf, BadBuf::DATA);
}

#[test]
fn write_all_at_extract() {
    let sq = test_queue();
    let waker = Waker::new();

    let mut path = temp_dir();
    path.push("write_all_at_extract");

    let _d = defer(|| remove_test_file(&path));

    let open_file = OpenOptions::new()
        .write()
        .create()
        .truncate()
        .open(sq, path.clone());
    let file = waker.block_on(open_file).unwrap();

    let mut expected = Vec::from("Hello".as_bytes());
    waker.block_on(file.write("Hello world")).unwrap();

    let buf = BadBuf {
        calls: Cell::new(0),
    };
    waker.block_on(file.write_all_at(buf, 5).extract()).unwrap();

    let got = std::fs::read(&path).unwrap();
    expected.extend_from_slice(BadBuf::DATA.as_slice());
    assert!(got == expected, "file can't be read back");
}

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
            0 => (ptr, 10),
            1 | 2 => (ptr, 20),
            3 | 4 => (ptr, 30),
            _ => (ptr, 0),
        }
    }
}

#[test]
fn write_all_vectored() {
    let sq = test_queue();
    let waker = Waker::new();

    let (r, w) = pipe2(sq).expect("failed to create pipe");

    let buf = BadBufSlice {
        calls: Cell::new(0),
    };
    waker.block_on(w.write_all_vectored(buf)).unwrap();

    let buf = waker.block_on(r.read(Vec::with_capacity(31))).unwrap();
    assert_eq!(buf[..10], BadBufSlice::DATA1);
    assert_eq!(buf[10..20], BadBufSlice::DATA2);
    assert_eq!(buf[20..], BadBufSlice::DATA3);
}

#[test]
fn write_all_vectored_at_extract() {
    let sq = test_queue();
    let waker = Waker::new();

    let mut path = temp_dir();
    path.push("write_all_vectored_at_extract");

    let _d = defer(|| remove_test_file(&path));

    let open_file = OpenOptions::new()
        .write()
        .create()
        .truncate()
        .open(sq, path.clone());
    let file = waker.block_on(open_file).unwrap();

    let mut expected = Vec::from("Hello".as_bytes());
    waker.block_on(file.write("Hello world")).unwrap();

    let buf = BadBufSlice {
        calls: Cell::new(0),
    };
    waker.block_on(file.write_all_vectored_at(buf, 5)).unwrap();

    let got = std::fs::read(&path).unwrap();
    expected.extend_from_slice(BadBufSlice::DATA1.as_slice());
    expected.extend_from_slice(BadBufSlice::DATA2.as_slice());
    expected.extend_from_slice(BadBufSlice::DATA3.as_slice());
    assert!(got == expected, "file can't be read back");
}

// NOTE: this implementation is BROKEN! It's only used to test the
// write_all_vectored method.
#[derive(Debug)]
pub(crate) struct BadBufSlice {
    pub(crate) calls: Cell<usize>,
}

impl BadBufSlice {
    pub(crate) const DATA1: [u8; 10] = [123, 123, 123, 123, 123, 123, 123, 123, 123, 123];
    pub(crate) const DATA2: [u8; 10] = [200, 200, 200, 200, 200, 200, 200, 200, 200, 200];
    pub(crate) const DATA3: [u8; 10] = [255, 255, 255, 255, 255, 255, 255, 255, 255, 255];
}

unsafe impl BufSlice<3> for BadBufSlice {
    unsafe fn as_iovecs(&self) -> [libc::iovec; 3] {
        let calls = self.calls.get();
        self.calls.set(calls + 1);

        fn cast(ptr: &[u8]) -> *mut libc::c_void {
            ptr.as_ptr().cast_mut().cast()
        }

        [
            libc::iovec {
                iov_base: cast(BadBufSlice::DATA1.as_slice()),
                iov_len: 10,
            },
            libc::iovec {
                iov_base: cast(BadBufSlice::DATA2.as_slice()),
                iov_len: 10,
            },
            libc::iovec {
                iov_base: cast(BadBufSlice::DATA3.as_slice()),
                iov_len: if calls == 0 { 5 } else { 10 },
            },
        ]
    }
}

#[test]
fn read_n() {
    let sq = test_queue();
    let waker = Waker::new();

    let (r, w) = pipe2(sq).expect("failed to create pipe");

    const DATA: &[u8] = b"hello world";
    waker.block_on(w.write(DATA)).unwrap();

    let buf = BadReadBuf {
        data: Vec::with_capacity(30),
    };
    let buf = waker.block_on(r.read_n(buf, DATA.len())).unwrap();
    assert_eq!(&buf.data, DATA);
}

#[test]
fn read_n_at() {
    let sq = test_queue();
    let waker = Waker::new();

    let test_file = &LOREM_IPSUM_5;

    let path = test_file.path.into();
    let open_file = OpenOptions::new().open(sq, path);
    let file = waker.block_on(open_file).unwrap();

    let buf = BadReadBuf {
        data: Vec::with_capacity(test_file.content.len()),
    };
    let buf = waker
        .block_on(file.read_n_at(buf, 5, test_file.content.len() - 5))
        .unwrap();
    assert_eq!(&buf.data, &test_file.content[5..]);
}

// NOTE: this implementation is BROKEN! It's only used to test the write_all
// method.
#[derive(Debug)]
pub(crate) struct BadReadBuf {
    pub(crate) data: Vec<u8>,
}

unsafe impl BufMut for BadReadBuf {
    unsafe fn parts_mut(&mut self) -> (*mut u8, u32) {
        let (ptr, size) = self.data.parts_mut();
        if size >= 10 {
            (ptr, 10)
        } else {
            (ptr, size)
        }
    }

    unsafe fn set_init(&mut self, n: usize) {
        self.data.set_init(n);
    }
}

#[test]
fn read_n_vectored() {
    let sq = test_queue();
    let waker = Waker::new();

    let (r, w) = pipe2(sq).expect("failed to create pipe");

    const DATA: &[u8] = b"Hello marsBooo!! Hi. How are you?";
    waker.block_on(w.write(DATA)).unwrap();

    let buf = BadReadBufSlice {
        data: [Vec::with_capacity(15), Vec::with_capacity(20)],
    };
    let buf = waker.block_on(r.read_n_vectored(buf, DATA.len())).unwrap();
    assert_eq!(&buf.data[0], b"Hello mars! Hi.");
    assert_eq!(&buf.data[1], b"Booo! How are you?");
}

// NOTE: this implementation is BROKEN! It's only used to test the write_all
// method.
#[derive(Debug)]
pub(crate) struct BadReadBufSlice {
    pub(crate) data: [Vec<u8>; 2],
}

unsafe impl BufMutSlice<2> for BadReadBufSlice {
    unsafe fn as_iovecs_mut(&mut self) -> [libc::iovec; 2] {
        let mut iovecs = self.data.as_iovecs_mut();
        if iovecs[0].iov_len >= 10 {
            iovecs[0].iov_len = 10;
            iovecs[1].iov_len = 5;
        }
        iovecs
    }

    unsafe fn set_init(&mut self, n: usize) {
        if n == 0 {
            return;
        }

        if self.as_iovecs_mut()[0].iov_len == 10 {
            self.data[0].set_init(10);
            self.data[1].set_init(n - 10);
        } else {
            self.data.set_init(n);
        }
    }
}

#[test]
fn read_n_vectored_at() {
    let sq = test_queue();
    let waker = Waker::new();

    let test_file = &LOREM_IPSUM_5;

    let path = test_file.path.into();
    let open_file = OpenOptions::new().open(sq, path);
    let file = waker.block_on(open_file).unwrap();

    let buf = GrowingBufSlice {
        data: [
            Vec::with_capacity(100),
            Vec::with_capacity(test_file.content.len() - 200),
        ],
    };
    let buf = waker
        .block_on(file.read_n_vectored_at(buf, 5, test_file.content.len() - 5))
        .unwrap();
    assert_eq!(&buf.data[0], &test_file.content[5..105]);
    assert_eq!(&buf.data[1], &test_file.content[105..]);
}

// NOTE: this implementation is BROKEN! It's only used to test the write_all
// method.
struct GrowingBufSlice {
    data: [Vec<u8>; 2],
}

unsafe impl BufMutSlice<2> for GrowingBufSlice {
    unsafe fn as_iovecs_mut(&mut self) -> [libc::iovec; 2] {
        self.data.as_iovecs_mut()
    }

    unsafe fn set_init(&mut self, n: usize) {
        self.data.set_init(n);
        self.data[1].reserve(200);
    }
}

#[test]
fn cancel_all_accept() {
    require_kernel!(5, 19);

    let sq = test_queue();
    let waker = Waker::new();

    let listener = waker.block_on(tcp_ipv4_socket(sq));
    bind_and_listen_ipv4(&listener);

    let mut accept = listener.accept::<libc::sockaddr_in>();
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut accept)).is_pending());

    let n = waker
        .block_on(listener.cancel_all())
        .expect("failed to cancel all calls");
    assert_eq!(n, 1);

    expect_io_errno(waker.block_on(accept), libc::ECANCELED);
}

#[test]
fn cancel_all_twice_accept() {
    require_kernel!(5, 19);

    let sq = test_queue();
    let waker = Waker::new();

    let listener = waker.block_on(tcp_ipv4_socket(sq));
    bind_and_listen_ipv4(&listener);

    let mut accept = listener.accept::<libc::sockaddr_in>();
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut accept)).is_pending());

    let n = waker
        .block_on(listener.cancel_all())
        .expect("failed to cancel all calls");
    assert_eq!(n, 1);
    let n = waker
        .block_on(listener.cancel_all())
        .expect("failed to cancel all calls");
    assert_eq!(n, 0);

    expect_io_errno(waker.block_on(accept), libc::ECANCELED);
}

#[test]
fn cancel_all_no_operation_in_progress() {
    require_kernel!(5, 19);

    let sq = test_queue();
    let waker = Waker::new();

    let socket = waker.block_on(tcp_ipv4_socket(sq));

    let n = waker
        .block_on(socket.cancel_all())
        .expect("failed to cancel");
    assert_eq!(n, 0);
}

#[test]
fn splice_to() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<Splice>();
    is_sync::<Splice>();

    let (r, w) = pipe2(sq.clone()).expect("failed to create pipe");

    let path = LOREM_IPSUM_50.path;
    let expected = LOREM_IPSUM_50.content;

    let open_file = OpenOptions::new().open(sq, path.into());
    let file = waker.block_on(open_file).unwrap();

    let n = waker
        .block_on(file.splice_to_at(
            10,
            w.as_fd().as_raw_fd(),
            NO_OFFSET,
            expected.len() as u32,
            0,
        ))
        .expect("failed to splice");
    assert_eq!(n, expected.len() - 10);

    let buf = Vec::with_capacity(expected.len() + 1);
    let buf = waker.block_on(r.read_n(buf, expected.len() - 10)).unwrap();
    assert!(buf == expected[10..], "read content is different");
}

#[test]
fn splice_from() {
    let sq = test_queue();
    let waker = Waker::new();

    let expected = LOREM_IPSUM_50.content;

    let mut path = temp_dir();
    path.push("splice_from");
    let _d = defer(|| remove_test_file(&path));

    let open_file = OpenOptions::new()
        .write()
        .create()
        .truncate()
        .open(sq.clone(), path.clone());
    let file = waker.block_on(open_file).unwrap();

    let (r, w) = pipe2(sq).expect("failed to create pipe");

    waker
        .block_on(w.write_all(expected))
        .expect("failed to write all");

    let n = waker
        .block_on(file.splice_from_at(
            10,
            r.as_fd().as_raw_fd(),
            NO_OFFSET,
            expected.len() as u32,
            0,
        ))
        .expect("failed to splice");
    assert_eq!(n, expected.len());

    let buf = Vec::with_capacity(expected.len() + 11);
    //let buf = waker.block_on(file.read_n(buf, expected.len())).unwrap();
    let buf = waker.block_on(file.read(buf)).unwrap();
    assert!(&buf[10..] == expected, "read content is different");
}

#[test]
fn close_socket_fd() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<Close>();
    is_sync::<Close>();

    let socket = waker.block_on(tcp_ipv4_socket(sq));
    waker.block_on(socket.close()).expect("failed to close fd");
}

#[test]
fn close_fs_fd() {
    let sq = test_queue();
    let waker = Waker::new();

    let open_file = OpenOptions::new().open(sq, "tests/data/lorem_ipsum_5.txt".into());
    let file = waker.block_on(open_file).unwrap();
    waker.block_on(file.close()).expect("failed to close fd");
}

#[test]
fn dropped_futures_do_not_leak_buffers() {
    // NOTE: run this test with the `leak` or `address` sanitizer, see the
    // test_sanitizer Make target, and it shouldn't cause any errors.

    let sq = test_queue();
    let waker = Waker::new();

    let open_file = OpenOptions::new().write().open_temp_file(sq, temp_dir());
    let file = waker.block_on(open_file).unwrap();

    let buf = vec![123; 64 * 1024];
    let write = file.write(buf);
    drop(write);
}

#[test]
fn stdout_write() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<Stdout>();
    is_sync::<Stdout>();

    let stdout = stdout(sq);
    waker.block_on(stdout.write("Hello, stdout!\n")).unwrap();
}

#[test]
fn stderr_write() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<Stderr>();
    is_sync::<Stderr>();

    let stderr = stderr(sq);
    waker.block_on(stderr.write("Hello, stderr!\n")).unwrap();
}

fn pipe2(sq: SubmissionQueue) -> io::Result<(AsyncFd, AsyncFd)> {
    let mut fds: [RawFd; 2] = [-1, -1];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: we just initialised the `fds` above.
    let r = unsafe { AsyncFd::from_raw_fd(fds[0], sq.clone()) };
    let w = unsafe { AsyncFd::from_raw_fd(fds[1], sq) };
    Ok((r, w))
}

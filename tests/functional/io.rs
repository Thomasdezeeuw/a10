use std::cell::Cell;
use std::future::Future;
use std::io;
use std::os::fd::{FromRawFd, RawFd};

use a10::fs::{self, OpenOptions};
#[cfg(any(target_os = "android", target_os = "linux"))]
use a10::io::ReadBufPool;
#[cfg(any(target_os = "android", target_os = "linux"))]
use a10::io::Splice;
use a10::io::{BufMut, Close, Stderr, Stdout, stderr, stdout};
use a10::{AsyncFd, Extract, SubmissionQueue};

use crate::util::{
    BadBuf, BadBufSlice, BadReadBuf, BadReadBufSlice, GrowingBufSlice, LOREM_IPSUM_5,
    LOREM_IPSUM_50, Waker, defer, is_send, is_sync, pipe, remove_test_file, tcp_ipv4_socket,
    test_queue, tmp_path,
};
#[cfg(any(target_os = "android", target_os = "linux"))]
use crate::util::{fd, next};

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
fn write_all() {
    let sq = test_queue();
    let waker = Waker::new();

    let (r, w) = pipe2(sq).expect("failed to create pipe");

    waker.block_on(w.write_all(BadBuf::new())).unwrap();

    let buf = waker
        .block_on(r.read(Vec::with_capacity(BadBuf::DATA.len() + 1)))
        .unwrap();
    assert_eq!(buf, BadBuf::DATA);
}

#[test]
fn write_all_at_extract() {
    let sq = test_queue();
    let waker = Waker::new();

    let path = tmp_path();
    let _d = defer(|| remove_test_file(&path));

    let open_file = OpenOptions::new()
        .write()
        .create()
        .truncate()
        .open(sq, path.clone());
    let file = waker.block_on(open_file).unwrap();

    let mut expected = Vec::from("Hello".as_bytes());
    waker.block_on(file.write("Hello world")).unwrap();

    waker
        .block_on(file.write_all(BadBuf::new()).at(5).extract())
        .unwrap();

    let got = std::fs::read(&path).unwrap();
    expected.extend_from_slice(BadBuf::DATA.as_slice());
    assert!(got == expected, "file can't be read back");
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
    assert_eq!(buf[..10], *BadBufSlice::DATA1);
    assert_eq!(buf[10..20], *BadBufSlice::DATA2);
    assert_eq!(buf[20..], *BadBufSlice::DATA3);
}

#[test]
fn write_all_vectored_at_extract() {
    let sq = test_queue();
    let waker = Waker::new();

    let path = tmp_path();
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
    waker.block_on(file.write_all_vectored(buf).at(5)).unwrap();

    let got = std::fs::read(&path).unwrap();
    expected.extend_from_slice(BadBufSlice::DATA1);
    expected.extend_from_slice(BadBufSlice::DATA2);
    expected.extend_from_slice(BadBufSlice::DATA3);
    assert!(got == expected, "file can't be read back");
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn multishot_read() {
    let sq = test_queue();
    let waker = Waker::new();

    let buf_pool = ReadBufPool::new(sq.clone(), 2, 128).expect("failed to create buf pool");

    let (r, w) = pipe2(sq).expect("failed to create pipe");

    let mut reads = r.multishot_read(buf_pool.clone());

    const DATA1: &[u8] = b"Hello world!";
    const DATA2: &[u8] = b"Hello mars!";

    waker.block_on(w.write_all(DATA1)).unwrap();

    let buf = waker
        .block_on(next(&mut reads))
        .unwrap()
        .expect("failed to read");
    assert_eq!(&*buf, DATA1);

    waker.block_on(w.write_all(DATA2)).unwrap();

    let buf = waker
        .block_on(next(&mut reads))
        .unwrap()
        .expect("failed to read");
    assert_eq!(&*buf, DATA2);
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
fn read_n_from() {
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
        .block_on(file.read_n(buf, test_file.content.len() - 5).from(5))
        .unwrap();
    assert_eq!(&buf.data, &test_file.content[5..]);
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

#[test]
fn read_n_vectored_from() {
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
        .block_on(
            file.read_n_vectored(buf, test_file.content.len() - 5)
                .from(5),
        )
        .unwrap();
    assert_eq!(&buf.data[0], &test_file.content[5..105]);
    assert_eq!(&buf.data[1], &test_file.content[105..]);
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
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
        .block_on(file.splice_to(fd(&w), expected.len() as u32).from(10))
        .expect("failed to splice");
    assert_eq!(n, expected.len() - 10);

    let buf = Vec::with_capacity(expected.len() + 1);
    let buf = waker.block_on(r.read_n(buf, expected.len() - 10)).unwrap();
    assert!(buf == expected[10..], "read content is different");
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn splice_from() {
    let sq = test_queue();
    let waker = Waker::new();

    let expected = LOREM_IPSUM_50.content;

    let path = tmp_path();
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
        .block_on(file.splice_from(fd(&r), expected.len() as u32).at(10))
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
fn dropping_should_close_socket_fd() {
    let sq = test_queue();
    let waker = Waker::new();

    let socket = waker.block_on(tcp_ipv4_socket(sq));
    drop(socket);
}

#[test]
fn dropping_should_close_fs_fd() {
    let sq = test_queue();
    let waker = Waker::new();

    let open_file = OpenOptions::new().open(sq, "tests/data/lorem_ipsum_5.txt".into());
    let file = waker.block_on(open_file).unwrap();
    drop(file);
}

#[test]
fn dropped_futures_do_not_leak_buffers() {
    // NOTE: run this test with the `leak` or `address` sanitizer, see the
    // test_sanitizer Make target, and it shouldn't cause any errors.

    let sq = test_queue();
    let waker = Waker::new();

    let open_file = OpenOptions::new().write().create().open(sq, tmp_path());
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
    if unsafe { libc::pipe(fds.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: we just initialised the `fds` above.
    let r = unsafe { AsyncFd::from_raw_fd(fds[0], sq.clone()) };
    let w = unsafe { AsyncFd::from_raw_fd(fds[1], sq) };
    Ok((r, w))
}

#[test]
fn read_small_file() {
    all_bufs!(for new_buf in bufs {
        test_read(&LOREM_IPSUM_5.content, open_file, new_buf)
    });
}

#[test]
fn read_large_file() {
    all_bufs!(for new_buf in bufs {
        test_read(&LOREM_IPSUM_50.content, open_file, new_buf)
    });
}

#[test]
fn read_small_pipe() {
    all_bufs!(for new_buf in bufs {
        test_read(&LOREM_IPSUM_5.content, open_read_pipe, new_buf)
    });
}

#[test]
fn read_large_pipe() {
    all_bufs!(for new_buf in bufs {
        test_read(&LOREM_IPSUM_50.content, open_read_pipe, new_buf)
    });
}

fn test_read<F, Fut, B>(expected: &'static [u8], open_fd: F, new_buf: fn() -> B)
where
    F: FnOnce(&'static [u8], SubmissionQueue) -> Fut,
    Fut: Future<Output = AsyncFd>,
    B: TestBuf,
{
    let sq = test_queue();
    let waker = Waker::new();

    let fd = waker.block_on(open_fd(expected, sq.clone()));
    let mut buf = new_buf();

    let mut expected = expected;
    loop {
        buf = waker.block_on(fd.read(buf)).expect("failed to read");
        assert_ne!(buf.len(), 0);
        assert_eq!(buf.bytes().len(), buf.len());
        if buf.bytes().len() > expected.len() {
            panic!(
                "read too much: buf: {}, expected: {}",
                buf.len(),
                expected.len(),
            );
        }
        assert_eq!(buf.bytes(), &expected[..buf.len()]);
        expected = &expected[buf.len()..];
        if expected.is_empty() {
            break;
        }
        buf.reset();
    }
}

async fn open_file(expected: &'static [u8], sq: SubmissionQueue) -> AsyncFd {
    let tmp_path = tmp_path();
    std::fs::write(&tmp_path, expected).expect("failed to write to file");
    fs::open_file(sq, tmp_path)
        .await
        .expect("failed to open file")
}

async fn open_read_pipe(expected: &'static [u8], sq: SubmissionQueue) -> AsyncFd {
    let [r, w] = pipe();
    // SAFETY: we just initialised the `fds` above.
    let r = unsafe { AsyncFd::from_raw_fd(r, sq) };
    let mut w = unsafe { std::fs::File::from_raw_fd(w) };

    std::thread::spawn(move || {
        std::io::Write::write_all(&mut w, expected).expect("failed to write all data to pipe");
    });

    r
}

/// Macro to run a code block with all buffer kinds.
macro_rules! all_bufs {
    (
        for $new_buf: ident in bufs
            $code: block
    ) => {{
        all_bufs!(for $new_buf in [ small_vec, vec, large_vec] $code);
    }};
    (
        // Private.
        for $new_buf: ident in [ $( $create_buf: ident ),+ ]
            $code: block
    ) => {{
        $(
        {
            let $new_buf = $create_buf;
            $code
        }
        )+
    }};
}

use all_bufs;

/// NOTE: all implementations should be in the `all_bufs` macro.
trait TestBuf: BufMut {
    fn len(&self) -> usize;
    fn bytes(&self) -> &[u8];
    fn reset(&mut self);
}

impl TestBuf for Vec<u8> {
    fn len(&self) -> usize {
        self.len()
    }

    fn bytes(&self) -> &[u8] {
        &*self
    }

    fn reset(&mut self) {
        self.clear()
    }
}

fn small_vec() -> Vec<u8> {
    Vec::with_capacity(64)
}

fn vec() -> Vec<u8> {
    Vec::with_capacity(4 * 1024) // 4KB.
}

fn large_vec() -> Vec<u8> {
    Vec::with_capacity(1024 * 1024) // 1MB.
}

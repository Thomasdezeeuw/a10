use a10::fd::{self, AsyncFd, Kind, ToDirect, ToFd};
use a10::fs::OpenOptions;

use crate::util::{
    LOREM_IPSUM_5, Waker, expect_io_errno, is_send, is_sync, require_kernel, test_queue,
};

#[test]
fn async_fd_size() {
    assert_eq!(std::mem::size_of::<AsyncFd>(), 16);
    assert_eq!(std::mem::size_of::<Option<AsyncFd>>(), 16);
}

#[test]
fn async_fd_is_send_and_sync() {
    is_send::<AsyncFd>();
    is_sync::<AsyncFd>();
}

#[test]
fn kind_is_send_and_sync() {
    is_send::<Kind>();
    is_sync::<Kind>();
}

#[test]
fn to_fd_is_send_and_sync() {
    is_send::<ToFd>();
    is_sync::<ToFd>();
}

#[test]
fn to_direct_is_send_and_sync() {
    is_send::<ToDirect>();
    is_sync::<ToDirect>();
}

#[test]
fn to_direct_descriptor() {
    require_kernel!(5, 5);
    test_change_descriptor_type(fd::Kind::File);
}

#[test]
fn to_file_descriptor() {
    require_kernel!(6, 8);
    test_change_descriptor_type(fd::Kind::Direct);
}

fn test_change_descriptor_type(fd_kind: fd::Kind) {
    let sq = test_queue();
    let waker = Waker::new();

    let open_file = OpenOptions::new()
        .kind(fd_kind)
        .open(sq, LOREM_IPSUM_5.path.into());
    let fd = waker.block_on(open_file).unwrap();

    let (regular_fd, direct_fd) = if let fd::Kind::File = fd_kind {
        let regular_fd = fd;
        let direct_fd = waker.block_on(regular_fd.to_direct_descriptor()).unwrap();
        (regular_fd, direct_fd)
    } else {
        let direct_fd = fd;
        let regular_fd = waker.block_on(direct_fd.to_file_descriptor()).unwrap();
        (regular_fd, direct_fd)
    };

    let mut buf = Vec::with_capacity(LOREM_IPSUM_5.content.len() + 1);

    // Regular file descriptor.
    let read = regular_fd.read_at(buf, 0);
    buf = waker.block_on(read).unwrap();
    if buf.is_empty() {
        panic!("read zero bytes");
    }
    assert!(
        buf == &LOREM_IPSUM_5.content[..buf.len()],
        "read content is different"
    );

    // Direct descriptor.
    buf.clear();
    let read = direct_fd.read_at(buf, 0);
    buf = waker.block_on(read).unwrap();
    if buf.is_empty() {
        panic!("read zero bytes");
    }
    assert!(
        buf == &LOREM_IPSUM_5.content[..buf.len()],
        "read content is different"
    );

    waker.block_on(regular_fd.close()).unwrap();
    waker.block_on(direct_fd.close()).unwrap();
}

#[test]
#[cfg_attr(
    debug_assertions,
    should_panic = "can't covert a direct descriptor to a different direct descriptor"
)]
fn direct_to_direct_descriptor() {
    let sq = test_queue();
    let waker = Waker::new();

    let open_file = OpenOptions::new()
        .kind(fd::Kind::Direct)
        .open(sq, LOREM_IPSUM_5.path.into());
    let direct_fd = waker.block_on(open_file).unwrap();
    // This should panic.
    let res = waker.block_on(direct_fd.to_direct_descriptor());
    expect_io_errno(res, libc::EINVAL);
}

#[test]
#[cfg_attr(
    debug_assertions,
    should_panic = "can't covert a file descriptor to a different file descriptor"
)]
fn file_to_file_descriptor() {
    let sq = test_queue();
    let waker = Waker::new();

    let open_file = OpenOptions::new().open(sq, LOREM_IPSUM_5.path.into());
    let regular_fd = waker.block_on(open_file).unwrap();
    // This should panic.
    let res = waker.block_on(regular_fd.to_file_descriptor());
    expect_io_errno(res, libc::EBADF);
}

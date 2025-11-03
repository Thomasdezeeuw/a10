//! Tests for the usage of direct descriptors.

use std::sync::Arc;

use a10::fd::{self, AsyncFd, Direct, File};
use a10::fs::OpenOptions;

use crate::util::{require_kernel, test_queue, Waker, LOREM_IPSUM_5};

#[test]
fn to_direct_descriptor() {
    let sq = test_queue();
    let waker = Waker::new();

    let open_file = OpenOptions::new().open(sq, LOREM_IPSUM_5.path.into());
    let regular_fd: AsyncFd<File> = waker.block_on(open_file).unwrap();
    let direct_fd: AsyncFd<Direct> = waker.block_on(regular_fd.to_direct_descriptor()).unwrap();

    check_fs_fd(waker, regular_fd, direct_fd);
}

#[test]
fn to_file_descriptor() {
    require_kernel!(6, 8);

    let sq = test_queue();
    let waker = Waker::new();

    let open_file = OpenOptions::new()
        .kind(fd::Kind::Direct)
        .open(sq, LOREM_IPSUM_5.path.into());
    let direct_fd: AsyncFd<Direct> = waker.block_on(open_file).unwrap();
    let regular_fd: AsyncFd<File> = waker.block_on(direct_fd.to_file_descriptor()).unwrap();

    check_fs_fd(waker, regular_fd, direct_fd);
}

fn check_fs_fd(waker: Arc<Waker>, regular_fd: AsyncFd<File>, direct_fd: AsyncFd<Direct>) {
    let mut buf = Vec::with_capacity(LOREM_IPSUM_5.content.len() + 1);

    // Regular fd.
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

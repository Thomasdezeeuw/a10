//! Tests for the usage of direct descriptors.

use a10::fd;
use a10::fs::OpenOptions;

use crate::util::{require_kernel, test_queue, Waker, LOREM_IPSUM_5};

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
#[should_panic = "can't covert a direct descriptor to a different direct descriptor"]
fn direct_to_direct_descriptor() {
    let sq = test_queue();
    let waker = Waker::new();

    let open_file = OpenOptions::new()
        .kind(fd::Kind::Direct)
        .open(sq, LOREM_IPSUM_5.path.into());
    let direct_fd = waker.block_on(open_file).unwrap();
    // This should panic.
    waker.block_on(direct_fd.to_direct_descriptor()).unwrap();
}

#[test]
#[should_panic = "can't covert a file descriptor to a different file descriptor"]
fn file_to_file_descriptor() {
    let sq = test_queue();
    let waker = Waker::new();

    let open_file = OpenOptions::new().open(sq, LOREM_IPSUM_5.path.into());
    let regular_fd = waker.block_on(open_file).unwrap();
    // This should panic.
    waker.block_on(regular_fd.to_file_descriptor()).unwrap();
}

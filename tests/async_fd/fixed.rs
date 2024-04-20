//! Tests for the usage of direct descriptors.

use a10::fs::OpenOptions;

use crate::util::{require_kernel, test_queue, Waker, LOREM_IPSUM_5};

#[test]
fn to_direct_descriptor() {
    let sq = test_queue();
    let waker = Waker::new();

    let open_file = OpenOptions::new().open(sq, LOREM_IPSUM_5.path.into());
    let regular_fd = waker.block_on(open_file).unwrap();
    let direct_fd = waker.block_on(regular_fd.to_direct_descriptor()).unwrap();

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
}

#[test]
fn to_file_descriptor() {
    require_kernel!(6, 8);

    let sq = test_queue();
    let waker = Waker::new();

    // TODO: use open with direct descriptor?
    let open_file = OpenOptions::new().open(sq, LOREM_IPSUM_5.path.into());
    let regular_fd = waker.block_on(open_file).unwrap();
    let direct_fd = waker.block_on(regular_fd.to_direct_descriptor()).unwrap();
    let regular_fd2 = waker.block_on(direct_fd.to_file_descriptor()).unwrap();

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

    // Regular fd (created from the direct descriptor).
    buf.clear();
    let read = regular_fd2.read_at(buf, 0);
    buf = waker.block_on(read).unwrap();
    if buf.is_empty() {
        panic!("read zero bytes");
    }
    assert!(
        buf == &LOREM_IPSUM_5.content[..buf.len()],
        "read content is different"
    );
}

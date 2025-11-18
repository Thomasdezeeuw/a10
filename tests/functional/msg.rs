use std::pin::{Pin, pin};

use a10::msg::{self, Message};

use crate::util::{
    Waker, is_send, is_sync, next, poll_nop, require_kernel, start_mulitshot_op, test_queue,
};

#[test]
fn listen_is_send_and_sync() {
    is_send::<msg::Listener>();
    is_sync::<msg::Listener>();
}

#[test]
fn send_is_send_and_sync() {
    is_send::<msg::Sender>();
    is_sync::<msg::Sender>();
}

#[test]
fn message_is_send_and_sync() {
    is_send::<Message>();
    is_sync::<Message>();
}

#[test]
fn send_msg_is_send_and_sync() {
    is_send::<msg::SendMsg>();
    is_sync::<msg::SendMsg>();
}

#[test]
fn message_sending() {
    require_kernel!(5, 18);

    const DATA1: u32 = 123;
    const DATA2: u32 = u32::MAX;

    let sq = test_queue();
    let waker = Waker::new();

    let (listener, sender) = msg::listener(sq.clone()).unwrap();
    let mut listener = pin!(listener);
    start_mulitshot_op(&mut listener);

    // Send some messages.
    sender.try_send(DATA1).unwrap();
    waker.block_on(pin!(sender.send(DATA2))).unwrap();

    assert_eq!(waker.block_on(next(listener.as_mut())), Some(DATA1));
    assert_eq!(waker.block_on(next(listener.as_mut())), Some(DATA2));

    assert!(poll_nop(Pin::new(&mut next(listener.as_mut()))).is_pending());
}

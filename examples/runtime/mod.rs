//! A bad [`Future`] runtime implementation. This is just here for the examples,
//! don't actually use this. Replace this with your favorite [`Future`] runtime.

use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::ptr;
use std::task::{self, Poll};

use a10::Ring;

/// Block on the `future`, expecting polling `ring` to drive it forward.
pub fn block_on<Fut>(ring: &mut Ring, future: Fut) -> Fut::Output
where
    Fut: IntoFuture,
{
    let mut future = future.into_future();
    let mut future = unsafe { Pin::new_unchecked(&mut future) };

    loop {
        match poll_future(future.as_mut()) {
            Poll::Ready(output) => return output,
            Poll::Pending => {
                // Poll the `Ring` to get an update on the operation(s).
                //
                // In pratice you would first yield to another future, but in
                // this example we don't have one, so we'll always poll the
                // `Ring`.
                ring.poll(None).expect("failed to poll");
            }
        }
    }
}

/// Since we only have a single future we don't need to be awoken.
fn poll_future<Fut>(fut: Pin<&mut Fut>) -> Poll<Fut::Output>
where
    Fut: Future,
{
    use std::task::{RawWaker, RawWakerVTable};
    static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
        |_| RawWaker::new(ptr::null(), &WAKER_VTABLE),
        |_| {},
        |_| {},
        |_| {},
    );
    let waker = unsafe { task::Waker::from_raw(RawWaker::new(ptr::null(), &WAKER_VTABLE)) };
    let mut ctx = task::Context::from_waker(&waker);
    fut.poll(&mut ctx)
}

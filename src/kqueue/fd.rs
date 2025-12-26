use std::mem::replace;
use std::ptr;
use std::sync::atomic::AtomicPtr;

use crate::SubmissionQueue;

#[derive(Debug)]
pub(crate) struct State {
    inner: AtomicPtr<OpState>,
}

/// For kqueue the pair `ident` and `filter`, or file descriptor and operation
/// kind (e.g. `EVFILT_READ`) for our use case, is unique. This means that
/// multiple (e.g.) read operations will need to share a single completion
/// event.
///
/// To do this we use this type to share a single completion event with multiple
/// Futures waiting on the same filters.
pub(super) struct OpState {
    // TODO: add registered operations, etc.
}

impl State {
    pub(crate) const fn new() -> State {
        State {
            inner: AtomicPtr::new(ptr::null_mut()),
        }
    }

    pub(crate) unsafe fn drop(&mut self, sq: &SubmissionQueue) {
        let ptr = replace(self.inner.get_mut(), ptr::null_mut());
        if ptr.is_null() {
            // Never started any operations, so we don't have to clean anything up.
            return;
        }

        // Because polling for events is done via a different type (not
        // `AsyncFd`, but `Ring`), it can happen on another thread. Which means
        // we don't know if another thread is currently accessing the operations
        // state (via `Ring::poll`). So, we have to delay its deallocation until
        // we know for sure that no other thread is accessing it to prevent a
        // use-after-free.
        //
        // We do this by sending a kevent to the polling thread to deallocate
        // the operation state for us.
        let result = sq.inner.submit_no_completion(|kevent| {
            kevent.0.ident = ptr as _;
            kevent.0.filter = libc::EVFILT_USER;
        });
        if let Err(err) = result {
            log::warn!("failed to submit delayed allocation, leaking operation state at {ptr:p}");
        }
    }
}

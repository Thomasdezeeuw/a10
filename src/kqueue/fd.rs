use std::mem::replace;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::{ptr, task};

use crate::{kqueue, lock, SubmissionQueue};

/// State of a file descriptor.
#[derive(Debug)]
pub(crate) struct State {
    /// # SAFETY
    ///
    /// This pointer maybe be null at the start and then initialised once and
    /// not be changed until it's dropped.
    shared: AtomicPtr<SharedState>,
}

/// State shared between all operations of a single file descriptor.
pub(super) type SharedState = Mutex<OpState>;

impl State {
    pub(crate) const fn new() -> State {
        State {
            shared: AtomicPtr::new(ptr::null_mut()),
        }
    }

    pub(super) fn lock<'a>(&'a self) -> MutexGuard<'a, OpState> {
        let mut ptr = self.shared.load(Ordering::Acquire);
        if ptr.is_null() {
            let op_state = Box::new(Mutex::new(OpState {
                ops: Vec::with_capacity(1),
            }));
            let state_ptr = Box::into_raw(op_state);
            let res = self.shared.compare_exchange(
                ptr::null_mut(),
                state_ptr,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            match res {
                Ok(old_ptr) => {
                    debug_assert!(old_ptr.is_null());
                    ptr = state_ptr;
                }
                Err(old_ptr) => {
                    debug_assert!(!old_ptr.is_null());
                    debug_assert!(old_ptr != state_ptr);
                    ptr = old_ptr;
                    // SAFETY: created the box above, but we're not using it.
                    drop(unsafe { Box::from_raw(state_ptr) });
                }
            }
        }

        // SAFETY: ensured the pointer is valid above.
        lock(unsafe { &*ptr })
    }

    /// Returns the `kevent::udata` to register events for this fd (state).
    pub(super) fn as_udata(&self) -> *mut libc::c_void {
        // SAFETY: this is only called *after* we called lock, which ensures
        // that the pointer is written. And since the pointer may only be
        // written to once it can't change, making Relaxed ordering ok.
        self.shared.load(Ordering::Relaxed).cast()
    }

    /// # SAFETY
    ///
    /// Only call this if the state needs to be dropped.
    pub(crate) unsafe fn drop(&mut self, sq: &SubmissionQueue) {
        let ptr = replace(self.shared.get_mut(), ptr::null_mut());
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
        // We do this by sending an event to the polling thread to deallocate
        // the operation state for us.
        sq.submissions().add(|event| {
            event.0.ident = ptr as _;
            event.0.filter = libc::EVFILT_USER;
        });
    }
}

/// For kqueue the pair `ident` and `filter` is unique. For our use case here
/// that means the file descriptor and operation kind (e.g. `EVFILT_READ`) is
/// unique. This means that multiple (e.g.) read operations will need to share a
/// single event register.
///
/// To do this we use this type to share a single completion event with multiple
/// Futures waiting on the same filters.
#[derive(Debug)]
pub(super) struct OpState {
    ops: Vec<(OpKind, task::Waker)>,
}

/// Kind of operation (or filter in kqueue terminology).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum OpKind {
    /// EVFILT_READ
    Read,
    /// EVFILT_WRITE
    Write,
}

impl OpState {
    /// Returns true if we already have an event for the given operation kind.
    pub(crate) fn has_waiting_op(&self, op: OpKind) -> bool {
        self.ops.iter().any(|(kind, _)| op == *kind)
    }

    pub(crate) fn add(&mut self, op: OpKind, waker: task::Waker) {
        self.ops.push((op, waker));
    }

    /// Wake the relevant future based on `event`.
    pub(crate) fn wake(&mut self, event: &kqueue::Event) {
        self.ops
            .extract_if(.., |(filter, _)| match filter {
                OpKind::Read => event.0.filter == libc::EVFILT_READ,
                OpKind::Write => event.0.filter == libc::EVFILT_WRITE,
            })
            .for_each(|(_, waker)| waker.wake())
    }
}

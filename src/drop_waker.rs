//! [`DropWake`] trait and implementations.
//!
//! See [`drop_task_waker`].

use std::cell::UnsafeCell;
use std::ffi::CString;
use std::{ptr, task};

use crate::io::Buffer;
use crate::net::AddressStorage;
use crate::{sq, OperationId, SubmissionQueue};

/// Create a [`task::Waker`] that will drop itself when the waker is dropped.
///
/// # Safety
///
/// The returned `task::Waker` cannot be cloned, it will panic.
pub(crate) unsafe fn drop_task_waker<T: DropWake>(to_drop: T) -> task::Waker {
    // SAFETY: we meet the `task::Waker` and `task::RawWaker` requirements.
    unsafe {
        task::Waker::from_raw(task::RawWaker::new(
            to_drop.into_waker_data(),
            &task::RawWakerVTable::new(
                |_| panic!("attempted to clone `a10::drop_task_waker`"),
                // SAFETY: `wake` takes ownership, so dropping is safe.
                T::drop_from_waker_data,
                |_| { /* `wake_by_ref` is a no-op. */ },
                T::drop_from_waker_data,
            ),
        ))
    }
}

/// Trait used by [`drop_task_waker`].
pub(crate) trait DropWake {
    /// Return itself as waker data.
    fn into_waker_data(self) -> *const ();

    /// Drop the waker `data` created by `into_waker_data`.
    unsafe fn drop_from_waker_data(data: *const ());
}

impl<T: DropWake> DropWake for UnsafeCell<T> {
    fn into_waker_data(self) -> *const () {
        self.into_inner().into_waker_data()
    }

    unsafe fn drop_from_waker_data(data: *const ()) {
        T::drop_from_waker_data(data);
    }
}

impl<T> DropWake for Box<T> {
    fn into_waker_data(self) -> *const () {
        Box::into_raw(self).cast()
    }

    unsafe fn drop_from_waker_data(data: *const ()) {
        drop(Box::<T>::from_raw(data.cast_mut().cast()));
    }
}

impl DropWake for CString {
    fn into_waker_data(self) -> *const () {
        CString::into_raw(self).cast()
    }

    unsafe fn drop_from_waker_data(data: *const ()) {
        drop(CString::from_raw(data.cast_mut().cast()));
    }
}

impl DropWake for SubmissionQueue {
    fn into_waker_data(self) -> *const () {
        unsafe { sq::Queue::into_raw(self.inner) }
    }

    unsafe fn drop_from_waker_data(data: *const ()) {
        drop(SubmissionQueue {
            inner: sq::Queue::from_raw(data),
        });
    }
}

impl<A> DropWake for AddressStorage<Box<A>> {
    fn into_waker_data(self) -> *const () {
        self.0.into_waker_data()
    }

    unsafe fn drop_from_waker_data(data: *const ()) {
        Box::<A>::drop_from_waker_data(data);
    }
}

// Don't need to be deallocated.

impl DropWake for () {
    fn into_waker_data(self) -> *const () {
        ptr::null()
    }

    unsafe fn drop_from_waker_data(_: *const ()) {}
}

impl DropWake for OperationId {
    fn into_waker_data(self) -> *const () {
        // Doesn't have to stay alive.
        ptr::null()
    }

    unsafe fn drop_from_waker_data(_: *const ()) {}
}

// Uses a `Box` to get a single pointer.

impl<T, U> DropWake for (T, U) {
    fn into_waker_data(self) -> *const () {
        Box::from(self).into_waker_data()
    }

    unsafe fn drop_from_waker_data(data: *const ()) {
        Box::<(T, U)>::drop_from_waker_data(data);
    }
}

impl<B> DropWake for Buffer<B> {
    fn into_waker_data(self) -> *const () {
        Box::<B>::from(self.buf).into_waker_data()
    }

    unsafe fn drop_from_waker_data(data: *const ()) {
        Box::<B>::drop_from_waker_data(data);
    }
}

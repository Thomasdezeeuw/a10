//! [`DropWake`] trait and implementations.
//!
//! See [`drop_task_waker`].

use std::cell::UnsafeCell;
use std::ffi::CString;
use std::ptr;
use std::sync::Arc;
use std::task;

/// Create a [`task::Waker`] that will drop itself when the waker is dropped.
///
/// # Safety
///
/// The returned `task::Waker` cannot be cloned, it will panic.
pub(crate) unsafe fn drop_task_waker<T: DropWake>(to_drop: T) -> task::Waker {
    unsafe fn drop_by_ptr<T: DropWake>(data: *const ()) {
        T::drop_from_waker_data(data);
    }

    // SAFETY: we meet the `task::Waker` and `task::RawWaker` requirements.
    unsafe {
        task::Waker::from_raw(task::RawWaker::new(
            to_drop.into_waker_data(),
            &task::RawWakerVTable::new(
                |_| panic!("attempted to clone `a10::drop_task_waker`"),
                // SAFETY: `wake` takes ownership, so dropping is safe.
                drop_by_ptr::<T>,
                |_| { /* `wake_by_ref` is a no-op. */ },
                drop_by_ptr::<T>,
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

impl<T> DropWake for UnsafeCell<T>
where
    T: DropWake,
{
    fn into_waker_data(self) -> *const () {
        self.into_inner().into_waker_data()
    }

    unsafe fn drop_from_waker_data(data: *const ()) {
        T::drop_from_waker_data(data);
    }
}

impl<T> DropWake for (T,)
where
    T: DropWake,
{
    fn into_waker_data(self) -> *const () {
        self.0.into_waker_data()
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

impl<T> DropWake for Arc<T> {
    fn into_waker_data(self) -> *const () {
        Arc::into_raw(self).cast()
    }

    unsafe fn drop_from_waker_data(data: *const ()) {
        drop(Arc::<T>::from_raw(data.cast_mut().cast()));
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

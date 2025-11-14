//! Utilities for memory sanitizer support.

// Let's make it easier with all the `cfg`s.
#![allow(unused_variables)]

use std::cmp::min;
use std::ffi::c_void;
use std::mem;

use crate::io::{BufMut, IoMutSlice};

// TODO: replace with `std::ffi::c_size_t` once stable
// <https://github.com/rust-lang/rust/issues/88345>.
#[allow(non_camel_case_types)]
type c_size_t = usize;

// From <sanitizer/asan_interface.h>.
#[cfg_attr(feature = "nightly", cfg(sanitize = "memory"))]
#[cfg_attr(not(feature = "nightly"), cfg(false))]
unsafe extern "C" {
    fn __msan_unpoison(addr: *const c_void, size: c_size_t);
}

/// Mark a memory region as fully initialized.
pub(crate) fn unpoison_region(addr: *const c_void, size: c_size_t) {
    #[cfg_attr(feature = "nightly", cfg(sanitize = "memory"))]
    #[cfg_attr(not(feature = "nightly"), cfg(false))]
    unsafe {
        __msan_unpoison(addr, size);
    }
}

/// Mark memory storing `T` as fully initialized.
#[allow(clippy::borrowed_box)]
pub(crate) fn unpoison_box<T>(value: &Box<T>) {
    // TODO: replace with `Box::as_ptr` once stable
    // <https://github.com/rust-lang/rust/issues/129090>.
    unpoison_region((&raw const **value).cast(), mem::size_of::<T>());
}

/// Mark the first `n` bytes of buf `B` as fully initialized.
pub(crate) fn unpoison_buf_mut<B: BufMut>(buf: &mut B, n: usize) {
    let (addr, _) = unsafe { buf.parts_mut() };
    unpoison_region(addr.cast(), n);
}

/// Mark the first `n` bytes of iovecs as fully initialized.
pub(crate) fn unpoison_iovecs_mut(iovecs: &[IoMutSlice], n: usize) {
    let mut left = n;
    for iovec in iovecs {
        let m = min(left, iovec.len());
        unpoison_region(unsafe { iovec.ptr().cast() }, m);
        left -= m;
        if left == 0 {
            break;
        }
    }
}

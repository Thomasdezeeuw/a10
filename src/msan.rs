//! Utilities for memory sanitizer support.

// Let's make it easier with all the `cfg`s.
#![allow(unused_variables, dead_code)]

use crate::io::IoMutSlice;
use std::cmp::min;
use std::ffi::c_void;

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

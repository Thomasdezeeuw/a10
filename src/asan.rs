//! Utilities for address sanitizer support.

// Let's make it easier with all the `cfg`s.
#![allow(unused_variables)]

use std::ffi::{CString, c_void};
use std::mem;

use crate::io::{IoMutSlice, IoSlice};

// TODO: replace with `std::ffi::c_size_t` once stable
// <https://github.com/rust-lang/rust/issues/88345>.
#[allow(non_camel_case_types)]
type c_size_t = usize;

// From <sanitizer/asan_interface.h>.
#[cfg_attr(feature = "nightly", cfg(sanitize = "address"))]
#[cfg_attr(not(feature = "nightly"), cfg(false))]
unsafe extern "C" {
    fn __asan_poison_memory_region(addr: *const c_void, size: c_size_t);
    fn __asan_unpoison_memory_region(addr: *const c_void, size: c_size_t);
}

/// Mark a memory region as unaddressable.
pub(crate) fn poison_region(addr: *const c_void, size: c_size_t) {
    #[cfg_attr(feature = "nightly", cfg(sanitize = "address"))]
    #[cfg_attr(not(feature = "nightly"), cfg(false))]
    unsafe {
        __asan_poison_memory_region(addr, size);
    }
}

/// Mark memory storing `T` as unaddressable.
pub(crate) fn poison<T: Sized>(addr: *const T) {
    if mem::size_of::<T>() != 0 && !addr.is_null() {
        // Don't poison zero sized types.
        poison_region(addr.cast(), mem::size_of::<T>());
    }
}

/// Mark the bytes of iovecs as addressable.
pub(crate) fn poison_iovecs(iovecs: &[IoSlice]) {
    for iovec in iovecs {
        poison_region(unsafe { iovec.ptr().cast() }, iovec.len());
    }
}

/// Mark the bytes of iovecs as addressable.
pub(crate) fn poison_iovecs_mut(iovecs: &[IoMutSlice]) {
    for iovec in iovecs {
        poison_region(unsafe { iovec.ptr().cast() }, iovec.len());
    }
}

/// Mark memory storing `T` as unaddressable.
pub(crate) fn poison_cstring(value: &CString) {
    let value = value.as_bytes_with_nul();
    poison_region(value.as_ptr().cast(), value.len());
}

/// Mark a memory region as addressable.
pub(crate) fn unpoison_region(addr: *const c_void, size: c_size_t) {
    #[cfg_attr(feature = "nightly", cfg(sanitize = "address"))]
    #[cfg_attr(not(feature = "nightly"), cfg(false))]
    unsafe {
        __asan_unpoison_memory_region(addr, size);
    }
}

/// Mark memory storing `T` as addressable.
pub(crate) fn unpoison<T: Sized>(addr: *const T) {
    if mem::size_of::<T>() != 0 && !addr.is_null() {
        // We don't poison zero sized types, so don't unpoison them
        unpoison_region(addr.cast(), mem::size_of::<T>());
    }
}

/// Mark the bytes of iovecs as addressable.
pub(crate) fn unpoison_iovecs(iovecs: &[IoSlice]) {
    for iovec in iovecs {
        unpoison_region(unsafe { iovec.ptr().cast() }, iovec.len());
    }
}

/// Mark the bytes of iovecs as addressable.
pub(crate) fn unpoison_iovecs_mut(iovecs: &[IoMutSlice]) {
    for iovec in iovecs {
        unpoison_region(unsafe { iovec.ptr().cast() }, iovec.len());
    }
}

/// Mark memory storing `T` as addressable.
pub(crate) fn unpoison_cstring(value: &CString) {
    let value = value.as_bytes_with_nul();
    unpoison_region(value.as_ptr().cast(), value.len());
}

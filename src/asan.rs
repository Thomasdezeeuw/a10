//! Utilities for address sanitizer support.

// Let's make it easier with all the `cfg`s.
#![allow(unused_variables, dead_code)]

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
#[track_caller]
pub(crate) fn poison_region(addr: *const c_void, size: c_size_t) {
    /* For some unknown reason (un)poisoning AddressSanitizer does not work at
     * all. At this point I'm doing dealing with it and disabling it. Some
     * debugging notes:
     *  * AddressSanitizer complains about "use-after-poison" or "unknown-crash"
     *    on address.
     *  * It shows the helpful shadown bytes table, which indicates that the
     *    address is addressable (i.e. totally fine).
     *  * The (un)poisoning functions are not thread-safe, "because no two
     *    threads can poison or unpoison memory in the same memory region
     *    simultaneously" (llvm-project/compiler-rt/include/sanitizer/asan_interface.h).
     *    However, the logs (see below) indicate that the thread that poison the
     *    memory also unpoisons it, which makes senses as it's usually the same
     *    thread that polls the Future that does the (un)poisoning. At least
     *    this is true when testing, which also causes errors.
    // Don't poison zero sized types.
    if size != 0 && !addr.is_null() {
        #[cfg_attr(feature = "nightly", cfg(sanitize = "address"))]
        #[cfg_attr(not(feature = "nightly"), cfg(false))]
        unsafe {
            __asan_poison_memory_region(addr, size);
            log::trace!(
                "poisoned {addr:?}+{size}, from: {:?}@{}",
                std::thread::current().id(),
                std::panic::Location::caller(),
            );
        }
    }
    */
}

/// Mark memory storing `T` as unaddressable.
#[track_caller]
pub(crate) fn poison<T: Sized>(addr: *const T) {
    poison_region(addr.cast(), mem::size_of::<T>());
}

/// Mark the bytes of iovecs as addressable.
#[track_caller]
pub(crate) fn poison_iovecs(iovecs: &[IoSlice]) {
    for iovec in iovecs {
        poison_region(unsafe { iovec.ptr().cast() }, iovec.len());
    }
}

/// Mark the bytes of iovecs as addressable.
#[track_caller]
pub(crate) fn poison_iovecs_mut(iovecs: &[IoMutSlice]) {
    for iovec in iovecs {
        poison_region(unsafe { iovec.ptr().cast() }, iovec.len());
    }
}

/// Mark memory storing `T` as unaddressable.
#[track_caller]
pub(crate) fn poison_cstring(value: &CString) {
    let value = value.as_bytes_with_nul();
    poison_region(value.as_ptr().cast(), value.len());
}

/// Mark a memory region as addressable.
#[track_caller]
pub(crate) fn unpoison_region(addr: *const c_void, size: c_size_t) {
    /* See poison_region for why this is disabled.
    // We don't poison zero sized types, so don't unpoison them
    if size != 0 && !addr.is_null() {
        #[cfg_attr(feature = "nightly", cfg(sanitize = "address"))]
        #[cfg_attr(not(feature = "nightly"), cfg(false))]
        unsafe {
            __asan_unpoison_memory_region(addr, size);
            log::error!(
                "unpoisoned {addr:?}+{size}, from: {:?}@{}",
                std::thread::current().id(),
                std::panic::Location::caller(),
            );
        }
    }
    */
}

/// Mark memory storing `T` as addressable.
#[track_caller]
pub(crate) fn unpoison<T: Sized>(addr: *const T) {
    unpoison_region(addr.cast(), mem::size_of::<T>());
}

/// Mark the bytes of iovecs as addressable.
#[track_caller]
pub(crate) fn unpoison_iovecs(iovecs: &[IoSlice]) {
    for iovec in iovecs {
        unpoison_region(unsafe { iovec.ptr().cast() }, iovec.len());
    }
}

/// Mark the bytes of iovecs as addressable.
#[track_caller]
pub(crate) fn unpoison_iovecs_mut(iovecs: &[IoMutSlice]) {
    for iovec in iovecs {
        unpoison_region(unsafe { iovec.ptr().cast() }, iovec.len());
    }
}

/// Mark memory storing `T` as addressable.
#[track_caller]
pub(crate) fn unpoison_cstring(value: &CString) {
    let value = value.as_bytes_with_nul();
    unpoison_region(value.as_ptr().cast(), value.len());
}

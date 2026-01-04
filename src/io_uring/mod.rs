//! io_uring implementation.

use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

pub(crate) mod config;
pub(crate) mod cq;
pub(crate) mod io;
mod libc;
pub(crate) mod net;
pub(crate) mod op;
pub(crate) mod sq;

pub(crate) use config::Config;
pub(crate) use cq::Completions;
pub(crate) use sq::Submissions;

/// `mmap(2)` wrapper that also sets `MADV_DONTFORK`.
fn mmap(
    len: libc::size_t,
    prot: libc::c_int,
    flags: libc::c_int,
    fd: libc::c_int,
    offset: libc::off_t,
) -> io::Result<*mut libc::c_void> {
    let addr = match unsafe { libc::mmap(ptr::null_mut(), len, prot, flags, fd, offset) } {
        libc::MAP_FAILED => return Err(io::Error::last_os_error()),
        addr => addr,
    };

    match unsafe { libc::madvise(addr, len, libc::MADV_DONTFORK) } {
        0 => Ok(addr),
        _ => {
            let err = io::Error::last_os_error();
            _ = munmap(addr, len); // Can't handle two errors.
            Err(err)
        }
    }
}

/// `munmap(2)` wrapper.
pub(crate) fn munmap(addr: *mut libc::c_void, len: libc::size_t) -> io::Result<()> {
    match unsafe { libc::munmap(addr, len) } {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

/// Load a `u32` using relaxed ordering from `ptr`.
unsafe fn load_atomic_u32(ptr: *mut libc::c_void) -> u32 {
    unsafe { (*ptr.cast::<AtomicU32>()).load(Ordering::Acquire) }
}

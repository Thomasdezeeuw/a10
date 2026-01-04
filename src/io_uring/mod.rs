//! io_uring implementation.

use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::{ptr, task};

use crate::{asan, syscall};

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

/// Data shared between [`Submissions`] and [`Completions`].
#[derive(Debug)]
pub(crate) struct Shared {
    /// Pointer and length to the mmaped page(s).
    submission_ring: *mut libc::c_void,
    submission_ring_len: u32,
    // NOTE: the following fields reference mmaped pages shared with the kernel,
    // thus all need atomic/synchronised access.
    /// Flags set by the kernel to communicate information.
    kernel_flags: *const AtomicU32,
    /// Head of the submissions queue, i.e. the numbers of submissions read by
    /// the kernel. Incremented by the kernel when submissions has succesfully
    /// been processed.
    submissions_head: *const AtomicU32,
    /// Tail of the submissions queue, i.e. the number of submissions we've
    /// submitted to the kernel.
    submissions_tail: *const AtomicU32,
    /// Array of [`Shared::submissions_len`] submission entries shared with the
    /// kernel. We're the only one modifiying the structures, but the kernel can
    /// read from them.
    submissions: *mut sq::Submission,
    // Fixed values that don't change after the setup.
    /// Length of [`Shared::submissions`].
    submissions_len: u32,
    /// True if we're using a kernel thread to do submission polling, i.e. if
    /// `IORING_SETUP_SQPOLL` is enabled.
    kernel_thread: bool,
    /// Boolean indicating a thread is [`Ring::poll`]ing.
    is_polling: AtomicBool,
    /// Futures that are waiting for a slot in `queued_ops`.
    blocked_futures: Mutex<Vec<task::Waker>>,
    /// File descriptor of the io_uring.
    /// NOTE: must come last to ensure it's dropped (closed) last.
    rfd: OwnedFd,
}

impl Shared {
    pub(crate) fn new(rfd: OwnedFd, parameters: &libc::io_uring_params) -> io::Result<Shared> {
        let submission_ring_len =
            parameters.sq_off.array + parameters.sq_entries * (size_of::<libc::__u32>() as u32);
        let submission_ring = mmap(
            submission_ring_len as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_POPULATE,
            rfd.as_raw_fd(),
            libc::off_t::from(libc::IORING_OFF_SQ_RING),
        )?;

        let submissions_len = parameters.sq_entries as usize * size_of::<sq::Submission>();
        let submissions_ptr = mmap(
            submissions_len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_POPULATE,
            rfd.as_raw_fd(),
            libc::off_t::from(libc::IORING_OFF_SQES),
        )
        .inspect_err(|_| {
            _ = munmap(submission_ring, submission_ring_len as usize); // Can't handle two errors.
        })?;
        // Same as what we did for the submission array, but this time with the
        // actual submissions.
        asan::poison_region(submissions_ptr.cast(), submissions_len);

        // SAFETY: we do a whole bunch of pointer manipulations, the kernel
        // ensures all of this stuff is set up for us with the mmap calls above.
        Ok(Shared {
            submission_ring,
            submission_ring_len,
            kernel_flags: unsafe { submission_ring.add(parameters.sq_off.flags as usize).cast() },
            submissions_head: unsafe {
                submission_ring.add(parameters.sq_off.head as usize).cast()
            },
            submissions_tail: unsafe {
                submission_ring.add(parameters.sq_off.tail as usize).cast()
            },
            submissions: submissions_ptr.cast(),
            submissions_len: parameters.sq_entries,
            kernel_thread: (parameters.flags & libc::IORING_SETUP_SQPOLL) != 0,
            is_polling: AtomicBool::new(false),
            blocked_futures: Mutex::new(Vec::new()),
            rfd,
        })
    }

    /// Make a `io_uring_register(2)` system call.
    pub(crate) fn register(
        &self,
        op: libc::c_uint,
        arg: *const libc::c_void,
        nr_args: libc::c_uint,
    ) -> io::Result<()> {
        syscall!(io_uring_register(self.rfd.as_raw_fd(), op, arg, nr_args))?;
        Ok(())
    }
}

unsafe impl Send for Shared {}

unsafe impl Sync for Shared {}

impl Drop for Shared {
    fn drop(&mut self) {
        let ptr = self.submissions.cast();
        let len = (self.submissions_len as usize) * size_of::<sq::Submission>();
        asan::unpoison_region(ptr, len);
        if let Err(err) = munmap(ptr, len) {
            log::warn!(ptr:? = ptr, len = len; "error unmapping io_uring submissions: {err}");
        }

        let ptr = self.submission_ring;
        let len = self.submission_ring_len as usize;
        asan::unpoison_region(ptr, len);
        if let Err(err) = munmap(ptr, len) {
            log::warn!(ptr:? = ptr, len = len; "error unmapping io_uring submission ring: {err}");
        }
    }
}

fn load_kernel_shared(ptr: *const AtomicU32) -> u32 {
    // SAFETY: since the value is shared with the kernel we need to use Acquire
    // memory ordering.
    unsafe { (*ptr).load(Ordering::Acquire) }
}

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

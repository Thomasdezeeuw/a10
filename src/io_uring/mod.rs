//! io_uring implementation.
//!
//! Manuals:
//! * <https://man7.org/linux/man-pages/man7/io_uring.7.html>

use std::cmp::min;
use std::mem::{drop as unlock, swap, take};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use std::{ptr, task};

use crate::{PollingState, asan, lock, syscall, try_lock};

pub(crate) mod config;
pub(crate) mod cq;
pub(crate) mod fd;
pub(crate) mod fs;
pub(crate) mod io;
mod libc;
pub(crate) mod mem;
pub(crate) mod net;
pub(crate) mod op;
pub(crate) mod pipe;
pub(crate) mod poll;
pub(crate) mod process;
pub(crate) mod sq;

pub(crate) use config::Config;
pub(crate) use cq::Completions;
pub(crate) use sq::Submissions;

/// io_uring specific methods.
impl crate::Ring {
    /// Enable the ring.
    ///
    /// This only required when starting the ring in disabled mode, see
    /// [`Config::disable`].
    ///
    /// [`Config::disable`]: crate::Config::disable
    #[allow(clippy::needless_pass_by_ref_mut)]
    #[doc(alias = "IORING_REGISTER_ENABLE_RINGS")]
    pub fn enable(&mut self) -> io::Result<()> {
        self.sq
            .shared()
            .register(libc::IORING_REGISTER_ENABLE_RINGS, ptr::null(), 0)
    }
}

/// Data shared between [`Submissions`] and [`Completions`].
#[derive(Debug)]
pub(crate) struct Shared {
    /// Pointer and length to the mmaped page(s).
    submission_ring: ptr::NonNull<libc::c_void>,
    submission_ring_len: u32,
    // NOTE: the following fields reference mmaped pages shared with the kernel,
    // thus all need atomic/synchronised access.
    /// Flags set by the kernel to communicate information.
    kernel_flags: ptr::NonNull<AtomicU32>,
    /// Head of the submissions queue, i.e. the numbers of submissions read by
    /// the kernel. Incremented by the kernel when submissions has succesfully
    /// been processed.
    submissions_head: ptr::NonNull<AtomicU32>,
    /// Tail of the submissions queue, i.e. the number of submissions we've
    /// submitted to the kernel.
    submissions_tail: ptr::NonNull<AtomicU32>,
    /// Array of [`Shared::submissions_len`] submission entries shared with the
    /// kernel. We're the only one modifiying the structures, but the kernel can
    /// read from them.
    submissions: ptr::NonNull<sq::Submission>,
    /// Lock to submit the next submission.
    submissions_lock: Mutex<()>,
    // Fixed values that don't change after the setup.
    /// Length of [`Shared::submissions`].
    submissions_len: u32,
    /// True if we're using a kernel thread to do submission polling, i.e. if
    /// `IORING_SETUP_SQPOLL` is enabled.
    kernel_thread: bool,
    /// True if only a single thread can submit submissions, i.e. if
    /// `IORING_SETUP_SINGLE_ISSUER` is enabled.
    single_issuer: bool,
    polling: PollingState,
    /// Futures that are waiting for a slot in submissions.
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
        asan::poison_region(submissions_ptr.cast().as_ptr(), submissions_len);

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
            submissions_lock: Mutex::new(()),
            submissions_len: parameters.sq_entries,
            kernel_thread: (parameters.flags & libc::IORING_SETUP_SQPOLL) != 0,
            single_issuer: (parameters.flags & libc::IORING_SETUP_SINGLE_ISSUER) != 0,
            polling: PollingState::new(),
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

    /// Make a `io_uring_enter2(2)` system call.
    pub(crate) fn enter(
        &self,
        min_complete: libc::c_uint,
        mut flags: libc::c_uint,
        timeout: Option<Duration>,
    ) -> io::Result<u32> {
        let mut args = libc::io_uring_getevents_arg {
            sigmask: 0,
            sigmask_sz: 0,
            min_wait_usec: 0,
            ts: 0,
        };
        let mut timespec = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        if let Some(timeout) = timeout {
            timespec.tv_sec = timeout.as_secs().try_into().unwrap_or(i64::MAX);
            timespec.tv_nsec = libc::c_longlong::from(timeout.subsec_nanos());
            args.ts = ptr::from_ref(&timespec).addr() as u64;
        }

        let submissions = if self.kernel_thread {
            // If needed wake up the kernel thread.
            if load_kernel_shared(self.kernel_flags) & libc::IORING_SQ_NEED_WAKEUP != 0 {
                flags |= libc::IORING_ENTER_SQ_WAKEUP;
            }
            0 // Kernel thread handles the submissions.
        } else {
            self.unsubmitted_submissions()
        };

        log::trace!(submissions, timeout:?; "entering kernel");
        let result = syscall!(io_uring_enter2(
            self.ring_fd(),
            submissions,
            min_complete,
            flags | libc::IORING_ENTER_EXT_ARG, // Passing of `args`.
            ptr::from_ref(&args).cast(),
            size_of::<libc::io_uring_getevents_arg>(),
        ));
        match result {
            Ok(n) => {
                self.wake_blocked_futures();
                Ok(n.cast_unsigned())
            }
            // Hit a timeout or got interrupted, we can ignore it.
            Err(ref err) if matches!(err.raw_os_error(), Some(libc::ETIME | libc::EINTR)) => Ok(0),
            Err(err) => Err(err),
        }
    }

    /// Wake any futures that were blocked on a submission slot.
    pub(crate) fn wake_blocked_futures(&self) {
        // Only wake up futures if a submission slot is available for them.
        let available = (self.submissions_len - self.unsubmitted_submissions()) as usize;
        if available == 0 {
            return;
        }

        let Some(mut blocked_futures) = try_lock(&self.blocked_futures) else {
            return;
        };
        if blocked_futures.is_empty() {
            // No futures to wake up.
            return;
        }
        let mut wakers = take(&mut *blocked_futures);
        unlock(blocked_futures); // Unblock others.
        let awoken = min(available, wakers.len());
        for waker in wakers.drain(..awoken) {
            log::trace!(waker:?; "waking up future for submission");
            waker.wake();
        }

        // Reuse allocation.
        let mut blocked_futures = lock(&self.blocked_futures);
        swap(&mut *blocked_futures, &mut wakers);
        // Add back any wakers for which we don't have a slot.
        let awoken = min(available - awoken, wakers.len());
        blocked_futures.extend(wakers.drain(wakers.len() - awoken..));
        unlock(blocked_futures); // Unblock others.
        for waker in wakers {
            log::trace!(waker:?; "waking up future for submission");
            waker.wake();
        }
    }

    /// Returns the number of unsumitted submission queue entries.
    pub(crate) fn unsubmitted_submissions(&self) -> u32 {
        // NOTE: we MUST load the head before the tail to ensure the head is
        // ALWAYS older. Otherwise it's possible for the subtraction to
        // underflow.
        let head = load_kernel_shared(self.submissions_head);
        let tail = load_kernel_shared(self.submissions_tail);
        tail - head
    }

    fn ring_fd(&self) -> RawFd {
        self.rfd.as_raw_fd()
    }
}

unsafe impl Send for Shared {}

unsafe impl Sync for Shared {}

impl Drop for Shared {
    fn drop(&mut self) {
        let ptr = self.submissions.cast();
        let len = (self.submissions_len as usize) * size_of::<sq::Submission>();
        asan::unpoison_region(ptr.as_ptr(), len);
        if let Err(err) = munmap(ptr, len) {
            log::warn!(ptr:?, len; "error unmapping io_uring submissions: {err}");
        }

        let ptr = self.submission_ring;
        let len = self.submission_ring_len as usize;
        asan::unpoison_region(ptr.as_ptr(), len);
        if let Err(err) = munmap(ptr, len) {
            log::warn!(ptr:?, len; "error unmapping io_uring submission ring: {err}");
        }
    }
}

fn load_kernel_shared(ptr: ptr::NonNull<AtomicU32>) -> u32 {
    // SAFETY: since the value is shared with the kernel we need to use Acquire
    // memory ordering.
    unsafe { (*ptr.as_ptr()).load(Ordering::Acquire) }
}

/// `mmap(2)` wrapper that also sets `MADV_DONTFORK`.
fn mmap(
    len: libc::size_t,
    prot: libc::c_int,
    flags: libc::c_int,
    fd: libc::c_int,
    offset: libc::off_t,
) -> io::Result<ptr::NonNull<libc::c_void>> {
    let addr = match unsafe { libc::mmap(ptr::null_mut(), len, prot, flags, fd, offset) } {
        libc::MAP_FAILED => return Err(io::Error::last_os_error()),
        // SAFETY: mmap ensure the pointer is not null.
        addr => unsafe { ptr::NonNull::new_unchecked(addr) },
    };

    match unsafe { libc::madvise(addr.as_ptr(), len, libc::MADV_DONTFORK) } {
        0 => Ok(addr),
        _ => {
            let err = io::Error::last_os_error();
            _ = munmap(addr, len); // Can't handle two errors.
            Err(err)
        }
    }
}

/// `munmap(2)` wrapper.
pub(crate) fn munmap(addr: ptr::NonNull<libc::c_void>, len: libc::size_t) -> io::Result<()> {
    match unsafe { libc::munmap(addr.as_ptr(), len) } {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

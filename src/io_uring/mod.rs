//! io_uring implementation.

use std::os::fd::{AsRawFd, OwnedFd};
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::drop_waker::DropWake;
use crate::op::OpResult;
use crate::{AsyncFd, asan, syscall};

pub(crate) mod cancel;
pub(crate) mod config;
mod cq;
pub(crate) mod fd;
pub(crate) mod fs;
pub(crate) mod io;
mod libc;
pub(crate) mod mem;
pub(crate) mod msg;
pub(crate) mod net;
pub(crate) mod pipe;
pub(crate) mod poll;
pub(crate) mod process;
mod sq;

pub(crate) use config::Config;
pub(crate) use cq::Completions;
pub(crate) use sq::{Submission, Submissions};

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
        self.cq
            .shared()
            .data
            .register(libc::IORING_REGISTER_ENABLE_RINGS, ptr::null(), 0)
    }
}

/// io_uring implementation.
pub(crate) enum Implementation {}

impl crate::Implementation for Implementation {
    type Shared = Shared;
    type Submissions = Submissions;
    type Completions = Completions;
}

#[derive(Debug)]
#[allow(clippy::mutex_integer)]
pub(crate) struct Shared {
    /// File descriptor of the io_uring.
    rfd: OwnedFd,
    /// Mmap-ed pointer.
    ptr: *mut libc::c_void,
    /// Mmap-ed size in bytes.
    size: libc::c_uint,
    /// Increased in `queue` to give the caller mutable access to a
    /// submission in [`Submissions`].
    /// Used by [`Completions`] to determine the number of submissions to
    /// submit.
    pending_tail: AtomicU32,
    /// True if we're using a kernel thread to do submission polling, i.e. if
    /// `IORING_SETUP_SQPOLL` is enabled.
    kernel_thread: bool,
    // NOTE: the following fields reference mmaped pages shared with the kernel,
    // thus all need atomic/synchronised access.
    /// Flags set by the kernel to communicate state information.
    flags: *const AtomicU32,
    /// Head to queue, i.e. the submussions read by the kernel. Incremented by
    /// the kernel when submissions has succesfully been processed.
    kernel_read: *const AtomicU32,
    /// Array of `len` submission entries shared with the kernel. We're the only
    /// one modifiying the structures, but the kernel can read from them.
    ///
    /// This pointer is also used in the `unmmap` call.
    entries: *mut sq::Submission,
    /// Number of `entries`.
    entries_len: u32,
    /// Mask used to index into the `entries` queue.
    entries_mask: u32,
    /// Variable used to get an index into `array`. The lock must be held while
    /// writing into `array` to prevent race conditions with other threads.
    array_index: Mutex<u32>,
    /// Array of `len` indices (into `entries`) shared with the kernel. We're
    /// the only one modifiying the structures, but the kernel can read from it.
    ///
    /// This is protected by `array_index`.
    array: *mut AtomicU32,
    /// Incremented by us when submitting new submissions.
    array_tail: *mut AtomicU32,
}

impl Shared {
    pub(crate) fn new(rfd: OwnedFd, parameters: &libc::io_uring_params) -> io::Result<Shared> {
        let submission_queue_size =
            parameters.sq_off.array + parameters.sq_entries * (size_of::<libc::__u32>() as u32);
        let submission_queue = mmap(
            submission_queue_size as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_POPULATE,
            rfd.as_raw_fd(),
            libc::off_t::from(libc::IORING_OFF_SQ_RING),
        )?;
        // We poison the entire allocation and then unpoison on a per submission
        // basis (see Submissions::add) when we get write something to it.
        asan::poison_region(submission_queue.cast(), submission_queue_size as usize);

        let submission_queue_entries_size =
            parameters.sq_entries as usize * size_of::<libc::io_uring_sqe>();
        let submission_queue_entries = mmap(
            submission_queue_entries_size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_POPULATE,
            rfd.as_raw_fd(),
            libc::off_t::from(libc::IORING_OFF_SQES),
        )
        .inspect_err(|_| {
            _ = munmap(submission_queue, submission_queue_size as usize); // Can't handle two errors.
        })?;
        // Same as what we did for the submission queue, but this time with the entries.
        asan::poison_region(
            submission_queue_entries.cast(),
            submission_queue_entries_size,
        );

        let entries_len = unsafe {
            load_atomic_u32(submission_queue.add(parameters.sq_off.ring_entries as usize))
        };
        debug_assert!(entries_len == parameters.sq_entries);
        let entries_mask =
            unsafe { load_atomic_u32(submission_queue.add(parameters.sq_off.ring_mask as usize)) };
        debug_assert!(entries_mask == parameters.sq_entries - 1);

        // SAFETY: we do a whole bunch of pointer manipulations, the kernel
        // ensures all of this stuff is set up for us with the mmap calls above.
        #[allow(clippy::mutex_integer)] // For `array_index`, need to the lock for more.
        Ok(unsafe {
            Shared {
                rfd,
                ptr: submission_queue,
                size: submission_queue_size,
                pending_tail: AtomicU32::new(0),
                kernel_thread: (parameters.flags & libc::IORING_SETUP_SQPOLL) != 0,
                // Fields are shared with the kernel.
                kernel_read: submission_queue.add(parameters.sq_off.head as usize).cast(),
                flags: submission_queue
                    .add(parameters.sq_off.flags as usize)
                    .cast(),
                entries: submission_queue_entries.cast(),
                entries_len,
                entries_mask,
                array_index: Mutex::new(0),
                array: submission_queue
                    .add(parameters.sq_off.array as usize)
                    .cast(),
                array_tail: submission_queue.add(parameters.sq_off.tail as usize).cast(),
            }
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

    /// Wake up the kernel thread polling for submission events, if the kernel
    /// thread needs a wakeup.
    fn maybe_wake_kernel_thread(&self) {
        if self.kernel_thread && (self.flags() & libc::IORING_SQ_NEED_WAKEUP != 0) {
            log::debug!("waking io_uring submission queue polling kernel thread");
            let res = syscall!(io_uring_enter2(
                self.rfd.as_raw_fd(),
                0,                            // We've already queued our submissions.
                0,                            // Don't wait for any completion events.
                libc::IORING_ENTER_SQ_WAKEUP, // Wake up the kernel.
                ptr::null(),                  // We don't pass any additional arguments.
                0,
            ));
            if let Err(err) = res {
                log::warn!("failed to wake io_uring submission queue polling kernel thread: {err}");
            }
        }
    }

    /// Submit the event to the kernel when not using a kernel polling thread
    /// and another thread is currently [`Ring::poll`]ing.
    ///
    /// [`Ring::poll`]: crate::Ring::poll
    fn maybe_submit_event(&self, is_polling: &AtomicBool) {
        if !self.kernel_thread && is_polling.load(Ordering::Acquire) {
            log::debug!("submitting submission event while another thread is `Ring::poll`ing");
            let rfd = self.rfd.as_raw_fd();
            let res = syscall!(io_uring_enter2(rfd, 1, 0, 0, ptr::null(), 0));
            if let Err(err) = res {
                log::warn!("failed to io_uring submit event: {err}");
            }
        }
    }

    /// Returns the number of unsumitted submission queue entries.
    pub(crate) fn unsubmitted(&self) -> u32 {
        // NOTE: we MUST load the head before the tail to ensure the head is
        // ALWAYS older. Otherwise it's possible for the subtraction to
        // underflow.
        let head = self.kernel_read();
        let tail = self.pending_tail();
        tail - head
    }

    /// Returns `self.kernel_read`.
    fn kernel_read(&self) -> u32 {
        // SAFETY: this written to by the kernel so we need to use `Acquire`
        // ordering. The pointer itself is valid as long as `Ring.fd` is alive.
        unsafe { (*self.kernel_read).load(Ordering::Acquire) }
    }

    /// Returns `self.pending_tail`.
    fn pending_tail(&self) -> u32 {
        // SAFETY: to sync with other threads use `Acquire` ordering.
        self.pending_tail.load(Ordering::Acquire)
    }

    /// Returns `self.flags`.
    fn flags(&self) -> u32 {
        // SAFETY: this written to by the kernel so we need to use `Acquire`
        // ordering. The pointer itself is valid as long as `Ring.fd` is alive.
        unsafe { (*self.flags).load(Ordering::Acquire) }
    }
}

unsafe impl Send for Shared {}

unsafe impl Sync for Shared {}

impl Drop for Shared {
    fn drop(&mut self) {
        let ptr = self.entries.cast();
        let size = self.entries_len as usize * size_of::<sq::Submission>();
        asan::unpoison_region(ptr, size);
        if let Err(err) = munmap(ptr, size) {
            log::warn!(ptr:? = ptr, size = size; "error unmapping io_uring entries: {err}");
        }

        asan::unpoison_region(self.ptr, self.size as usize);
        if let Err(err) = munmap(self.ptr, self.size as usize) {
            log::warn!(ptr:? = self.ptr, size = self.size; "error unmapping io_uring submission queue: {err}");
        }
    }
}

/// io_uring specific [`crate::op::Op`] trait.
pub(crate) trait Op {
    type Output;
    type Resources: DropWake;
    type Args;

    fn fill_submission(
        resources: &mut Self::Resources,
        args: &mut Self::Args,
        submission: &mut sq::Submission,
    );

    fn map_ok(
        sq: &crate::SubmissionQueue,
        resources: Self::Resources,
        op_output: cq::OpReturn,
    ) -> Self::Output;
}

impl<T: Op> crate::op::Op for T {
    type Output = T::Output;
    type Resources = T::Resources;
    type Args = T::Args;
    type Submission = sq::Submission;
    type OperationState = cq::OperationState;
    type OperationOutput = cq::OpReturn;

    fn fill_submission(
        resources: &mut Self::Resources,
        args: &mut Self::Args,
        submission: &mut Self::Submission,
    ) {
        T::fill_submission(resources, args, submission);
    }

    fn check_result(
        _: &mut Self::Resources,
        _: &mut Self::Args,
        state: &mut Self::OperationState,
    ) -> OpResult<Self::OperationOutput> {
        match state {
            cq::OperationState::Single { result } => result.as_op_return(),
            cq::OperationState::Multishot { results } if results.is_empty() => {
                OpResult::Again(false)
            }
            cq::OperationState::Multishot { results } => results.remove(0).as_op_return(),
        }
    }

    fn map_ok(
        sq: &crate::SubmissionQueue,
        resources: Self::Resources,
        op_output: Self::OperationOutput,
    ) -> Self::Output {
        T::map_ok(sq, resources, op_output)
    }
}

/// io_uring specific [`crate::op::FdOp`] trait.
pub(crate) trait FdOp {
    type Output;
    type Resources: DropWake;
    type Args;

    fn fill_submission(
        fd: &AsyncFd,
        resources: &mut Self::Resources,
        args: &mut Self::Args,
        submission: &mut sq::Submission,
    );

    fn map_ok(fd: &AsyncFd, resources: Self::Resources, op_output: cq::OpReturn) -> Self::Output;
}

impl<T: FdOp> crate::op::FdOp for T {
    type Output = T::Output;
    type Resources = T::Resources;
    type Args = T::Args;
    type Submission = sq::Submission;
    type OperationState = cq::OperationState;
    type OperationOutput = cq::OpReturn;

    fn fill_submission(
        fd: &AsyncFd,
        resources: &mut Self::Resources,
        args: &mut Self::Args,
        submission: &mut Self::Submission,
    ) {
        T::fill_submission(fd, resources, args, submission);
    }

    fn check_result(
        _: &AsyncFd,
        _: &mut Self::Resources,
        _: &mut Self::Args,
        state: &mut Self::OperationState,
    ) -> OpResult<Self::OperationOutput> {
        match state {
            cq::OperationState::Single { result } => result.as_op_return(),
            cq::OperationState::Multishot { results } if results.is_empty() => {
                OpResult::Again(false)
            }
            cq::OperationState::Multishot { results } => results.remove(0).as_op_return(),
        }
    }

    fn map_ok(
        fd: &AsyncFd,
        resources: Self::Resources,
        op_output: Self::OperationOutput,
    ) -> Self::Output {
        T::map_ok(fd, resources, op_output)
    }
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

/// Load a `u32` using relaxed ordering from `ptr`.
unsafe fn load_atomic_u32(ptr: *mut libc::c_void) -> u32 {
    unsafe { (*ptr.cast::<AtomicU32>()).load(Ordering::Acquire) }
}

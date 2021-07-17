use std::mem::size_of;
use std::os::unix::io::RawFd;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::{io, ptr};

use crate::{libc, CompletionQueue, Ring, SubmissionQueue};

/// Configuration for a [`Ring`].
#[derive(Debug)]
pub struct Config {
    submission_entries: u32,
    completion_entries: Option<u32>,
    clamp: bool,
}

impl Config {
    pub(crate) const fn new(entries: u32) -> Config {
        Config {
            submission_entries: entries,
            completion_entries: None,
            clamp: false,
        }
    }

    /// Set the size of the completion queue.
    ///
    /// By default the kernel will use a completion twice as large as the
    /// submission queue (`entries` in the call to [`Config::new`]).
    ///
    /// Uses `IORING_SETUP_CQSIZE`, added in Linux kernel 5.5.
    #[doc(alias = "IORING_SETUP_CQSIZE")]
    pub fn with_completion_queue_size(mut self, entries: u32) -> Self {
        self.completion_entries = Some(entries);
        self
    }

    /// Clamp queue sizes to the maximum.
    ///
    /// The maximum queue sizes aren't exposed by the kernel, making this the
    /// only way (currently) to get the largest possible queues.
    ///
    /// Uses `IORING_SETUP_CLAMP`, added in Linux kernel 5.6.
    #[doc(alias = "IORING_SETUP_CLAMP")]
    pub fn clamp_queue_sizes(mut self) -> Self {
        self.clamp = true;
        self
    }

    /*
    /// Perform busy-waiting for an I/O completion, as opposed to getting
    /// notifications via an asynchronous IRQ (Interrupt Request). The file
    /// system (if any) and block device must support polling in order for this
    /// to work.
    ///
    /// Busy-waiting provides lower latency, but may consume more CPU resources
    /// than interrupt driven I/O. Currently, this feature is usable only on a
    /// file descriptor opened using the `O_DIRECT` flag. When a read or write
    /// is submitted to a polled context, the application must poll for
    /// completions on the CQ ring by calling [`Ring::sumbit`]. It is illegal to
    /// mix and match polled and non-polled I/O on an io_uring instance.
    ///
    /// Uses `IORING_SETUP_IOPOLL`, added in Linux kernel 5.1.
    #[doc(alias = "IORING_SETUP_IOPOLL")]
    pub fn io_polling(mut self) -> Self {
        todo!("Config::io_polling")
    }
    */

    /*
     * TODO: document the required permissions.
     * TODO: read the manual carefull and see what changes are required.
     * TODO: add support for `IORING_SETUP_SQ_AFF` flag as well.
    /// When this flag is specified, a kernel thread is created to perform
    /// submission queue polling. A [`Ring`] instance configured in this way
    /// enables an application to issue I/O without ever context switching into
    /// the kernel.
    ///
    /// Uses `IORING_SETUP_SQPOLL`, added in Linux kernel 5.1.
    #[doc(alias = "IORING_SETUP_SQPOLL")]
    pub fn with_kernel_polling(mut self) -> Self {
        todo!("Config::with_kernel_thread")
    }
    */

    // TODO: add method for `IORING_SETUP_ATTACH_WQ`, required access to the fd
    // in an existing `Ring`.

    // TODO: add method for `IORING_SETUP_R_DISABLED`.

    /// Build a new [`Ring`].
    #[doc(alias = "io_uring_setup")]
    pub fn build(self) -> io::Result<Ring> {
        let mut parameters = libc::io_uring_params::default();
        if let Some(completion_entries) = self.completion_entries {
            parameters.cq_entries = completion_entries;
            parameters.flags |= libc::IORING_SETUP_CQSIZE;
        }
        if self.clamp {
            parameters.flags |= libc::IORING_SETUP_CLAMP;
        }

        let fd = syscall!(io_uring_setup(self.submission_entries, &mut parameters))?;
        // TODO: check the `parameters.features` flags set by the kernel.

        // FIXME: close `fd` on error.
        let sq = mmap_submission_queue(fd, &parameters)?;
        // FIXME: close `fd` on error.
        let cq = mmap_completion_queue(fd, &parameters)?;
        let submission_tail = sq.pending_tail.load(Ordering::Relaxed);
        Ok(Ring {
            sq: Arc::new(sq),
            cq,
            submission_tail,
        })
    }
}

/// Memory-map the submission queue.
fn mmap_submission_queue(
    ring_fd: RawFd,
    parameters: &libc::io_uring_params,
) -> io::Result<SubmissionQueue> {
    let size = parameters.sq_off.array + parameters.sq_entries * (size_of::<libc::__u32>() as u32);

    let submission_queue = mmap(
        size as usize,
        libc::PROT_READ | libc::PROT_WRITE,
        libc::MAP_SHARED | libc::MAP_POPULATE,
        ring_fd,
        libc::IORING_OFF_SQ_RING as libc::off_t,
    )?;

    // FIXME: unmap `submission_queue` on error.
    let submission_queue_entries = mmap(
        parameters.sq_entries as usize * size_of::<libc::io_uring_sqe>(),
        libc::PROT_READ | libc::PROT_WRITE,
        libc::MAP_SHARED | libc::MAP_POPULATE,
        ring_fd,
        libc::IORING_OFF_SQES as libc::off_t,
    )?;

    unsafe {
        Ok(SubmissionQueue {
            ring_fd,
            ptr: submission_queue,
            size,
            // Fields are constant, so we load them once.
            len: load_atomic_u32(submission_queue.add(parameters.sq_off.ring_entries as usize)),
            ring_mask: load_atomic_u32(submission_queue.add(parameters.sq_off.ring_mask as usize)),
            pending_tail: AtomicU32::new(0),
            pending_index: AtomicU32::new(0),
            // Fields are shared with the kernel.
            head: submission_queue.add(parameters.sq_off.head as usize).cast(),
            tail: submission_queue.add(parameters.sq_off.tail as usize).cast(),
            dropped: submission_queue
                .add(parameters.sq_off.dropped as usize)
                .cast(),
            entries: submission_queue_entries.cast(),
            array: submission_queue
                .add(parameters.sq_off.array as usize)
                .cast(),
        })
    }
}

/// Memory-map the completion queue.
fn mmap_completion_queue(
    ring_fd: RawFd,
    parameters: &libc::io_uring_params,
) -> io::Result<CompletionQueue> {
    let size =
        parameters.cq_off.cqes + parameters.cq_entries * (size_of::<libc::io_uring_cqe>() as u32);

    let completion_queue = mmap(
        size as usize,
        libc::PROT_READ | libc::PROT_WRITE,
        libc::MAP_SHARED | libc::MAP_POPULATE,
        ring_fd,
        libc::IORING_OFF_CQ_RING as libc::off_t,
    )?;

    unsafe {
        Ok(CompletionQueue {
            ptr: completion_queue,
            size,
            // Fields are constant, so we load them once.
            len: load_atomic_u32(completion_queue.add(parameters.cq_off.ring_entries as usize)),
            ring_mask: load_atomic_u32(completion_queue.add(parameters.cq_off.ring_mask as usize)),
            // Fields are shared with the kernel.
            head: completion_queue.add(parameters.cq_off.head as usize).cast(),
            tail: completion_queue.add(parameters.cq_off.tail as usize).cast(),
            overflow: completion_queue
                .add(parameters.cq_off.overflow as usize)
                .cast(),
            entries: completion_queue.add(parameters.cq_off.cqes as usize).cast(),
        })
    }
}

/// `mmap(2)` wrapper.
fn mmap(
    len: libc::size_t,
    prot: libc::c_int,
    flags: libc::c_int,
    fd: libc::c_int,
    offset: libc::off_t,
) -> io::Result<*mut libc::c_void> {
    // FIXME: use `MADV_DONTFORK`.
    match unsafe { libc::mmap(ptr::null_mut(), len, prot, flags, fd, offset) } {
        libc::MAP_FAILED => Err(io::Error::last_os_error()),
        ptr => Ok(ptr),
    }
}

/// Load a `u32` using relaxed ordering from `ptr`.
unsafe fn load_atomic_u32(ptr: *mut libc::c_void) -> u32 {
    (&*ptr.cast::<AtomicU32>()).load(Ordering::Relaxed)
}

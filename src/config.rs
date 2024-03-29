//! Configuration of a [`Ring`].

use std::mem::{self, size_of};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{io, ptr};

use crate::{libc, AtomicBitMap, CompletionQueue, Ring, SharedSubmissionQueue, SubmissionQueue};

/// Configuration of a [`Ring`].
///
/// Created by calling [`Ring::config`].
#[derive(Debug, Clone)]
#[must_use = "no ring is created until `a10::Config::build` is called"]
pub struct Config<'r> {
    submission_entries: u32,
    completion_entries: Option<u32>,
    clamp: bool,
    kernel_thread: bool,
    cpu_affinity: Option<u32>,
    idle_timeout: Option<u32>,
    attach: Option<&'r SubmissionQueue>,
}

macro_rules! check_feature {
    ($features: expr, $required: ident $(,)?) => {{
        assert!(
            $features & libc::$required != 0,
            concat!(
                "Kernel doesn't have required `",
                stringify!($required),
                "` feature"
            )
        );
    }};
}

macro_rules! remove_flag {
    ($parameters: ident, $first_err: ident, $err: ident, $( $flag: ident, )+ ) => {
        $(
        if $parameters.flags & libc::$flag != 0 {
            log::debug!(concat!("failed to create io_uring: {}, dropping ", stringify!($flag), " flag and trying again"), $err);
            $parameters.flags &= !libc::$flag;
            $first_err.get_or_insert($err);
            continue;
        }
        )+
    };
}

impl<'r> Config<'r> {
    /// Create a new `Config`.
    pub(crate) const fn new(entries: u32) -> Config<'r> {
        Config {
            submission_entries: entries,
            completion_entries: None,
            clamp: false,
            kernel_thread: true,
            cpu_affinity: None,
            idle_timeout: None,
            attach: None,
        }
    }

    /// Set the size of the completion queue.
    ///
    /// By default the kernel will use a completion queue twice as large as the
    /// submission queue (`entries` in the call to [`Ring::config`]).
    ///
    /// Uses `IORING_SETUP_CQSIZE`, added in Linux kernel 5.5.
    #[doc(alias = "IORING_SETUP_CQSIZE")]
    pub const fn with_completion_queue_size(mut self, entries: u32) -> Self {
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
    pub const fn clamp_queue_sizes(mut self) -> Self {
        self.clamp = true;
        self
    }

    /// Start a kernel thread polling the [`Ring`].
    ///
    /// When this option is enabled a kernel thread is created to perform
    /// submission queue polling. This allows issuing I/O without ever context
    /// switching into the kernel.
    ///
    /// # Notes
    ///
    /// When setting this to false it significantly changes the way A10 works.
    /// With this disabled you need to call [`Ring::poll`] to *submit* I/O work,
    /// with this enables this is done by the kernel thread. That means that if
    /// multiple threads use the same [`SubmissionQueue`] their submissions
    /// might not actually be submitted until `Ring::poll` is called.
    #[doc(alias = "IORING_SETUP_SQPOLL")]
    pub const fn with_kernel_thread(mut self, enabled: bool) -> Self {
        self.kernel_thread = enabled;
        self
    }

    /// Set the CPU affinity of kernel thread polling the [`Ring`].
    ///
    /// Only works in combination with [`Config::with_kernel_thread`].
    #[doc(alias = "IORING_SETUP_SQ_AFF")]
    #[doc(alias = "sq_thread_cpu")]
    pub const fn with_cpu_affinity(mut self, cpu: u32) -> Self {
        self.cpu_affinity = Some(cpu);
        self
    }

    /// Set the idle timeout of the kernel thread polling the submission queue.
    /// After `timeout` time has passed after the last I/O submission the kernel
    /// thread will go to sleep. If the I/O is kept busy the kernel thread will
    /// never sleep. Note that A10 will ensure the kernel thread is woken up
    /// when more submissions are added.
    ///
    /// The accuracy of `timeout` is only in milliseconds, anything more precise
    /// will be discarded.
    #[doc(alias = "sq_thread_idle")]
    pub const fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        let ms = timeout.as_millis();
        let ms = if ms <= u32::MAX as u128 {
            // SAFETY: just check above that `millis` is less then `u32::MAX`
            ms as u32
        } else {
            u32::MAX
        };
        self.idle_timeout = Some(ms);
        self
    }

    /// Attach the new (to be created) ring to `other_ring`.
    ///
    /// This will cause the `Ring` being created to share the asynchronous
    /// worker thread backend of the specified `other_ring`, rather than create
    /// a new separate thread pool.
    ///
    /// Uses `IORING_SETUP_ATTACH_WQ`, added in Linux kernel 5.6.
    #[doc(alias = "IORING_SETUP_ATTACH_WQ")]
    pub const fn attach(self, other_ring: &'r Ring) -> Self {
        self.attach_queue(other_ring.submission_queue())
    }

    /// Same as [`Config::attach`], but accepts a [`SubmissionQueue`].
    pub const fn attach_queue(mut self, other_ring: &'r SubmissionQueue) -> Self {
        self.attach = Some(other_ring);
        self
    }

    /// Build a new [`Ring`].
    #[doc(alias = "io_uring_setup")]
    pub fn build(self) -> io::Result<Ring> {
        // SAFETY: all zero is valid for `io_uring_params`.
        let mut parameters: libc::io_uring_params = unsafe { mem::zeroed() };
        parameters.flags = libc::IORING_SETUP_SUBMIT_ALL; // Submit all submissions on error.
        if self.kernel_thread {
            parameters.flags |= libc::IORING_SETUP_SQPOLL // Kernel thread for polling.
            // Using `IORING_SETUP_SQPOLL` we always have one issuer.
            | libc::IORING_SETUP_SINGLE_ISSUER;
        } else {
            // Don't interrupt userspace, the user must call `Ring::poll` any way.
            parameters.flags |= libc::IORING_SETUP_COOP_TASKRUN;
        }
        if let Some(completion_entries) = self.completion_entries {
            parameters.cq_entries = completion_entries;
            parameters.flags |= libc::IORING_SETUP_CQSIZE;
        }
        if self.clamp {
            parameters.flags |= libc::IORING_SETUP_CLAMP;
        }
        if let Some(cpu) = self.cpu_affinity {
            parameters.flags |= libc::IORING_SETUP_SQ_AFF;
            parameters.sq_thread_cpu = cpu;
        }
        if let Some(idle_timeout) = self.idle_timeout {
            parameters.sq_thread_idle = idle_timeout;
        }
        #[allow(clippy::cast_sign_loss)] // File descriptors are always positive.
        if let Some(other_ring) = self.attach {
            parameters.wq_fd = other_ring.shared.ring_fd.as_raw_fd() as u32;
            parameters.flags |= libc::IORING_SETUP_ATTACH_WQ;
        }

        let mut first_err = None;
        let fd = loop {
            match libc::syscall!(io_uring_setup(self.submission_entries, &mut parameters)) {
                // SAFETY: just created the fd (and checked the error).
                Ok(fd) => break unsafe { OwnedFd::from_raw_fd(fd) },
                Err(err) => {
                    if let io::ErrorKind::InvalidInput = err.kind() {
                        // We set some flags which are not strictly required by
                        // A10, but provide various benefits. However in doing
                        // so we also increases our minimal supported Kernel
                        // version.
                        // Here we remove the flags one by one and try again.
                        // NOTE: this is mainly done to support the CI, which
                        // currently uses Linux 5.15.
                        remove_flag!(
                            parameters,
                            first_err,
                            err,
                            IORING_SETUP_SUBMIT_ALL,    // 5.18.
                            IORING_SETUP_COOP_TASKRUN,  // 5.19.
                            IORING_SETUP_SINGLE_ISSUER, // 6.0.
                        );
                    }
                    return Err(first_err.unwrap_or(err));
                }
            };
        };
        check_feature!(parameters.features, IORING_FEAT_NODROP); // Never drop completions.
        check_feature!(parameters.features, IORING_FEAT_SUBMIT_STABLE); // All data for async offload must be consumed.
        check_feature!(parameters.features, IORING_FEAT_RW_CUR_POS); // Allow -1 as current position.
        check_feature!(parameters.features, IORING_FEAT_SQPOLL_NONFIXED); // No need for fixed files.

        let cq = mmap_completion_queue(fd.as_fd(), &parameters)?;
        let sq = mmap_submission_queue(fd, &parameters)?;
        Ok(Ring { cq, sq })
    }
}

/// Memory-map the submission queue.
fn mmap_submission_queue(
    ring_fd: OwnedFd,
    parameters: &libc::io_uring_params,
) -> io::Result<SubmissionQueue> {
    let size = parameters.sq_off.array + parameters.sq_entries * (size_of::<libc::__u32>() as u32);

    let submission_queue = mmap(
        size as usize,
        libc::PROT_READ | libc::PROT_WRITE,
        libc::MAP_SHARED | libc::MAP_POPULATE,
        ring_fd.as_raw_fd(),
        libc::off_t::from(libc::IORING_OFF_SQ_RING),
    )?;

    let submission_queue_entries = mmap(
        parameters.sq_entries as usize * size_of::<libc::io_uring_sqe>(),
        libc::PROT_READ | libc::PROT_WRITE,
        libc::MAP_SHARED | libc::MAP_POPULATE,
        ring_fd.as_raw_fd(),
        libc::off_t::from(libc::IORING_OFF_SQES),
    )
    .map_err(|err| {
        _ = munmap(submission_queue, size as usize); // Can't handle two errors.
        err
    })?;

    let op_indices = AtomicBitMap::new(parameters.cq_entries as usize);
    let mut queued_ops = Vec::with_capacity(op_indices.capacity());
    queued_ops.resize_with(queued_ops.capacity(), || Mutex::new(None));
    let queued_ops = queued_ops.into_boxed_slice();

    #[allow(clippy::mutex_integer)] // For `array_index`, need to the lock for more.
    unsafe {
        Ok(SubmissionQueue {
            shared: Arc::new(SharedSubmissionQueue {
                ring_fd,
                ptr: submission_queue,
                size,
                // Fields are constant, so we load them once.
                len: load_atomic_u32(submission_queue.add(parameters.sq_off.ring_entries as usize)),
                ring_mask: load_atomic_u32(
                    submission_queue.add(parameters.sq_off.ring_mask as usize),
                ),
                kernel_thread: (parameters.flags & libc::IORING_SETUP_SQPOLL) != 0,
                is_polling: AtomicBool::new(false),
                op_indices,
                queued_ops,
                blocked_futures: Mutex::new(Vec::new()),
                pending_tail: AtomicU32::new(0),
                // Fields are shared with the kernel.
                kernel_read: submission_queue.add(parameters.sq_off.head as usize).cast(),
                flags: submission_queue
                    .add(parameters.sq_off.flags as usize)
                    .cast(),
                entries: submission_queue_entries.cast(),
                array_index: Mutex::new(0),
                array: submission_queue
                    .add(parameters.sq_off.array as usize)
                    .cast(),
                array_tail: submission_queue.add(parameters.sq_off.tail as usize).cast(),
            }),
        })
    }
}

/// Memory-map the completion queue.
fn mmap_completion_queue(
    ring_fd: BorrowedFd<'_>,
    parameters: &libc::io_uring_params,
) -> io::Result<CompletionQueue> {
    let size =
        parameters.cq_off.cqes + parameters.cq_entries * (size_of::<libc::io_uring_cqe>() as u32);

    let completion_queue = mmap(
        size as usize,
        libc::PROT_READ | libc::PROT_WRITE,
        libc::MAP_SHARED | libc::MAP_POPULATE,
        ring_fd.as_raw_fd(),
        libc::off_t::from(libc::IORING_OFF_CQ_RING),
    )?;

    unsafe {
        Ok(CompletionQueue {
            ptr: completion_queue,
            size,
            // Fields are constant, so we load them once.
            /* NOTE: usunused.
            len: load_atomic_u32(completion_queue.add(parameters.cq_off.ring_entries as usize)),
            */
            ring_mask: load_atomic_u32(completion_queue.add(parameters.cq_off.ring_mask as usize)),
            // Fields are shared with the kernel.
            head: completion_queue.add(parameters.cq_off.head as usize).cast(),
            tail: completion_queue.add(parameters.cq_off.tail as usize).cast(),
            entries: completion_queue.add(parameters.cq_off.cqes as usize).cast(),
        })
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
    (*ptr.cast::<AtomicU32>()).load(Ordering::Relaxed)
}

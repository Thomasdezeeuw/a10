//! Configuration of a [`Ring`].

use std::mem::{self, size_of};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::time::Duration;
use std::{io, ptr};

use crate::io_uring::{self, libc, Completions, Submissions};
use crate::{syscall, Ring, SubmissionQueue};

#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)] // This is just stupid.
pub(crate) struct Config<'r> {
    submission_entries: u32,
    completion_entries: Option<u32>,
    disabled: bool,
    single_issuer: bool,
    defer_taskrun: bool,
    clamp: bool,
    kernel_thread: bool,
    cpu_affinity: Option<u32>,
    idle_timeout: Option<u32>,
    direct_descriptors: Option<u32>,
    attach: Option<&'r SubmissionQueue>,
}

impl<'r> Config<'r> {
    pub(crate) const fn new() -> Config<'r> {
        Config {
            submission_entries: 32,
            completion_entries: None,
            disabled: false,
            single_issuer: false,
            defer_taskrun: false,
            clamp: false,
            kernel_thread: false,
            cpu_affinity: None,
            idle_timeout: None,
            direct_descriptors: None,
            attach: None,
        }
    }
}

/// io_uring specific configuration.
impl<'r> crate::Config<'r> {
    /// Set the size of the io_uring submission queue.
    ///
    /// It must be a power of two. Defaults to 32. Also see
    /// [`Config::maximum_queue_size`] to set the maximum queue size.
    ///
    /// [`Config::maximum_queue_size`]: crate::Config::maximum_queue_size
    #[doc(alias = "io_uring_setup")]
    pub const fn with_submission_queue_size(mut self, entries: u32) -> Self {
        self.sys.submission_entries = entries;
        self
    }

    /// Set the size of the io_uring completion queue.
    ///
    /// It must be a power of two. Defaults to twice the size of the submission
    /// queue. Also see [`Config::maximum_queue_size`] to set the maximum queue
    /// size.
    ///
    /// [`Config::maximum_queue_size`]: crate::Config::maximum_queue_size
    #[doc(alias = "IORING_SETUP_CQSIZE")]
    pub const fn with_completion_queue_size(mut self, entries: u32) -> Self {
        self.sys.completion_entries = Some(entries);
        self
    }

    /// Use the largest possible submission and completion queue sizes.
    #[doc(alias = "IORING_SETUP_CLAMP")]
    pub const fn with_maximum_queue_size(mut self) -> Self {
        self.sys.submission_entries = u32::MAX;
        self.sys.clamp = true;
        self
    }

    /// Enable single issuer.
    ///
    /// This hints to the kernel that only a single thread will submit requests,
    /// which is used for optimisations within the kernel. This means that only
    /// the thread that [`build`] the ring or [`enabled`] it (after starting in
    /// disable mode) may register resources with the ring, resources such as
    /// the [`ReadBufPool`].
    ///
    /// This optimisation is enforces by the kernel, which will return `EEXIST`
    /// or `AlreadyExists` if another thread attempts to register resource or
    /// otherwise use the [`Ring`] in a way that is not allowed.
    ///
    /// [`build`]: crate::Config::build
    /// [`enabled`]: Ring::enable
    /// [`ReadBufPool`]: crate::io::ReadBufPool
    #[doc(alias = "IORING_SETUP_SINGLE_ISSUER")]
    pub const fn single_issuer(mut self) -> Self {
        self.sys.single_issuer = true;
        self
    }

    /// Defer task running.
    ///
    /// By default, kernel will process all outstanding work at the end of any
    /// system call or thread interrupt. This can delay the application from
    /// making other progress.
    ///
    /// Enabling this option will hint to kernel that it should defer work until
    /// [`Ring::poll`] is called. This way the work is done in the
    /// [`Ring::poll`].
    ///
    /// This options required [`Config::single_issuer`] to be set. This option
    /// does not work with [`Config::with_kernel_thread`] set.
    ///
    /// [`Config::single_issuer`]: crate::Config::with_kernel_thread
    /// [`Config::with_kernel_thread`]: crate::Config::with_kernel_thread
    #[doc(alias = "IORING_SETUP_DEFER_TASKRUN")]
    pub const fn defer_task_run(mut self) -> Self {
        self.sys.defer_taskrun = true;
        self
    }

    /// Start a kernel thread polling the [`Ring`].
    ///
    /// When this option is enabled a kernel thread is created to perform
    /// submission queue polling. This allows issuing I/O without ever context
    /// switching into the kernel.
    #[doc(alias = "IORING_SETUP_SQPOLL")]
    pub const fn with_kernel_thread(mut self) -> Self {
        self.sys.kernel_thread = true;
        self
    }

    /// Set the CPU affinity of kernel thread polling the [`Ring`].
    ///
    /// Only works in combination with [`Config::with_kernel_thread`].
    ///
    /// [`Config::with_kernel_thread`]: crate::Config::with_kernel_thread
    #[doc(alias = "IORING_SETUP_SQ_AFF")]
    #[doc(alias = "sq_thread_cpu")]
    pub const fn with_cpu_affinity(mut self, cpu: u32) -> Self {
        self.sys.cpu_affinity = Some(cpu);
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
    ///
    /// Only works in combination with [`Config::with_kernel_thread`].
    ///
    /// [`Config::with_kernel_thread`]: crate::Config::with_kernel_thread
    #[doc(alias = "sq_thread_idle")]
    pub const fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        let millis = timeout.as_millis();
        let ms = if millis > (u32::MAX as u128) {
            u32::MAX
        } else {
            // SAFETY: checked if the value fits above.
            // TODO: use `millis.try_into().unwrap_or(u32::MAX)` once that is
            // stable in const fns.
            millis as u32
        };
        self.sys.idle_timeout = Some(ms);
        self
    }

    /// Enable direct descriptors.
    ///
    /// This registers a sparse array of `size` direct descriptor slots enabling
    /// direct descriptors to be used. If this is not used attempts to create a
    /// direct descriptor will result in `ENXIO`.
    ///
    /// By default direct descriptors are not enabled.
    #[doc(alias = "IORING_REGISTER_FILES")]
    #[doc(alias = "IORING_REGISTER_FILES2")]
    #[doc(alias = "IORING_RSRC_REGISTER_SPARSE")]
    pub const fn with_direct_descriptors(mut self, size: u32) -> Self {
        self.sys.direct_descriptors = Some(size);
        self
    }

    /// Start the ring in a disabled state.
    ///
    /// While the ring is disabled submissions are not allowed. To enable the
    /// ring use [`Ring::enable`].
    #[doc(alias = "IORING_SETUP_R_DISABLED")]
    pub const fn disable(mut self) -> Self {
        self.sys.disabled = true;
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
    pub fn attach(self, other_ring: &'r Ring) -> Self {
        self.attach_queue(other_ring.sq())
    }

    /// Same as [`Config::attach`], but accepts a [`SubmissionQueue`].
    ///
    /// [`Config::attach`]: crate::Config::attach
    #[doc(alias = "IORING_SETUP_ATTACH_WQ")]
    pub fn attach_queue(mut self, other_sq: &'r SubmissionQueue) -> Self {
        self.sys.attach = Some(other_sq);
        self
    }

    pub(crate) fn build_sys(self) -> io::Result<(Submissions, Completions)> {
        // SAFETY: all zero is valid for `io_uring_params`.
        let mut parameters: libc::io_uring_params = unsafe { mem::zeroed() };
        parameters.flags = libc::IORING_SETUP_SUBMIT_ALL // Submit all submissions on error.
            | libc::IORING_SETUP_NO_SQARRAY; // Don't use indirection for submissions.
        if self.sys.kernel_thread {
            parameters.flags |= libc::IORING_SETUP_SQPOLL; // Kernel thread for polling.
        } else {
            // Don't interrupt userspace, the user must call `Ring::poll` any way.
            parameters.flags |= libc::IORING_SETUP_COOP_TASKRUN;
        }
        if self.sys.disabled {
            // Start the ring in disabled mode.
            parameters.flags |= libc::IORING_SETUP_R_DISABLED;
        }
        if self.sys.single_issuer {
            // Only allow access from a single thread.
            parameters.flags |= libc::IORING_SETUP_SINGLE_ISSUER;
        }
        if self.sys.defer_taskrun {
            parameters.flags |= libc::IORING_SETUP_DEFER_TASKRUN;
        }
        #[rustfmt::skip]
        let submission_entries = self.sys.submission_entries;
        if let Some(completion_entries) = self.sys.completion_entries {
            parameters.cq_entries = completion_entries;
            parameters.flags |= libc::IORING_SETUP_CQSIZE;
        }
        if self.sys.clamp {
            parameters.flags |= libc::IORING_SETUP_CLAMP;
        }
        if let Some(cpu) = self.sys.cpu_affinity {
            parameters.flags |= libc::IORING_SETUP_SQ_AFF;
            parameters.sq_thread_cpu = cpu;
        }
        if let Some(idle_timeout) = self.sys.idle_timeout {
            parameters.sq_thread_idle = idle_timeout;
        }
        #[allow(clippy::cast_sign_loss)] // File descriptors are always positive.
        if let Some(other_sq) = self.sys.attach {
            parameters.wq_fd = other_sq.submissions().ring_fd() as u32;
            parameters.flags |= libc::IORING_SETUP_ATTACH_WQ;
        }

        let rfd = match syscall!(io_uring_setup(submission_entries, &raw mut parameters)) {
            // SAFETY: just created the fd (and checked the error).
            Ok(rfd) => unsafe { OwnedFd::from_raw_fd(rfd) },
            Err(err) => return Err(err),
        };
        check_feature!(parameters.features, IORING_FEAT_NODROP); // Never drop completions.
        check_feature!(parameters.features, IORING_FEAT_SUBMIT_STABLE); // All data for async offload must be consumed.
        check_feature!(parameters.features, IORING_FEAT_RW_CUR_POS); // Allow -1 as current position.
        check_feature!(parameters.features, IORING_FEAT_SQPOLL_NONFIXED); // No need for fixed files.

        let shared = io_uring::Shared::new(rfd, &parameters)?;
        let submissions = io_uring::Submissions::new(shared);
        let completions = io_uring::Completions::new(submissions.ring_fd(), &parameters)?;

        if let Some(size) = self.sys.direct_descriptors {
            let register = libc::io_uring_rsrc_register {
                flags: libc::IORING_RSRC_REGISTER_SPARSE,
                nr: size,
                resv2: 0,
                data: 0,
                tags: 0,
            };
            let arg = ptr::from_ref(&register).cast();
            let size = size_of::<libc::io_uring_rsrc_register>();
            submissions
                .shared()
                .register(libc::IORING_REGISTER_FILES2, arg, size as u32)?;
        }

        Ok((submissions, completions))
    }
}

macro_rules! check_feature {
    ($features: expr, $required: ident $(,)?) => {
        if $features & libc::$required != 0 {
            return Err(::std::io::Error::new(
                ::std::io::ErrorKind::Unsupported,
                ::std::concat!(
                    "Kernel doesn't have required `",
                    stringify!($required),
                    "` feature"
                ),
            ));
        };
    };
}

use check_feature;

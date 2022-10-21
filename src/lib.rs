//! The [A10] io_uring library.
//!
//! This library is meant as a low-level library safely exposing the io_uring
//! API. For simplicity this only has two main types and a number of helper
//! types:
//!  * [`Ring`] is a wrapper around io_uring used to poll for completion events.
//!  * [`AsyncFd`] is a wrapper around a file descriptor that provides a safe
//!    API to schedule operations.
//!
//! Some modules provide ways to create `AsyncFd`, e.g. [`OpenOptions`], others
//! are simply a place to expose the [`Future`]s supporting the scheduled
//! operations. The modules try to follow the same structure as that of std lib.
//!
//! [A10]: https://en.wikipedia.org/wiki/A10_motorway_(Netherlands)
//! [`OpenOptions`]: fs::OpenOptions
//! [`Future`]: std::future::Future

#![feature(const_mut_refs, io_error_more)]

use std::marker::PhantomData;
use std::os::unix::io::{AsRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::{fmt, ptr};

pub mod buf;
mod config;
pub mod extract;
pub mod fs;
pub mod io;
pub mod net;
mod op;

// TODO: replace this with definitions from the `libc` crate once available.
mod sys;
use sys as libc;

pub use config::Config;
#[doc(no_inline)]
pub use extract::Extract;
use op::{SharedOperationState, Submission};

/// This type represents the user space side of an io_uring.
///
/// An io_uring is split into two queues: the submissions and completions queue.
/// The [`SubmissionQueue`] is public, but doesn't provide any methods. The
/// `SubmissionQueue` is only used by I/O types in the crate to schedule
/// asynchronous operations.
///
/// The completions queue is not exposed by the crate and only used internally.
/// Instead it will wake the [`Future`]s exposed by the various I/O type, such
/// as [`AsyncFd::write`]'s [`Write`] `Future`.
///
/// [`Future`]: std::future::Future
/// [`AsyncFd::write`]: AsyncFd::write
/// [`Write`]: io::Write
#[derive(Debug)]
pub struct Ring {
    /// # Notes
    ///
    /// `CompletionQueue` musted be dropped before the `SubmissionQueue` because
    /// the `ring_fd` in `SubmissionQueue` is used in the memory mappings
    /// backing `CompletionQueue`.
    cq: CompletionQueue,
    /// Shared between this `Ring` and all types that queue any operations.
    ///
    /// Because it depends on memory mapping from the file descriptor of the
    /// ring the file descriptor is stored in the `SubmissionQueue` itself.
    sq: SubmissionQueue,
}

impl Ring {
    /// Configure a `Ring`.
    ///
    /// `entries` must be a power of two and in the range 1..=4096.
    ///
    /// # Notes
    ///
    /// A10 always uses `IORING_SETUP_SQPOLL`, which required Linux kernel 5.11
    /// to work correctly. Furthermore the user needs the `CAP_SYS_NICE`
    /// capability.
    pub const fn config<'r>(entries: u32) -> Config<'r> {
        Config::new(entries)
    }

    /// Create a new `Ring` with the default configuration.
    ///
    /// For more configuration options see [`Config`].
    #[doc(alias = "io_uring_setup")]
    pub fn new(entries: u32) -> io::Result<Ring> {
        Config::new(entries).build()
    }

    /// Returns the `SubmissionQueue` used by this ring.
    ///
    /// The `SubmissionQueue` can be used to queue asynchronous I/O operations.
    pub fn submission_queue(&self) -> SubmissionQueue {
        self.sq.clone()
    }

    /// Poll the ring for completions.
    ///
    /// This will wake all completed [`Future`]s of the result of their
    /// operation.
    ///
    /// If a zero duration timeout (i.e. `Some(Duration::ZERO)`) is passed this
    /// function will only wake all already completed operations. It guarantees
    /// to not make a system call, but it also means it doesn't gurantee at
    /// least one completion was processed.
    ///
    /// [`Future`]: std::future::Future
    #[doc(alias = "io_uring_enter")]
    pub fn poll(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        for completion in self.completions(timeout)? {
            log::trace!("got completion: {completion:?}");
            let user_data = completion.inner.user_data;
            if user_data == 0 {
                // No callback.
                continue;
            }

            unsafe { SharedOperationState::complete(user_data, completion.inner.res) }
        }
        Ok(())
    }

    /// Submit all submissions and wait for at least `n` completions.
    ///
    /// Setting `n` to zero will submit all queued operations and return any
    /// completions, without blocking.
    #[doc(alias = "io_uring_enter")]
    fn completions(&mut self, timeout: Option<Duration>) -> io::Result<Completions> {
        // First we check if there are already completions events queued.
        let head = self.completion_head();
        let tail = self.completion_tail();
        if head != tail || matches!(timeout, Some(Duration::ZERO)) {
            return Ok(Completions {
                entries: self.cq.entries,
                local_head: head,
                head: self.cq.head,
                tail,
                ring_mask: self.cq.ring_mask,
                _lifetime: PhantomData,
            });
        }

        if let Some(timeout) = timeout {
            let timeout = libc::timespec {
                tv_sec: timeout.as_secs() as _,
                tv_nsec: libc::c_longlong::from(timeout.subsec_nanos()),
            };

            self.sq
                .add(|submission| unsafe { submission.timeout(&timeout) })?;
        }

        // If not we need to check if the kernel thread is stil awake. If the
        // kernel thread is not awake we'll need to wake it.
        let mut enter_flags = libc::IORING_ENTER_GETEVENTS;
        let submission_flags = unsafe { &*self.sq.shared.flags }.load(Ordering::Acquire);
        if submission_flags & libc::IORING_SQ_NEED_WAKEUP != 0 {
            log::debug!("waking kernel thread");
            enter_flags |= libc::IORING_ENTER_SQ_WAKEUP;
        }

        log::debug!("waiting for completion events");
        let n = libc::syscall!(io_uring_enter(
            self.sq.shared.ring_fd.as_raw_fd(),
            0, // We've already queued and submitted our submissions.
            1, // Wait for at least one completion.
            enter_flags,
            ptr::null(),
            0
        ))?;
        log::trace!("got {n} completion events");

        // NOTE: we're the only onces writing to the completion head so we don't
        // need to read it again.
        let tail = self.completion_tail();
        Ok(Completions {
            entries: self.cq.entries,
            local_head: head,
            head: self.cq.head,
            tail,
            ring_mask: self.cq.ring_mask,
            _lifetime: PhantomData,
        })
    }

    /// Returns `CompletionQueue.head`.
    fn completion_head(&mut self) -> u32 {
        // SAFETY: we're the only once writing to it so `Relaxed` is fine. The
        // pointer itself is valid as long as `Ring.fd` is alive.
        unsafe { (*self.cq.head).load(Ordering::Relaxed) }
    }

    /// Returns `CompletionQueue.tail`.
    fn completion_tail(&self) -> u32 {
        // SAFETY: this written to by the kernel so we need to use `Acquire`
        // ordering. The pointer itself is valid as long as `Ring.fd` is alive.
        unsafe { (*self.cq.tail).load(Ordering::Acquire) }
    }
}

/// Queue to submit asynchronous operations to.
///
/// This type doesn't have any public methods, but is used by all I/O types,
/// such as [`OpenOptions`], to queue asynchronous operations. The queue can be
/// acquired by using [`Ring::submission_queue`].
///
/// [`OpenOptions`]: fs::OpenOptions
#[derive(Debug, Clone)]
pub struct SubmissionQueue {
    shared: Arc<SharedSubmissionQueue>,
}

/// Shared internals of [`SubmissionQueue`].
#[derive(Debug)]
struct SharedSubmissionQueue {
    /// File descriptor of the io_uring.
    ring_fd: OwnedFd,

    /// Mmap-ed pointer.
    #[allow(dead_code)]
    ptr: *mut libc::c_void,
    /// Mmap-ed size in bytes.
    #[allow(dead_code)]
    size: libc::c_uint,

    /// Local version of `tail`.
    /// Increased in `queue` to give the caller mutable access to a
    /// [`Submission`] in `entries`.
    /// NOTE: this does not mean that `pending_tail` number of submissions are
    /// ready, this is determined by `tail`.
    pending_tail: AtomicU32,
    /// Variable used to get an index into `array`.
    pending_index: AtomicU32,

    // NOTE: the following two fields are constant. we read them once from the
    // mmap area and then copied them here to avoid the need for the atomics.
    /// Number of entries in the queue.
    len: u32,
    /// Mask used to index into the `sqes` queue.
    ring_mask: u32,

    // NOTE: the following fields reference mmaped pages shared with the kernel,
    // thus all need atomic access.
    // FIXME: I think the following fields need `UnsafeCell`.
    /// Head to queue, i.e. the submussions read by the kernel. Incremented by
    /// the kernel when submissions has succesfully been processed.
    kernel_read: *const AtomicU32,
    /// Incremented by us when submitting new submissions.
    tail: *mut AtomicU32,
    /// Number of invalid entries dropped by the kernel.
    #[allow(dead_code)]
    dropped: *const AtomicU32,
    /// Flags set by the kernel to communicate state information.
    flags: *const AtomicU32,
    /// Array of `len` submission entries shared with the kernel. We're the only
    /// one modifiying the structures, but the kernel can read from it.
    ///
    /// This pointer is also used in the `unmmap` call.
    entries: *mut Submission,
    /// Array of `len` indices (into `entries`) shared with the kernel. We're
    /// the only one modifiying the structures, but the kernel can read from it.
    array: *mut AtomicU32,
}

impl SubmissionQueue {
    /// Add a submission to the queue.
    ///
    /// Returns an error if the submission queue is full. To fix this call
    /// [`Ring::poll`] (and handle the completed operations) and try queueing
    /// again.
    fn add<F>(&self, submit: F) -> Result<(), QueueFull>
    where
        F: FnOnce(&mut Submission),
    {
        // First we need to acquire mutable access to an `Submission` entry in
        // the `entries` array.
        //
        // We do this by increasing `pending_tail` by 1, reserving
        // `entries[pending_tail]` for ourselves, while ensuring we don't go
        // beyond what the kernel has processed by checking `tail - kernel_read`
        // is less then the length of the submission queue.
        let kernel_read = self.kernel_read();
        let tail =
            self.shared
                .pending_tail
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |tail| {
                    if tail - kernel_read < self.shared.len {
                        // Still an entry available.
                        Some(tail + 1) // TODO: handle overflows.
                    } else {
                        None
                    }
                });

        if let Ok(tail) = tail {
            // SAFETY: the `ring_mask` ensures we can never get an index larger
            // then the size of the queue. Above we've already ensured that
            // we're the only thread  with mutable access to the entry.
            let submission_index = tail & self.shared.ring_mask;
            let submission = unsafe { &mut *self.shared.entries.add(submission_index as usize) };

            // Let the caller fill the `submission`.
            submission.reset();
            submit(submission);
            debug_assert!(!submission.is_unchanged());

            // Now that we've written our submission we need add it to the
            // `array` so that the kernel can process it.
            let array_tail = self.shared.pending_index.fetch_add(1, Ordering::AcqRel);
            let array_index = (array_tail & self.shared.ring_mask) as usize;
            // SAFETY: `idx` is masked above to be within the correct bounds.
            // As we have unique access `Relaxed` is acceptable.
            unsafe {
                (*self.shared.array.add(array_index)).store(submission_index, Ordering::Relaxed);
            }

            // FIXME: doesn't work. Can have a gap in the `self.array` the
            // kernel will then assume to be filled.
            unsafe { &*self.shared.tail }.fetch_add(1, Ordering::AcqRel);

            Ok(())
        } else {
            Err(QueueFull(()))
        }
    }

    /// Returns `self.kernel_read`.
    fn kernel_read(&self) -> u32 {
        // SAFETY: this written to by the kernel so we need to use `Acquire`
        // ordering. The pointer itself is valid as long as `Ring.fd` is alive.
        unsafe { (*self.shared.kernel_read).load(Ordering::Acquire) }
    }
}

unsafe impl Send for SharedSubmissionQueue {}

unsafe impl Sync for SharedSubmissionQueue {}

/// Error returned when the submission queue is full.
///
/// To resolve this issue call [`Ring::poll`].
///
/// Can be convert into [`io::Error`].
pub struct QueueFull(());

impl From<QueueFull> for io::Error {
    fn from(_: QueueFull) -> io::Error {
        io::Error::new(io::ErrorKind::ResourceBusy, "submission queue is full")
    }
}

impl fmt::Debug for QueueFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueueFull").finish()
    }
}

impl fmt::Display for QueueFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("`a10::Ring` submission queue is full")
    }
}

/// Queue of completion events.
#[derive(Debug)]
struct CompletionQueue {
    /// Mmap-ed pointer to the completion queue.
    #[allow(dead_code)]
    ptr: *mut libc::c_void,
    /// Mmap-ed size in bytes.
    #[allow(dead_code)]
    size: libc::c_uint,

    // NOTE: the following two fields are constant. we read them once from the
    // mmap area and then copied them here to avoid the need for the atomics.
    /// Number of entries in the queue.
    #[allow(dead_code)]
    len: u32,
    /// Mask used to index into the `sqes` queue.
    ring_mask: u32,

    // NOTE: the following fields reference mmaped pages shared with the kernel,
    // thus all need atomic access.
    // FIXME: I think the following fields need `UnsafeCell`.
    /// Incremented by us when completions have been read.
    head: *mut AtomicU32,
    /// Incremented by the kernel when adding completions.
    tail: *const AtomicU32,
    /// Array of `len` completion entries shared with the kernel. The kernel
    /// modifies this array, we're only reading from it.
    entries: *const Completion,
}

unsafe impl Send for CompletionQueue {}

unsafe impl Sync for CompletionQueue {}

/// Iterator of completed operations.
struct Completions<'ring> {
    // TODO: replace these fields with a reference to `CompletionQueue`?
    /// Same as [`CompletionQueue.entries`].
    entries: *const Completion,
    /// Local version of `head`. Used to updated `head` once `Completions` is
    /// dropped.
    local_head: u32,
    /// Same as [`CompletionQueue.head`], used to let the kernel know we've read
    /// the completions once we're dropped.
    head: *mut AtomicU32,
    /// Tail of `entries`, i.e. number of completions the kernel wrote.
    tail: u32,
    /// Same as [`CompletionQueue.ring_mask`].
    ring_mask: u32,
    /// We're depend on the lifetime of [`Ring`].
    _lifetime: PhantomData<&'ring Ring>,
}

impl<'ring> Iterator for Completions<'ring> {
    type Item = &'ring Completion;

    fn next(&mut self) -> Option<Self::Item> {
        let head = self.local_head;
        let tail = self.tail;
        if head < tail {
            // SAFETY: the `ring_mask` ensures we can never get an `idx` larger
            // then the size of the queue. We checked above that the kernel has
            // written the struct (and isn't writing to now) os we can safely
            // read from it.
            let idx = (head & self.ring_mask) as usize;
            let completion = unsafe { &*self.entries.add(idx) };
            self.local_head += 1;
            Some(completion)
        } else {
            None
        }
    }
}

impl<'ring> Drop for Completions<'ring> {
    fn drop(&mut self) {
        // Let the kernel know we've read the completions.
        // SAFETY: the kernel needs to read the value so we need `Release`. The
        // pointer itself is valid as long as `Ring.fd` is alive.
        unsafe { (*self.head).store(self.local_head, Ordering::Release) }
    }
}

/// Event that represents a completed operation.
#[repr(transparent)]
struct Completion {
    inner: libc::io_uring_cqe,
}

impl fmt::Debug for Completion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Completion")
            .field("user_data", &self.inner.user_data)
            .field("res", &self.inner.res)
            .field("flags", &self.inner.flags)
            .finish()
    }
}

/// An open file descriptor.
///
/// All functions on `AsyncFd` are asynchronous and return a [`Future`].
///
/// [`Future`]: std::future::Future
#[derive(Debug)]
pub struct AsyncFd {
    pub(crate) fd: RawFd,
    pub(crate) state: SharedOperationState,
}

// NOTE: the implementation are split over the modules to give the `Future`
// implementation types a reasonable place in the docs.

impl AsyncFd {
    /// Create a new `AsyncFd`.
    ///
    /// # Unsafety
    ///
    /// The call must ensure that `fd` is valid and that it's no longer used by
    /// anything other than the returned `AsyncFd`.
    pub unsafe fn new(fd: RawFd, sq: SubmissionQueue) -> AsyncFd {
        AsyncFd {
            fd,
            state: SharedOperationState::new(sq),
        }
    }
}

impl Drop for AsyncFd {
    fn drop(&mut self) {
        let result = self
            .state
            .start(|submission| unsafe { submission.close(self.fd, false) });
        if let Err(err) = result {
            log::error!("error submitting close operation for a10::AsyncFd: {err}");
        }
    }
}

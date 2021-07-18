#![feature(vec_spare_capacity)]

// # NOTES
//
// This code references the "io_uring paper" which is "Efficient IO with
// io_uring" by Jens Axboe.
//
// SQ  -> submission queue.
// SQE -> submission queue event.
// CQ  -> completion queue.
// CQE -> completion queue event.
//
// `io_uring_sqe` -> submission queue event structure.
// `io_uring_cqe` -> completion queue event structure.
//
// Code:
// https://github.com/torvalds/linux/blob/c288d9cd710433e5991d58a0764c4d08a933b871/include/uapi/linux/io_uring.h
// https://github.com/torvalds/linux/blob/50be9417e23af5a8ac860d998e1e3f06b8fd79d7/fs/io_uring.c

// # TODO
//
// Review atomic ordering.

use std::marker::PhantomData;
use std::os::unix::io::RawFd;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::{fmt, ptr};

/// Helper macro to execute a system call that returns an `io::Result`.
macro_rules! syscall {
    ($fn: ident ( $($arg: expr),* $(,)* ) ) => {{
        let res = unsafe { libc::$fn($($arg, )*) };
        if res == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(res)
        }
    }};
}

mod config;
pub mod fs;
pub mod io;
mod op;

// TODO: replace this with definitions from the `libc` crate once available.
mod sys;
use sys as libc;

pub use config::Config;
use op::{SharedOperationState, Submission};

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
    sq: Arc<SubmissionQueue>,
}

#[derive(Debug)]
struct SubmissionQueue {
    /// File descriptor of the I/O ring.
    ring_fd: RawFd,

    /// Mmap-ed pointer.
    ptr: *mut libc::c_void,
    /// Mmap-ed size in bytes.
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
    /// Incremented by the kernel when I/O has succesfully been submitted.
    head: *const AtomicU32,
    /// Incremented by us when submitting new I/O.
    tail: *mut AtomicU32,
    /// Number of invalid entries dropped by the kernel.
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
    /// [`Ring::wait_for`] (and handle the completed operations) and try
    /// queueing again.
    fn add<F>(&self, submit: F) -> Result<(), QueueFull>
    where
        F: FnOnce(&mut Submission),
    {
        // First we need to acquire mutable access to an `Submission` entry in
        // the `entries` array.
        //
        // We do this by increasing `pending_tail` by 1, reserving
        // `entries[pending_tail]` for ourselves, while ensuring we don't go
        // beyond what the kernel has processed by checking `tail - head` is
        // less then the length of the submission queue.
        let head = self.head();
        let tail = self
            .pending_tail
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |tail| {
                if tail - head < self.len {
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
            let submission_index = tail & self.ring_mask;
            let submission = unsafe { &mut *self.entries.add(submission_index as usize) };

            // Let the caller fill the `submission`.
            #[cfg(debug_assertions)]
            submission.reset();
            submit(submission);
            debug_assert!(!submission.is_unchanged());

            // Now that we've written our submission we need add it to the
            // `array` so that the kernel can process it.
            let array_tail = self.pending_index.fetch_add(1, Ordering::AcqRel);
            let array_index = (array_tail & self.ring_mask) as usize;
            // SAFETY: `idx` is masked above to be within the correct bounds.
            // As we have unique access `Relaxed` is acceptable.
            unsafe { (&*self.array.add(array_index)).store(submission_index, Ordering::Relaxed) }

            // FIXME: doesn't work. Can have a gap in the `self.array` the
            // kernel will then assume to be filled.
            unsafe { &*self.tail }.fetch_add(1, Ordering::AcqRel);

            Ok(())
        } else {
            Err(QueueFull(()))
        }
    }

    /// Returns `self.head`.
    fn head(&self) -> u32 {
        // SAFETY: this written to by the kernel so we need to use `Acquire`
        // ordering. The pointer itself is valid as long as `Ring.fd` is alive.
        unsafe { (&*self.head).load(Ordering::Acquire) }
    }
}

/// Error returned when the submission queue is full.
///
/// To resolve this issue call [`Ring::poll`].
pub struct QueueFull(());

impl From<QueueFull> for io::Error {
    fn from(_: QueueFull) -> io::Error {
        // TODO: maybe use `ResourceBusy`.
        io::Error::new(io::ErrorKind::Other, "submission queue is full")
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

#[derive(Debug)]
struct CompletionQueue {
    /// Mmap-ed pointer to the completion queue.
    ptr: *mut libc::c_void,
    /// Mmap-ed size in bytes.
    size: libc::c_uint,

    // NOTE: the following two fields are constant. we read them once from the
    // mmap area and then copied them here to avoid the need for the atomics.
    /// Number of entries in the queue.
    len: u32,
    /// Mask used to index into the `sqes` queue.
    ring_mask: u32,

    // NOTE: the following fields reference mmaped pages shared with the kernel,
    // thus all need atomic access.
    // FIXME: I think the following fields need `UnsafeCell`.
    /// Incremented by us when I/O completion has been read.
    head: *mut AtomicU32,
    /// Incremented by the kernel when I/O has been completed.
    tail: *const AtomicU32,
    /// Array of `len` completion entries shared with the kernel. The kernel
    /// modifies this array, we're only reading from it.
    entries: *const Completion,
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
    pub const fn config(entries: u32) -> Config {
        Config::new(entries)
    }

    /// Create a new `Ring`.
    ///
    /// For more configuration options see [`Config`].
    #[doc(alias = "io_uring_setup")]
    pub fn new(entries: u32) -> io::Result<Ring> {
        Config::new(entries).build()
    }

    /// Poll the ring for completions.
    ///
    /// This will alert all completed operations of the result of their
    /// operation.
    #[doc(alias = "io_uring_enter")]
    pub fn poll(&mut self) -> io::Result<()> {
        for completion in self.completions()? {
            log::trace!("got completion: {:?}", completion);
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
    fn completions(&mut self) -> io::Result<Completions> {
        // First we check if there are already completions events queued.
        let head = self.completion_head();
        let tail = self.completion_tail();
        if head != tail {
            return Ok(Completions {
                entries: self.cq.entries,
                local_head: head,
                head: self.cq.head,
                tail,
                ring_mask: self.cq.ring_mask,
                _lifetime: PhantomData,
            });
        }

        // If not we need to check if the kernel thread is stil awake. If the
        // kernel thread is not awake we'll need to wake it.
        let mut enter_flags = libc::IORING_ENTER_GETEVENTS;
        let submission_flags = unsafe { &*self.sq.flags }.load(Ordering::Acquire);
        if submission_flags & libc::IORING_SQ_NEED_WAKEUP != 0 {
            log::debug!("waking kernel thread");
            enter_flags |= libc::IORING_ENTER_SQ_WAKEUP
        }

        let n = syscall!(io_uring_enter(
            self.sq.ring_fd,
            0, // We've already queued and submitted our submissions.
            1, // Wait for at least one completion.
            enter_flags,
            ptr::null(),
            0
        ))?;
        log::debug!("waited for {} completion events", n);

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

    /// Clone the `SubmissionQueue`.
    pub(crate) fn submission_queue(&self) -> Arc<SubmissionQueue> {
        self.sq.clone()
    }

    /// Returns `CompletionQueue.head`.
    fn completion_head(&mut self) -> u32 {
        // SAFETY: we're the only once writing to it so `Relaxed` is fine. The
        // pointer itself is valid as long as `Ring.fd` is alive.
        unsafe { (&*self.cq.head).load(Ordering::Relaxed) }
    }

    /// Returns `CompletionQueue.tail`.
    fn completion_tail(&self) -> u32 {
        // SAFETY: this written to by the kernel so we need to use `Acquire`
        // ordering. The pointer itself is valid as long as `Ring.fd` is alive.
        unsafe { (&*self.cq.tail).load(Ordering::Acquire) }
    }
}

impl Drop for SubmissionQueue {
    fn drop(&mut self) {
        // FIXME: do we need to unmap here? Or is closing the fd enough.
        if let Err(err) = syscall!(close(self.ring_fd)) {
            log::error!("error closing io_uring: {}", err);
        }
    }
}

impl Drop for CompletionQueue {
    fn drop(&mut self) {
        // FIXME: do we need to unmap here? Or is closing the fd enough.
    }
}

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
        unsafe { (&*self.head).store(self.local_head, Ordering::Release) }
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

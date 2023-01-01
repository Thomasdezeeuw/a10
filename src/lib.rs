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
//!
//! # Notes
//!
//! Most I/O operations need ownership of the data, e.g. a buffer, so it can
//! delay deallocation if needed. For example when a `Future` is dropped before
//! being polled to completion. This data can be retrieved again by using the
//! [`Extract`] trait.

#![feature(const_mut_refs, io_error_more, new_uninit)]

use std::fmt;
use std::marker::PhantomData;
use std::mem::{replace, size_of};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::io::{AsRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{self, Poll};
use std::time::Duration;

mod bitmap;
mod config;
pub mod extract;
pub mod fs;
pub mod io;
pub mod net;
mod op;
pub mod signals;

// TODO: replace this with definitions from the `libc` crate once available.
mod sys;
use sys as libc;

use bitmap::AtomicBitMap;
use config::munmap;
pub use config::Config;
#[doc(no_inline)]
pub use extract::Extract;
use op::{QueuedOperation, Submission};

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
    pub const fn submission_queue(&self) -> &SubmissionQueue {
        &self.sq
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
        let sq = self.sq.clone(); // TODO: remove clone.
        for completion in self.completions(timeout)? {
            log::trace!("got completion event: {completion:?}");
            if completion.is_in_progress() {
                // SAFETY: we're calling this with information from the kernel.
                unsafe { sq.set_op_in_progress_result(completion.index(), completion.result()) };
            } else if completion.is_notification() {
                // SAFETY: we're calling this with information from the kernel.
                unsafe { sq.complete_in_progress_op(completion.index()) };
            } else {
                // SAFETY: we're calling this with information from the kernel.
                unsafe { sq.complete_op(completion.index(), completion.result()) };
            }
        }

        self.wake_blocked_futures();
        Ok(())
    }

    /// Submit all submissions and wait for at least `n` completions.
    ///
    /// Setting `n` to zero will submit all queued operations and return any
    /// completions, without blocking.
    fn completions(&mut self, timeout: Option<Duration>) -> io::Result<Completions> {
        let head = self.completion_head();
        let mut tail = self.completion_tail();
        if head == tail && !matches!(timeout, Some(Duration::ZERO)) {
            // If we have no completions and we have no, or a non-zero, timeout
            // we make a system call to wait for completion events.
            self.enter(timeout)?;
            // NOTE: we're the only onces writing to the completion `head` so we
            // don't need to read it again.
            tail = self.completion_tail();
        }

        Ok(Completions {
            entries: self.cq.entries,
            local_head: head,
            head: self.cq.head,
            tail,
            ring_mask: self.cq.ring_mask,
            _lifetime: PhantomData,
        })
    }

    /// Make the `io_uring_enter` system call.
    fn enter(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        let mut args = libc::io_uring_getevents_args {
            sigmask: 0,
            sigmask_sz: 0,
            pad: 0,
            ts: 0,
        };
        let mut timespec = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        if let Some(timeout) = timeout {
            timespec.tv_sec = timeout.as_secs() as _;
            timespec.tv_nsec = libc::c_longlong::from(timeout.subsec_nanos());
            args.ts = &timespec as *const _ as u64;
        }

        // If there are no completions we'll wait for one and wake the kernel
        // thread. Previously we checked the `flags` of the submission queue and
        // only set `IORING_ENTER_SQ_WAKEUP` when `IORING_SQ_NEED_WAKEUP` was
        // set, but that turned out was quite racy and didn't always work.
        let enter_flags = libc::IORING_ENTER_GETEVENTS // Wait for a completion.
            | libc::IORING_ENTER_SQ_WAKEUP // Wake the kernel thread.
            | libc::IORING_ENTER_EXT_ARG; // Passing of `args`.
        log::debug!("waiting for completion events");
        let result = libc::syscall!(io_uring_enter(
            self.sq.shared.ring_fd.as_raw_fd(),
            0, // We've already queued and submitted our submissions.
            1, // Wait for at least one completion.
            enter_flags,
            &args as *const _ as *const _,
            size_of::<libc::io_uring_getevents_args>(),
        ));
        match result {
            Ok(_) => Ok(()),
            // Hit timeout, we can ignore it.
            Err(ref err) if err.raw_os_error() == Some(libc::ETIME) => Ok(()),
            Err(err) => return Err(err),
        }
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

    /// Wake all [`SharedSubmissionQueue::blocked_futures`].
    fn wake_blocked_futures(&mut self) {
        // This not particullary efficient, but with a large enough number of
        // entries, `IORING_SETUP_SQPOLL` and suffcient calls to [`Ring::poll`]
        // this shouldn't be used at all.

        let mut blocked_futures = {
            let mut blocked_futures = self.sq.shared.blocked_futures.lock().unwrap();
            if blocked_futures.is_empty() {
                return;
            }

            replace(&mut *blocked_futures, Vec::new())
        };
        // Do the waking outside of the lock.
        for waker in blocked_futures.drain(..) {
            waker.wake();
        }

        // Keep the allocation around, just in case.
        let blocked_futures = {
            replace(
                &mut *self.sq.shared.blocked_futures.lock().unwrap(),
                blocked_futures,
            )
        };
        for waker in blocked_futures {
            waker.wake();
        }
    }
}

/// Queue to submit asynchronous operations to.
///
/// This type doesn't have any public methods, but is used by all I/O types,
/// such as [`OpenOptions`], to queue asynchronous operations. The queue can be
/// acquired by using [`Ring::submission_queue`].
///
/// The submission queue can be shared by cloning it, it's a cheap operation.
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

    /// Bitmap which can be used to create an index into `op_queue`.
    op_indices: Box<AtomicBitMap>,
    /// State of queued operations, holds the (would be) result and
    /// `task::Waker`. It's used when adding new operations and when marking
    /// operations as complete (by the kernel).
    queued_ops: Box<[Mutex<Option<QueuedOperation>>]>,
    /// Futures that are waiting for a slot in `queued_ops`.
    blocked_futures: Mutex<Vec<task::Waker>>,

    // NOTE: the following fields reference mmaped pages shared with the kernel,
    // thus all need atomic access.
    /// Head to queue, i.e. the submussions read by the kernel. Incremented by
    /// the kernel when submissions has succesfully been processed.
    kernel_read: *const AtomicU32,
    /// Incremented by us when submitting new submissions.
    tail: *mut AtomicU32,
    /* NOTE: unused because we expect `IORING_FEAT_NODROP`.
    /// Number of invalid entries dropped by the kernel.
    dropped: *const AtomicU32,
    */
    /* NOTE: currently unused.
    /// Flags set by the kernel to communicate state information.
    flags: *const AtomicU32,
    */
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
    /// Returns an index into the `op_queue` which can be used to check the
    /// progress of the operation. Once the operation is completed and the
    /// result read the index should be made avaiable again in `op_indices` and
    /// the value set to `None`.
    ///
    /// Returns an error if the submission queue is full. To fix this call
    /// [`Ring::poll`] (and handle the completed operations) and try queueing
    /// again.
    fn add<F>(&self, submit: F) -> Result<OpIndex, QueueFull>
    where
        F: FnOnce(&mut Submission),
    {
        // Get an index to the queued operation queue.
        let shared = &*self.shared;
        let op_index = match shared.op_indices.next_available() {
            Some(idx) => idx,
            None => return Err(QueueFull(())),
        };

        let queued_op = QueuedOperation::new();
        // SAFETY: the `AtomicBitMap` always returns valid indices for
        // `op_queue` (it's the whole point of it).
        let mut op = shared.queued_ops[op_index].lock().unwrap();
        let old_queued_op = replace(&mut *op, Some(queued_op));
        debug_assert!(old_queued_op.is_none());

        let res = self.add_no_result(|submission| {
            submit(submission);
            submission.set_user_data(op_index as u64);
        });

        match res {
            Ok(()) => Ok(OpIndex(op_index)),
            Err(err) => {
                // Make the index available, we're not going to use it.
                *op = None;
                shared.op_indices.make_available(op_index);
                Err(err)
            }
        }
    }

    /// Wait for a submission slot, waking `waker` once one is available.
    fn wait_for_submission(&self, waker: task::Waker) {
        self.shared.blocked_futures.lock().unwrap().push(waker);
    }

    /// Same as [`SubmissionQueue::add`], but ignores the result.
    fn add_no_result<F>(&self, submit: F) -> Result<(), QueueFull>
    where
        F: FnOnce(&mut Submission),
    {
        let shared = &*self.shared;
        // First we need to acquire mutable access to an `Submission` entry in
        // the `entries` array.
        //
        // We do this by increasing `pending_tail` by 1, reserving
        // `entries[pending_tail]` for ourselves, while ensuring we don't go
        // beyond what the kernel has processed by checking `tail - kernel_read`
        // is less then the length of the submission queue.
        let kernel_read = self.kernel_read();
        let tail = shared
            .pending_tail
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |tail| {
                if tail - kernel_read < shared.len {
                    // Still an entry available.
                    Some(tail + 1) // TODO: handle overflows.
                } else {
                    None
                }
            });
        let tail = match tail {
            Ok(tail) => tail,
            Err(_) => return Err(QueueFull(())),
        };

        // SAFETY: the `ring_mask` ensures we can never get an index larger
        // then the size of the queue. Above we've already ensured that
        // we're the only thread  with mutable access to the entry.
        let submission_index = tail & shared.ring_mask;
        let submission = unsafe { &mut *shared.entries.add(submission_index as usize) };

        // Let the caller fill the `submission`.
        submission.reset();
        submission.set_user_data(u64::MAX);
        submit(submission);
        #[cfg(debug_assertions)]
        debug_assert!(!submission.is_unchanged());

        // Now that we've written our submission we need add it to the
        // `array` so that the kernel can process it.
        let array_tail = shared.pending_index.fetch_add(1, Ordering::AcqRel);
        let array_index = (array_tail & shared.ring_mask) as usize;
        // SAFETY: `idx` is masked above to be within the correct bounds.
        // As we have unique access `Relaxed` is acceptable.
        unsafe {
            (*shared.array.add(array_index)).store(submission_index, Ordering::Relaxed);
        }

        // FIXME: doesn't work. Can have a gap in the `self.array` the
        // kernel will then assume to be filled.
        unsafe { &*shared.tail }.fetch_add(1, Ordering::AcqRel);

        Ok(())
    }

    /// Poll a queued operation with `op_index` to check if it's ready.
    ///
    /// # Notes
    ///
    /// If this return [`Poll::Ready`] it marks `op_index` slot as available.
    pub(crate) fn poll_op(
        &self,
        ctx: &mut task::Context<'_>,
        op_index: OpIndex,
    ) -> Poll<io::Result<i32>> {
        if let Some(operation) = self.shared.queued_ops.get(op_index.0) {
            let mut operation = operation.lock().unwrap();
            if let Some(op) = &mut *operation {
                let res = op.poll(ctx);
                if res.is_ready() {
                    *operation = None;
                    drop(operation);
                    self.shared.op_indices.make_available(op_index.0);
                }
                return res;
            }
        }
        panic!("a10::SubmissionQueue::poll called incorrectly");
    }

    /// Mark the operation with `op_index` as dropped.
    ///
    /// Because the kernel still has access to the `resources`, we might have to
    /// do some trickery to delay the deallocation of `resources` and making the
    /// queued operation slot available again.
    pub(crate) fn drop_op<T>(&self, op_index: OpIndex, resources: T) {
        if let Some(operation) = self.shared.queued_ops.get(op_index.0) {
            let mut operation = operation.lock().unwrap();
            if let Some(op) = &mut *operation {
                if op.is_done() {
                    // Easy path, the operation has already been completed.
                    *operation = None;
                    // Unlock defore dropping `resources`, which might take a
                    // while.
                    drop(operation);
                    self.shared.op_indices.make_available(op_index.0);

                    // We can safely drop the resources.
                    drop(resources);
                    return;
                }

                // Hard path, the operation is not done, but the Future holding
                // the resource is about to be dropped, so we need to apply some
                // trickery here.
                //
                // We need to do two things:
                // 1. Delay the dropping of `resources` until the kernel is done
                //    with the operation.
                // 2. Delay the available making of the queued operation slot
                //    until the kernel is done with the operation.
                //
                // We achieve 1 by creating a special waker that just drops the
                // resources in `resources`.
                // SAFETY: we're not going to clone the `waker`.
                let waker = unsafe { drop_task_waker(resources) };
                // We achive 2 by setting the operation state to dropped, so
                // that `QueuedOperation::set_result` returns true, which makes
                // `complete` below make the queued operation slot available
                // again.
                op.set_dropped(waker);
                return;
            }
        }
        panic!("a10::SubmissionQueue::drop_op called incorrectly");
    }

    /// Set the result of asynchronous operation, but don't mark it as complete.
    /// This is for zero copy operations which report their result in one
    /// completion and releasing of the buffer in another.
    ///
    /// # Safety
    ///
    /// This may only be called based on information form the kernel.
    unsafe fn set_op_in_progress_result(&self, op_index: usize, result: i32) {
        if let Some(operation) = self.shared.queued_ops.get(op_index) {
            let mut operation = operation.lock().unwrap();
            if let Some(op) = &mut *operation {
                op.set_in_progress_result(result);
            }
        }
    }

    /// Mark in in-progress operation (as set by `set_op_in_progress_result`) as
    /// completed.
    ///
    /// # Safety
    ///
    /// This may only be called when the kernel is no longer using the resources
    /// (e.g. read buffer) for the operation.
    unsafe fn complete_in_progress_op(&self, op_index: usize) {
        if let Some(operation) = self.shared.queued_ops.get(op_index) {
            let mut operation = operation.lock().unwrap();
            if let Some(op) = &mut *operation {
                let is_dropped = op.complete_in_progress();
                if is_dropped {
                    // The Future was previously dropped so no one is waiting on
                    // the result. We can make the slot avaiable again.
                    *operation = None;
                    drop(operation);
                    self.shared.op_indices.make_available(op_index);
                }
            }
        }
    }

    /// Mark an asynchronous operation as complete with `result`.
    ///
    /// # Safety
    ///
    /// This may only be called when the kernel is no longer using the resources
    /// (e.g. read buffer) for the operation.
    unsafe fn complete_op(&self, op_index: usize, result: i32) {
        if let Some(operation) = self.shared.queued_ops.get(op_index) {
            let mut operation = operation.lock().unwrap();
            if let Some(op) = &mut *operation {
                let is_dropped = op.complete(result);
                if is_dropped {
                    // The Future was previously dropped so no one is waiting on
                    // the result. We can make the slot avaiable again.
                    *operation = None;
                    drop(operation);
                    self.shared.op_indices.make_available(op_index);
                }
            }
        }
    }

    /// Returns `self.kernel_read`.
    fn kernel_read(&self) -> u32 {
        // SAFETY: this written to by the kernel so we need to use `Acquire`
        // ordering. The pointer itself is valid as long as `Ring.fd` is alive.
        unsafe { (*self.shared.kernel_read).load(Ordering::Acquire) }
    }
}

/// Returns a `task::Waker` that will drop `to_drop` when the waker is dropped.
///
/// # Safety
///
/// The returned `task::Waker` cannot be cloned, it will panic.
pub(crate) unsafe fn drop_task_waker<T>(to_drop: T) -> task::Waker {
    // SAFETY: this is safe because we just passed the pointer created by
    // `Box::into_raw` to this function.
    fn drop_by_ptr<T>(ptr: *const ()) {
        unsafe { drop(Box::<T>::from_raw(ptr as _)) }
    }

    // SAFETY: we meet the `task::Waker` and `task::RawWaker` requirements.
    unsafe {
        task::Waker::from_raw(task::RawWaker::new(
            Box::into_raw(Box::from(to_drop)) as _,
            &task::RawWakerVTable::new(
                |_| panic!("attempted to clone `a10::drop_task_waker`"),
                // SAFETY: `wake` takes ownership, so dropping is safe.
                drop_by_ptr::<T>,
                |_| { /* `wake_by_ref` is a no-op. */ },
                drop_by_ptr::<T>,
            ),
        ))
    }
}

unsafe impl Send for SharedSubmissionQueue {}

unsafe impl Sync for SharedSubmissionQueue {}

impl Drop for SharedSubmissionQueue {
    fn drop(&mut self) {
        if let Err(err) = munmap(
            self.entries.cast(),
            self.len as usize * size_of::<Submission>(),
        ) {
            log::warn!("error unmapping a10::SubmissionQueue entries: {err}");
        }

        if let Err(err) = munmap(self.ptr, self.size as usize) {
            log::warn!("error unmapping a10::SubmissionQueue: {err}");
        }
    }
}

/// Index into [`SharedSubmissionQueue::op_indices`].
///
/// Returned by [`SubmissionQueue::add`] and used by
/// [`SubmissionQueue::poll_op`] to check for a result.
#[derive(Copy, Clone)]
#[must_use]
struct OpIndex(usize);

impl fmt::Debug for OpIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Error returned when the submission queue is full.
///
/// To resolve this issue call [`Ring::poll`].
///
/// Can be convert into [`io::Error`].
struct QueueFull(());

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
    ptr: *mut libc::c_void,
    /// Mmap-ed size in bytes.
    size: libc::c_uint,

    // NOTE: the following two fields are constant. we read them once from the
    // mmap area and then copied them here to avoid the need for the atomics.
    /* NOTE: usunused.
    /// Number of entries in the queue.
    len: u32,
    */
    /// Mask used to index into the `sqes` queue.
    ring_mask: u32,

    // NOTE: the following fields reference mmaped pages shared with the kernel,
    // thus all need atomic access.
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

impl Drop for CompletionQueue {
    fn drop(&mut self) {
        if let Err(err) = munmap(self.ptr, self.size as usize) {
            log::warn!("error unmapping a10::CompletionQueue: {err}");
        }
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
        unsafe { (*self.head).store(self.local_head, Ordering::Release) }
    }
}

/// Event that represents a completed operation.
#[repr(transparent)]
struct Completion {
    inner: libc::io_uring_cqe,
}

impl Completion {
    /// Returns the operation index.
    const fn index(&self) -> usize {
        self.inner.user_data as usize
    }

    /// Returns the result of the operation.
    const fn result(&self) -> i32 {
        self.inner.res
    }

    /// Return `true` if `IORING_CQE_F_MORE` is set.
    const fn is_in_progress(&self) -> bool {
        self.inner.flags & libc::IORING_CQE_F_MORE != 0
    }

    /// Return `true` if `IORING_CQE_F_NOTIF` is set.
    const fn is_notification(&self) -> bool {
        self.inner.flags & libc::IORING_CQE_F_NOTIF != 0
    }
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
    pub(crate) sq: SubmissionQueue,
}

// NOTE: the implementation are split over the modules to give the `Future`
// implementation types a reasonable place in the docs.

impl AsyncFd {
    /// Create a new `AsyncFd`.
    ///
    /// # Safety
    ///
    /// The call must ensure that `fd` is valid and that it's no longer used by
    /// anything other than the returned `AsyncFd`.
    pub const unsafe fn new(fd: RawFd, sq: SubmissionQueue) -> AsyncFd {
        AsyncFd { fd, sq }
    }
}

impl AsFd for AsyncFd {
    fn as_fd(&self) -> BorrowedFd<'_> {
        unsafe { BorrowedFd::borrow_raw(self.fd) }
    }
}

impl Drop for AsyncFd {
    fn drop(&mut self) {
        let result = self
            .sq
            .add_no_result(|submission| unsafe { submission.close(self.fd) });
        if let Err(err) = result {
            log::error!("error submitting close operation for a10::AsyncFd: {err}");
        }
    }
}

/// Waker allow waking of [`Ring`].
///
/// All this does is interrupt a call to [`Ring::poll`].
#[derive(Clone)]
pub struct Waker {
    sq: SubmissionQueue,
}

impl Waker {
    /// Create a new `Waker` that wakes `ring`.
    pub fn new(ring: &Ring) -> Waker {
        Waker {
            sq: ring.submission_queue().clone(),
        }
    }

    /// Wake the connected [`Ring`].
    pub fn wake(&self) {
        // We ignore the queue full error as it means that is *very* unlikely
        // that the Ring is currently being polling if the submission queue is
        // filled. More likely the Ring hasn't been polled in a while.
        let _: Result<(), QueueFull> = self.sq.add_no_result(|submission| unsafe {
            submission.wake(self.sq.shared.ring_fd.as_raw_fd());
        });
    }
}

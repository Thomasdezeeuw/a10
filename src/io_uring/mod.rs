//! io_uring implementation.

use std::cmp::min;
use std::marker::PhantomData;
use std::mem::{needs_drop, replace, size_of, take};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::sync::atomic::{self, AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{self, Poll};
use std::time::Duration;
use std::{fmt, ptr};

mod config;
mod op;
mod sys;

#[rustfmt::skip] // This must come before the other modules for the documentation.
pub mod fd;
pub mod cancel;
pub mod extract;
pub mod fs;
pub mod io;
pub mod mem;
pub mod msg;
pub mod net;
pub mod poll;
pub mod process;

#[doc(no_inline)]
pub use cancel::Cancel;
pub use config::Config;
#[doc(no_inline)]
pub use extract::Extract;
#[doc(no_inline)]
pub use fd::AsyncFd;

use crate::bitmap::AtomicBitMap;
use crate::drop_waker::{drop_task_waker, DropWake};
use crate::syscall;
use config::munmap;
use op::{QueuedOperation, Submission};
use sys as libc; // TODO: replace this with definitions from the `libc` crate once available.

/// This type represents the user space side of an io_uring.
///
/// An io_uring is split into two queues: the submissions and completions queue.
/// The [`SubmissionQueue`] is public, but doesn't provide many methods. The
/// `SubmissionQueue` is used by I/O types in the crate to schedule asynchronous
/// operations.
///
/// The completions queue is not exposed by the crate and only used internally.
/// Instead it will wake the [`Future`]s exposed by the various I/O types, such
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
    /// A10 uses `IORING_SETUP_SQPOLL` by default, which required Linux kernel
    /// 5.11 to work correctly. Furthermore before Linux 5.13 the user needs the
    /// `CAP_SYS_NICE` capability if run as non-root. This can be disabled by
    /// [`Config::with_kernel_thread`].
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

    /// Enable the ring.
    ///
    /// This only required when starting the ring in disabled mode, see
    /// [`Config::disable`].
    #[allow(clippy::needless_pass_by_ref_mut)]
    pub fn enable(&mut self) -> io::Result<()> {
        self.sq
            .register(libc::IORING_REGISTER_ENABLE_RINGS, ptr::null(), 0)
    }

    /// Poll the ring for completions.
    ///
    /// This will wake all completed [`Future`]s with the result of their
    /// operations.
    ///
    /// If a zero duration timeout (i.e. `Some(Duration::ZERO)`) is passed this
    /// function will only wake all already completed operations. It then
    /// guarantees to not make a system call, but it also means it doesn't
    /// guarantee at least one completion was processed.
    ///
    /// [`Future`]: std::future::Future
    #[doc(alias = "io_uring_enter")]
    pub fn poll(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        let sq = self.sq.clone(); // TODO: remove clone.
        for completion in self.completions(timeout)? {
            log::trace!(completion:? = completion; "dequeued completion event");
            // SAFETY: we're calling this based on information from the kernel.
            unsafe { sq.update_op(completion) };
        }

        self.wake_blocked_futures();
        Ok(())
    }

    /// Returns an iterator for all completion events, makes a system call if no
    /// completions are queued.
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
        let mut args = libc::io_uring_getevents_arg {
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
            timespec.tv_sec = timeout.as_secs().try_into().unwrap_or(i64::MAX);
            timespec.tv_nsec = libc::c_longlong::from(timeout.subsec_nanos());
            args.ts = ptr::addr_of!(timespec) as u64;
        }

        let submissions = if self.sq.shared.kernel_thread {
            0 // Kernel thread handles the submissions.
        } else {
            self.sq.shared.is_polling.store(true, Ordering::Release);
            self.sq.unsubmitted()
        };

        // If there are no completions we'll wait for at least one.
        let enter_flags = libc::IORING_ENTER_GETEVENTS // Wait for a completion.
            | libc::IORING_ENTER_EXT_ARG; // Passing of `args`.

        log::debug!(submissions = submissions; "waiting for completion events");
        let result = syscall!(io_uring_enter2(
            self.sq.shared.ring_fd.as_raw_fd(),
            submissions,
            1, // Wait for at least one completion.
            enter_flags,
            ptr::addr_of!(args).cast(),
            size_of::<libc::io_uring_getevents_arg>(),
        ));
        if !self.sq.shared.kernel_thread {
            self.sq.shared.is_polling.store(false, Ordering::Release);
        }
        match result {
            Ok(_) => Ok(()),
            // Hit timeout, we can ignore it.
            Err(ref err) if err.raw_os_error() == Some(libc::ETIME) => Ok(()),
            Err(err) => Err(err),
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

    /// Wake [`SharedSubmissionQueue::blocked_futures`].
    #[allow(clippy::needless_pass_by_ref_mut)]
    fn wake_blocked_futures(&mut self) {
        // This not particullary efficient, but with a large enough number of
        // entries, `IORING_SETUP_SQPOLL` and suffcient calls to [`Ring::poll`]
        // this shouldn't be used at all.

        let n = self.sq.available_space();
        if n == 0 {
            return;
        }

        let mut blocked_futures = {
            let blocked_futures = &mut *self.sq.shared.blocked_futures.lock().unwrap();
            if blocked_futures.is_empty() {
                return;
            }

            take(blocked_futures)
        };
        // Do the waking outside of the lock.
        let waking = min(n, blocked_futures.len());
        log::trace!(waking_amount = n, waiting_futures = blocked_futures.len(); "waking blocked futures");
        for waker in blocked_futures.drain(..waking) {
            waker.wake();
        }

        // Put the remaining wakers back, even if it's empty to keep the
        // allocation.
        let got = &mut *self.sq.shared.blocked_futures.lock().unwrap();
        let mut added = replace(got, blocked_futures);
        got.append(&mut added);
    }
}

impl AsFd for Ring {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.sq.shared.ring_fd.as_fd()
    }
}

/// Queue to submit asynchronous operations to.
///
/// This type doesn't have many public methods, but is used by all I/O types,
/// such as [`OpenOptions`], to queue asynchronous operations. The queue can be
/// acquired by using [`Ring::submission_queue`].
///
/// The submission queue can be shared by cloning it, it's a cheap operation.
///
/// [`OpenOptions`]: fs::OpenOptions
#[derive(Clone)]
pub struct SubmissionQueue {
    shared: Arc<SharedSubmissionQueue>,
}

/// Shared internals of [`SubmissionQueue`].
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

    // NOTE: the following two fields are constant. We read them once from the
    // mmap area and then copied them here to avoid the need for the atomics.
    /// Number of entries in the queue.
    len: u32,
    /// Mask used to index into the `sqes` queue.
    ring_mask: u32,
    /// True if we're using a kernel thread to do submission polling, i.e. if
    /// `IORING_SETUP_SQPOLL` is enabled.
    kernel_thread: bool,
    /// Boolean indicating a thread is [`Ring::poll`]ing. Only used when
    /// `kernel_thread` is false.
    is_polling: AtomicBool,

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
    /// Flags set by the kernel to communicate state information.
    flags: *const AtomicU32,
    /// Array of `len` submission entries shared with the kernel. We're the only
    /// one modifiying the structures, but the kernel can read from them.
    ///
    /// This pointer is also used in the `unmmap` call.
    entries: *mut Submission,

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

impl SubmissionQueue {
    /// Wake the connected [`Ring`].
    ///
    /// All this does is interrupt a call to [`Ring::poll`].
    pub fn wake(&self) {
        // We ignore the queue full error as it means that is *very* unlikely
        // that the Ring is currently being polling if the submission queue is
        // filled. More likely the Ring hasn't been polled in a while.
        let _: Result<(), QueueFull> = self.add_no_result(|submission| unsafe {
            submission.wake(self.shared.ring_fd.as_raw_fd());
        });
    }

    /// Make a `io_uring_register(2)` system call.
    fn register(
        &self,
        op: libc::c_uint,
        arg: *const libc::c_void,
        nr_args: libc::c_uint,
    ) -> io::Result<()> {
        syscall!(io_uring_register(
            self.shared.ring_fd.as_raw_fd(),
            op,
            arg,
            nr_args
        ))?;
        Ok(())
    }

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
        self._add(submit, QueuedOperation::new)
    }

    /// Same as [`SubmissionQueue::add`] but uses a multishot `QueuedOperation`.
    fn add_multishot<F>(&self, submit: F) -> Result<OpIndex, QueueFull>
    where
        F: FnOnce(&mut Submission),
    {
        self._add(submit, QueuedOperation::new_multishot)
    }

    /// See [`SubmissionQueue::add`] or [`SubmissionQueue::add_multishot`].
    fn _add<F, O>(&self, submit: F, new_op: O) -> Result<OpIndex, QueueFull>
    where
        F: FnOnce(&mut Submission),
        O: FnOnce() -> QueuedOperation,
    {
        // Get an index to the queued operation queue.
        let shared = &*self.shared;
        let Some(op_index) = shared.op_indices.next_available() else {
            return Err(QueueFull(()));
        };

        let queued_op = new_op();
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
                drop(op);
                shared.op_indices.make_available(op_index);
                Err(err)
            }
        }
    }

    /// Queue a new operation without making a submission.
    fn queue_multishot(&self) -> Result<OpIndex, QueueFull> {
        self._queue(QueuedOperation::new_multishot)
    }

    /// See [`SubmissionQueue::queue_multishot`].
    fn _queue<O>(&self, new_op: O) -> Result<OpIndex, QueueFull>
    where
        O: FnOnce() -> QueuedOperation,
    {
        // Get an index to the queued operation queue.
        let shared = &*self.shared;
        let Some(op_index) = shared.op_indices.next_available() else {
            return Err(QueueFull(()));
        };

        let queued_op = new_op();
        // SAFETY: the `AtomicBitMap` always returns valid indices for
        // `op_queue` (it's the whole point of it).
        let old_queued_op = replace(
            &mut *shared.queued_ops[op_index].lock().unwrap(),
            Some(queued_op),
        );
        debug_assert!(old_queued_op.is_none());

        Ok(OpIndex(op_index))
    }

    /// Same as [`SubmissionQueue::add`], but ignores the result.
    #[allow(clippy::mutex_integer)] // For `array_index`, need to the lock for more.
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
                    Some(tail.wrapping_add(1))
                } else {
                    None
                }
            });
        let Ok(tail) = tail else {
            // If the kernel thread is not awake we'll need to wake it to make
            // space in the submission queue.
            self.maybe_wake_kernel_thread();
            return Err(QueueFull(()));
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

        // Ensure that all writes to the `submission` are done.
        atomic::fence(Ordering::SeqCst);

        // Now that we've written our submission we need add it to the
        // `array` so that the kernel can process it.
        log::trace!(submission:? = submission; "queueing submission");
        {
            // Now that the submission is filled we need to add it to the
            // `shared.array` so that the kernel can read from it.
            //
            // We do this with a lock to avoid a race condition between two
            // threads incrementing `shared.tail` concurrently. Consider the
            // following execution:
            //
            // Thread A                           | Thread B
            // ...                                | ...
            // ...                                | Got `array_index` 0.
            // Got `array_index` 1.               |
            // Writes index to `shared.array[1]`. |
            // `shared.tail.fetch_add` to 1.      |
            // At this point the kernel will/can read `shared.array[0]`, but
            // thread B hasn't filled it yet. So the kernel will read an invalid
            // index!
            //                                    | Writes index to `shared.array[0]`.
            //                                    | `shared.tail.fetch_add` to 2.

            let mut array_index = shared.array_index.lock().unwrap();
            let idx = (*array_index & shared.ring_mask) as usize;
            // SAFETY: `idx` is masked above to be within the correct bounds.
            unsafe { (*shared.array.add(idx)).store(submission_index, Ordering::Release) };
            // SAFETY: we filled the array above.
            let old_tail = unsafe { (*shared.array_tail).fetch_add(1, Ordering::AcqRel) };
            debug_assert!(old_tail == *array_index);
            *array_index += 1;
        }

        // If the kernel thread is not awake we'll need to wake it for it to
        // process our submission.
        self.maybe_wake_kernel_thread();
        // When we're not using the kernel polling thread we might have to
        // submit the event ourselves to ensure we can make progress while the
        // (user space) polling thread is calling `Ring::poll`.
        self.maybe_submit_event();
        Ok(())
    }

    /// Wait for a submission slot, waking `waker` once one is available.
    fn wait_for_submission(&self, waker: task::Waker) {
        log::trace!(waker:? = waker; "adding blocked future");
        self.shared.blocked_futures.lock().unwrap().push(waker);
    }

    /// Returns the number of slots available.
    ///
    /// # Notes
    ///
    /// The value return can be outdated the nanosecond it is returned, don't
    /// make a safety decisions based on it.
    fn available_space(&self) -> usize {
        // SAFETY: the `kernel_read` pointer itself is valid as long as
        // `Ring.fd` is alive.
        // We use Relaxed here because the caller knows the value will be
        // outdated.
        let kernel_read = unsafe { (*self.shared.kernel_read).load(Ordering::Relaxed) };
        let pending_tail = self.shared.pending_tail.load(Ordering::Relaxed);
        (self.shared.len - (pending_tail - kernel_read)) as usize
    }

    /// Returns the number of unsumitted submission queue entries.
    fn unsubmitted(&self) -> u32 {
        // SAFETY: the `kernel_read` pointer itself is valid as long as
        // `Ring.fd` is alive.
        // We use Relaxed here because it can already be outdated the moment we
        // return it, the caller has to deal with that.
        let kernel_read = unsafe { (*self.shared.kernel_read).load(Ordering::Relaxed) };
        let pending_tail = self.shared.pending_tail.load(Ordering::Relaxed);
        pending_tail - kernel_read
    }

    /// Wake up the kernel thread polling for submission events, if the kernel
    /// thread needs a wakeup.
    fn maybe_wake_kernel_thread(&self) {
        if self.shared.kernel_thread && (self.flags() & libc::IORING_SQ_NEED_WAKEUP != 0) {
            log::debug!("waking submission queue polling kernel thread");
            let res = syscall!(io_uring_enter2(
                self.shared.ring_fd.as_raw_fd(),
                0,                            // We've already queued our submissions.
                0,                            // Don't wait for any completion events.
                libc::IORING_ENTER_SQ_WAKEUP, // Wake up the kernel.
                ptr::null(),                  // We don't pass any additional arguments.
                0,
            ));
            if let Err(err) = res {
                log::warn!("failed to wake submission queue polling kernel thread: {err}");
            }
        }
    }

    /// Submit the event to the kernel when not using a kernel polling thread
    /// and another thread is currently [`Ring::poll`]ing.
    fn maybe_submit_event(&self) {
        if !self.shared.kernel_thread && self.shared.is_polling.load(Ordering::Relaxed) {
            log::debug!("submitting submission event while another thread is `Ring::poll`ing");
            let ring_fd = self.shared.ring_fd.as_raw_fd();
            let res = syscall!(io_uring_enter2(ring_fd, 1, 0, 0, ptr::null(), 0));
            if let Err(err) = res {
                log::warn!("failed to submit event: {err}");
            }
        }
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
    ) -> Poll<io::Result<(u16, i32)>> {
        log::trace!(op_index = op_index.0; "polling operation");
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

    /// Poll a queued multishot operation with `op_index` to check if it's
    /// ready.
    ///
    /// # Notes
    ///
    /// If this return [`Poll::Ready(None)`] it marks `op_index` slot as
    /// available.
    pub(crate) fn poll_multishot_op(
        &self,
        ctx: &mut task::Context<'_>,
        op_index: OpIndex,
    ) -> Poll<Option<io::Result<(u16, i32)>>> {
        log::trace!(op_index = op_index.0; "polling multishot operation");
        if let Some(operation) = self.shared.queued_ops.get(op_index.0) {
            let mut operation = operation.lock().unwrap();
            if let Some(op) = &mut *operation {
                return match op.poll(ctx) {
                    Poll::Ready(res) => Poll::Ready(Some(res)),
                    Poll::Pending if op.is_done() => {
                        *operation = None;
                        drop(operation);
                        self.shared.op_indices.make_available(op_index.0);
                        Poll::Ready(None)
                    }
                    Poll::Pending => Poll::Pending,
                };
            }
        }
        panic!("a10::SubmissionQueue::poll_multishot called incorrectly");
    }

    /// Mark the operation with `op_index` as dropped, attempting to cancel it.
    ///
    /// Because the kernel still has access to the resources, we might have to
    /// do some trickery to delay the deallocation of resources and making the
    /// queued operation slot available again.
    ///
    /// When the operation is still in progress we attempt to cancel it using
    /// submission created by `cancel`. If the operation has completed it will
    /// just drop resources (using `create_drop_waker`) and make the slot
    /// available again.
    ///
    /// # Notes
    ///
    /// `cancel` should most likely use [`Submission::no_completion_event`]
    pub(crate) fn cancel_op<R, D, F>(
        &self,
        op_index: OpIndex,
        create_drop_waker: R,
        cancel: F,
    ) -> Result<(), QueueFull>
    where
        R: FnOnce() -> D,
        D: DropWake,
        F: FnOnce(&mut Submission),
    {
        log::trace!(op_index = op_index.0; "canceling operation");
        if let Some(operation) = self.shared.queued_ops.get(op_index.0) {
            let mut operation = operation.lock().unwrap();
            if let Some(op) = &mut *operation {
                if op.no_more_events() {
                    // Easy path, the operation has already been completed.
                    *operation = None;
                    // Unlock defore dropping `create_drop_waker`, which might take a
                    // while.
                    drop(operation);
                    self.shared.op_indices.make_available(op_index.0);

                    // We can safely drop the resources.
                    drop(create_drop_waker);
                    return Ok(());
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
                // resources (created by `create_drop_waker`).
                let waker = if needs_drop::<D>() {
                    // SAFETY: we're not going to clone the `waker`.
                    Some(unsafe { drop_task_waker(create_drop_waker()) })
                } else {
                    // Of course if we don't need to drop `T`, then we don't
                    // have to use a special waker. But we still don't want to
                    // wake up the `Future` as that not longer used.
                    None
                };
                // We achive 2 by setting the operation state to dropped, so
                // that `QueuedOperation::set_result` returns true, which makes
                // `complete` below make the queued operation slot available
                // again.
                op.set_dropped(waker);
                // Cancel the operation.
                return self.add_no_result(cancel);
            }
        }
        panic!("a10::SubmissionQueue::cancel_op called incorrectly");
    }

    /// Update an operation based on `completion`.
    ///
    /// # Safety
    ///
    /// This may only be called based on information form the kernel.
    unsafe fn update_op(&self, completion: &Completion) {
        let op_index = completion.index();
        if let Some(operation) = self.shared.queued_ops.get(op_index) {
            let mut operation = operation.lock().unwrap();
            if let Some(op) = &mut *operation {
                log::trace!(op_index = op_index, completion:? = completion; "updating operation");
                let is_dropped = op.update(completion);
                if is_dropped && op.no_more_events() {
                    // The Future was previously dropped so no one is waiting on
                    // the result. We can make the slot avaiable again.
                    *operation = None;
                    drop(operation);
                    self.shared.op_indices.make_available(op_index);
                }
            } else {
                log::trace!(op_index = op_index, completion:? = completion; "operation gone, but got completion event");
            }
        }
    }

    /// Returns `self.kernel_read`.
    fn kernel_read(&self) -> u32 {
        // SAFETY: this written to by the kernel so we need to use `Acquire`
        // ordering. The pointer itself is valid as long as `Ring.fd` is alive.
        unsafe { (*self.shared.kernel_read).load(Ordering::Acquire) }
    }

    /// Returns `self.flags`.
    fn flags(&self) -> u32 {
        // SAFETY: this written to by the kernel so we need to use `Acquire`
        // ordering. The pointer itself is valid as long as `Ring.fd` is alive.
        unsafe { (*self.shared.flags).load(Ordering::Acquire) }
    }
}

#[allow(clippy::mutex_integer)] // For `array_index`, need to the lock for more.
impl fmt::Debug for SubmissionQueue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        /// Load a `u32` using relaxed ordering from `ptr`.
        fn load_atomic_u32(ptr: *const AtomicU32) -> u32 {
            unsafe { (*ptr).load(Ordering::Relaxed) }
        }

        let shared = &*self.shared;
        let all = f.alternate();
        let mut f = f.debug_struct("SubmissionQueue");

        f.field("ring_fd", &shared.ring_fd.as_raw_fd())
            .field("len", &shared.len)
            .field("ring_mask", &shared.ring_mask)
            .field("flags", &load_atomic_u32(shared.flags))
            .field("pending_tail", &shared.pending_tail)
            .field("kernel_read", &load_atomic_u32(shared.kernel_read))
            .field(
                "array_index",
                &shared.array_index.lock().map(|i| *i).unwrap_or(u32::MAX),
            )
            .field("array_tail", &load_atomic_u32(shared.array_tail));

        if all {
            f.field("op_indices", &shared.op_indices)
                .field("queued_ops", &shared.queued_ops)
                .field("blocked_futures", &shared.blocked_futures)
                .field("mmap_ptr", &shared.ptr)
                .field("mmap_size", &shared.size);
        }

        f.finish()
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
pub(crate) struct OpIndex(usize);

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
pub(crate) struct QueueFull(());

impl From<QueueFull> for io::Error {
    fn from(_: QueueFull) -> io::Error {
        #[cfg(not(feature = "nightly"))]
        let kind = io::ErrorKind::Other;
        #[cfg(feature = "nightly")]
        let kind = io::ErrorKind::ResourceBusy;
        io::Error::new(kind, "submission queue is full")
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

    // NOTE: the following field is constant. we read them once from the mmap
    // area and then copied them here to avoid the need for the atomics.
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

    /// Return `true` if `IORING_CQE_F_BUFFER` is set.
    const fn is_buffer_select(&self) -> bool {
        self.inner.flags & libc::IORING_CQE_F_BUFFER != 0
    }

    const fn flags(&self) -> u16 {
        (self.inner.flags & ((1 << libc::IORING_CQE_BUFFER_SHIFT) - 1)) as u16
    }

    /// Returns the operation flags that need to be passed to
    /// [`QueuedOperation`].
    const fn operation_flags(&self) -> u16 {
        if self.is_buffer_select() {
            (self.inner.flags >> libc::IORING_CQE_BUFFER_SHIFT) as u16
        } else {
            0
        }
    }
}

impl fmt::Debug for Completion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Completion")
            .field("user_data", &self.inner.user_data)
            // NOTE this this isn't always an errno, so we can't use
            // `io::Error::from_raw_os_error` without being misleading.
            .field("res", &self.inner.res)
            .field("flags", &self.flags())
            .field("operation_flags", &self.operation_flags())
            .finish()
    }
}

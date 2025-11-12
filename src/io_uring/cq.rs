use std::marker::PhantomData;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use std::{fmt, io, ptr};

use crate::io_uring::{self, libc, load_atomic_u32, mmap, munmap, Shared};
use crate::msg::Message;
use crate::op::OpResult;
use crate::{debug_detail, syscall, OperationId};

#[derive(Debug)]
pub(crate) struct Completions {
    /// Mmap-ed pointer to the completion queue.
    ptr: *mut libc::c_void,
    /// Mmap-ed size in bytes.
    size: libc::c_uint,
    // NOTE: the following fields reference mmaped pages shared with the kernel,
    // thus all need atomic/synchronised access.
    /// Incremented by us when completions have been read.
    head: *mut AtomicU32,
    /// Incremented by the kernel when adding completions.
    tail: *const AtomicU32,
    /// Array of `len` completion entries shared with the kernel. The kernel
    /// modifies this array, we're only reading from it.
    entries: *const Completion,
    /// Number of `entries`.
    entries_len: u32,
    /// Mask used to index into the `entries` queue.
    entries_mask: u32,
}

impl Completions {
    pub(crate) fn new(
        rfd: BorrowedFd<'_>,
        parameters: &libc::io_uring_params,
    ) -> io::Result<Completions> {
        let size = parameters.cq_off.cqes
            + parameters.cq_entries * (size_of::<libc::io_uring_cqe>() as u32);
        let completion_queue = mmap(
            size as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_POPULATE,
            rfd.as_raw_fd(),
            libc::off_t::from(libc::IORING_OFF_CQ_RING),
        )?;

        let entries_len = unsafe {
            load_atomic_u32(completion_queue.add(parameters.cq_off.ring_entries as usize))
        };
        debug_assert!(entries_len == parameters.cq_entries);
        let entries_mask =
            unsafe { load_atomic_u32(completion_queue.add(parameters.cq_off.ring_mask as usize)) };
        debug_assert!(entries_mask == parameters.cq_entries - 1);

        unsafe {
            Ok(Completions {
                ptr: completion_queue,
                size,
                // Fields are shared with the kernel.
                head: completion_queue.add(parameters.cq_off.head as usize).cast(),
                tail: completion_queue.add(parameters.cq_off.tail as usize).cast(),
                entries: completion_queue.add(parameters.cq_off.cqes as usize).cast(),
                entries_len,
                entries_mask,
            })
        }
    }

    /// Make the `io_uring_enter` system call.
    #[allow(clippy::unused_self, clippy::needless_pass_by_ref_mut)]
    fn enter(&mut self, shared: &io_uring::Shared, timeout: Option<Duration>) -> io::Result<()> {
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

        let submissions = if shared.kernel_thread {
            0 // Kernel thread handles the submissions.
        } else {
            shared.unsubmitted()
        };

        // If there are no completions we'll wait for at least one.
        let enter_flags = libc::IORING_ENTER_GETEVENTS // Wait for a completion.
            | libc::IORING_ENTER_EXT_ARG; // Passing of `args`.
        log::debug!(submissions = submissions; "waiting for completion events");
        let result = syscall!(io_uring_enter2(
            shared.rfd.as_raw_fd(),
            submissions,
            1, // Wait for at least one completion.
            enter_flags,
            ptr::from_ref(&args).cast(),
            size_of::<libc::io_uring_getevents_arg>(),
        ));
        match result {
            Ok(_) => Ok(()),
            // Hit timeout or got interrupted, we can ignore it.
            Err(ref err) if matches!(err.raw_os_error(), Some(libc::ETIME | libc::EINTR)) => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// Returns `Completions.head`.
    fn completion_head(&mut self) -> u32 {
        // SAFETY: we're the only once writing to it so `Relaxed` is fine. The
        // pointer itself is valid as long as `Ring.fd` is alive.
        unsafe { (*self.head).load(Ordering::Relaxed) }
    }

    /// Returns `Completions.tail`.
    fn completion_tail(&self) -> u32 {
        // SAFETY: this written to by the kernel so we need to use `Acquire`
        // ordering. The pointer itself is valid as long as `Ring.fd` is alive.
        unsafe { (*self.tail).load(Ordering::Acquire) }
    }
}

impl crate::cq::Completions for Completions {
    type Shared = Shared;
    type Event = Completion;

    fn poll<'a>(
        &'a mut self,
        shared: &Self::Shared,
        timeout: Option<Duration>,
    ) -> io::Result<impl Iterator<Item = &'a Self::Event>> {
        let head = self.completion_head();
        let mut tail = self.completion_tail();
        if head == tail && !matches!(timeout, Some(Duration::ZERO)) {
            // If we have no completions and we have no, or a non-zero, timeout
            // we make a system call to wait for completion events.
            self.enter(shared, timeout)?;
            // NOTE: we're the only onces writing to the completion `head` so we
            // don't need to read it again.
            tail = self.completion_tail();
        }

        Ok(CompletionsIter {
            entries: self.entries,
            local_head: head,
            head: self.head,
            tail,
            mask: self.entries_mask,
            _lifetime: PhantomData,
        })
    }

    fn queue_space(&mut self, shared: &Self::Shared) -> usize {
        let kernel_read = shared.kernel_read();
        let pending_tail = shared.pending_tail();
        (self.entries_len - (pending_tail - kernel_read)) as usize
    }
}

unsafe impl Send for Completions {}

unsafe impl Sync for Completions {}

impl Drop for Completions {
    fn drop(&mut self) {
        if let Err(err) = munmap(self.ptr, self.size as usize) {
            log::warn!(ptr:? = self.ptr, size = self.size; "error unmapping io_uring completions: {err}");
        }
    }
}

/// Iterator of completed operations.
struct CompletionsIter<'a> {
    /// Same as [`Completions.entries`].
    entries: *const Completion,
    /// Local version of `head`. Used to update `head` once `Completions` is
    /// dropped.
    local_head: u32,
    /// Same as [`Completions.head`], used to let the kernel know we've read
    /// the completions once we're dropped.
    head: *mut AtomicU32,
    /// Tail of `entries`, i.e. number of completions the kernel wrote.
    tail: u32,
    /// Same as [`Completions.entries_mask`].
    mask: u32,
    /// We're depend on the lifetime of [`io_uring::Shared`].
    _lifetime: PhantomData<&'a io_uring::Shared>,
}

impl<'a> Iterator for CompletionsIter<'a> {
    type Item = &'a Completion;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.local_head < self.tail {
                // SAFETY: the `mask` ensures we can never get an `idx` larger then
                // the size of the queue. We checked above that the kernel has
                // written the struct (and isn't writing to now) so we can safely
                // read from it.
                let idx = (self.local_head & self.mask) as usize;
                let completion = unsafe { &*self.entries.add(idx) };
                self.local_head += 1;

                // Skip the completion events that are inserted as padding to
                // fill gaps in the ring.
                if completion.0.flags & libc::IORING_CQE_F_SKIP != 0 {
                    let id = crate::cq::Event::id(completion);
                    log::trace!(id = id, completion:? = completion; "skipping completion");
                    continue;
                }

                return Some(completion);
            }
            return None;
        }
    }
}

impl<'a> Drop for CompletionsIter<'a> {
    fn drop(&mut self) {
        // Let the kernel know we've read the completions.
        // SAFETY: the kernel needs to read the value so we need `Release`. The
        // pointer itself is valid as long as `Ring.fd` is alive.
        unsafe { (*self.head).store(self.local_head, Ordering::Release) }
    }
}

/// Event that represents a completed operation.
#[repr(transparent)]
pub(crate) struct Completion(libc::io_uring_cqe);

impl Completion {
    /// Returns the operation flags that need to be passed to
    /// [`QueuedOperation`].
    ///
    /// [`QueuedOperation`]: crate::QueuedOperation
    const fn operation_flags(&self) -> u16 {
        if self.0.flags & libc::IORING_CQE_F_BUFFER != 0 {
            (self.0.flags >> libc::IORING_CQE_BUFFER_SHIFT) as u16
        } else {
            0
        }
    }
}

impl crate::cq::Event for Completion {
    type State = OperationState;

    fn id(&self) -> OperationId {
        self.0.user_data as OperationId
    }

    fn update_state(&self, state: &mut Self::State) -> bool {
        let completion = CompletionResult {
            result: self.0.res,
            flags: self.operation_flags(),
        };
        match state {
            OperationState::Single { .. } if self.0.flags & libc::IORING_CQE_F_NOTIF != 0 => {
                // Zero copy completed, we can now mark ourselves as done, not
                // overwriting result.
            }
            OperationState::Single { result } => {
                debug_assert_eq!(*result, CompletionResult::empty());
                *result = completion;
            }
            OperationState::Multishot { results } => {
                results.push(completion);
            }
        }
        // IORING_CQE_F_MORE indicates that more completions are coming for this
        // operation.
        self.0.flags & libc::IORING_CQE_F_MORE != 0
    }
}

impl fmt::Debug for Completion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        debug_detail!(
            bitset CompletionFlags(u32),
            libc::IORING_CQE_F_BUFFER,
            libc::IORING_CQE_F_MORE,
            libc::IORING_CQE_F_SOCK_NONEMPTY,
            libc::IORING_CQE_F_NOTIF,
            libc::IORING_CQE_F_BUF_MORE,
        );

        f.debug_struct("io_uring::Completion")
            .field("user_data", &self.0.user_data)
            // NOTE this this isn't always an errno, so we can't use
            // `io::Error::from_raw_os_error` without being misleading.
            .field("res", &self.0.res)
            .field("flags", &CompletionFlags(self.0.flags))
            .field("operation_flags", &self.operation_flags())
            .finish()
    }
}

#[derive(Debug)]
pub(crate) enum OperationState {
    /// Single result operation.
    Single {
        /// Result of the operation.
        result: CompletionResult,
    },
    /// Multishot operation, which expects multiple results for the same
    /// operation.
    Multishot {
        /// Results for the operation.
        results: Vec<CompletionResult>,
    },
}

impl crate::cq::OperationState for OperationState {
    fn new() -> OperationState {
        OperationState::Single {
            result: CompletionResult::empty(),
        }
    }

    fn new_multishot() -> OperationState {
        OperationState::Multishot {
            results: Vec::new(),
        }
    }

    fn prep_retry(&mut self) {
        match self {
            OperationState::Single { result } => {
                *result = CompletionResult::empty();
            }
            OperationState::Multishot { .. } => { /* We'll continue to collect the results. */ }
        }
    }
}

/// Completed result of an operation.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub(crate) struct CompletionResult {
    /// The 16 upper bits of `io_uring_cqe.flags`, e.g. the index of a buffer in
    /// a buffer pool.
    flags: u16,
    /// The result of an operation; negative is a (negative) errno, positive a
    /// successful result. The meaning is depended on the operation itself.
    result: i32,
}

impl CompletionResult {
    const fn empty() -> CompletionResult {
        CompletionResult {
            flags: u16::MAX,
            result: i32::MIN,
        }
    }

    #[allow(clippy::cast_sign_loss)]
    pub(crate) fn as_op_return(self) -> OpResult<OpReturn> {
        if self.result.is_negative() {
            OpResult::Err(io::Error::from_raw_os_error(-self.result))
        } else {
            // SAFETY: checked if `result` is negative above.
            OpResult::Ok((self.flags, self.result as u32))
        }
    }

    #[allow(clippy::cast_sign_loss)]
    pub(crate) const fn as_msg(self) -> Message {
        self.result as Message
    }
}

/// Return value of a system call.
///
/// The flags and positive result of a system call.
pub(crate) type OpReturn = (u16, u32);

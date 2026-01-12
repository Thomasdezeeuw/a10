use std::cmp::min;
use std::mem::{swap, take};
use std::os::fd::RawFd;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use std::{fmt, io, ptr};

use crate::io_uring::{Shared, libc, load_kernel_shared, mmap, munmap, op};
use crate::{asan, debug_detail, lock};

#[derive(Debug)]
pub(crate) struct Completions {
    /// Pointer and length to the mmaped page(s).
    ring: ptr::NonNull<libc::c_void>,
    ring_len: u32,
    // NOTE: the following fields reference mmaped pages shared with the kernel,
    // thus all need atomic/synchronised access.
    /// Incremented by us when completions have been read.
    entries_head: ptr::NonNull<AtomicU32>,
    /// Incremented by the kernel when adding completions.
    entries_tail: ptr::NonNull<AtomicU32>,
    /// Array of [`Completions::entries_len`] completion entries shared with the
    /// kernel. The kernel modifies this array, we're only reading from it.
    entries: ptr::NonNull<Completion>,
    // Fixed values that don't change after the setup.
    /// Number of `entries`.
    entries_len: u32,
}

impl Completions {
    pub(crate) fn new(rfd: RawFd, parameters: &libc::io_uring_params) -> io::Result<Completions> {
        let entries_len = parameters.cq_entries * (size_of::<Completion>() as u32);
        let ring_len = parameters.cq_off.cqes + entries_len;
        let ring = mmap(
            ring_len as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_POPULATE,
            rfd,
            libc::off_t::from(libc::IORING_OFF_CQ_RING),
        )?;

        // The entries are written by the kernel and can only be read based on
        // the `tail` (also written by the kernel), see `poll` below.
        let entries = unsafe { ring.add(parameters.cq_off.cqes as usize).cast() };
        asan::poison_region(entries.cast().as_ptr(), entries_len as usize);

        // SAFETY: we do a whole bunch of pointer manipulations, the kernel
        // ensures all of this stuff is set up for us with the mmap calls above.
        Ok(Completions {
            ring,
            ring_len,
            entries_head: unsafe { ring.add(parameters.cq_off.head as usize).cast() },
            entries_tail: unsafe { ring.add(parameters.cq_off.tail as usize).cast() },
            entries,
            entries_len: parameters.cq_entries,
        })
    }

    pub(crate) fn poll(&mut self, shared: &Shared, timeout: Option<Duration>) -> io::Result<()> {
        let mut head = load_kernel_shared(self.entries_head);
        let mut tail = load_kernel_shared(self.entries_tail);
        if head >= tail {
            // If we have no completions we make a system call to wait for
            // completion events.
            self.enter(shared, timeout)?;
            tail = load_kernel_shared(self.entries_tail);
        }

        debug_assert!(tail >= head);
        while head < tail {
            let index = (head & (self.entries_len - 1)) as usize;
            // SAFETY: see below.
            let ptr = unsafe { self.entries.add(index).as_ptr() };
            asan::unpoison_region(ptr.cast(), size_of::<Completion>());
            // SAFETY: the pointer is valid and we've ensured above that the
            // kernel has written a new completion.
            let completion = unsafe { &*ptr };
            log::trace!(completion:?, index, head; "dequeued completion");
            // SAFETY: we're only processing the completion once.
            unsafe { completion.process() };
            asan::poison_region(ptr.cast(), size_of::<Completion>());
            head += 1;
        }

        // Let the kernel write more completions.
        unsafe { (&*self.entries_head.as_ptr()).store(head, Ordering::Release) };

        self.wake_blocked_futures(shared);

        Ok(())
    }

    /// Make the `io_uring_enter` system call.
    #[allow(clippy::unused_self, clippy::needless_pass_by_ref_mut)]
    fn enter(&mut self, shared: &Shared, timeout: Option<Duration>) -> io::Result<()> {
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
            shared.unsubmitted_submissions()
        };

        // If there are no completions we'll wait for at least one.
        let flags = libc::IORING_ENTER_GETEVENTS // Wait for a completion.
            | libc::IORING_ENTER_EXT_ARG; // Passing of `args`.
        log::trace!(submissions, timeout:?; "waiting for completion events");
        shared.is_polling.store(true, Ordering::Release);
        let result = shared.enter(
            submissions,
            1, // Wait for at least one completion.
            flags,
            ptr::from_ref(&args).cast(),
            size_of::<libc::io_uring_getevents_arg>(),
        );
        shared.is_polling.store(false, Ordering::Release);
        match result {
            Ok(_) => Ok(()),
            // Hit timeout or got interrupted, we can ignore it.
            Err(ref err) if matches!(err.raw_os_error(), Some(libc::ETIME | libc::EINTR)) => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// Wake any futures that were blocked on a submission slot.
    // Work around <https://github.com/rust-lang/rust-clippy/issues/8539>.
    #[allow(clippy::iter_with_drain, clippy::needless_pass_by_ref_mut)]
    fn wake_blocked_futures(&mut self, shared: &Shared) {
        let available = (shared.submissions_len - shared.unsubmitted_submissions()) as usize;
        if available == 0 {
            return;
        }

        let mut blocked_futures = lock(&shared.blocked_futures);
        if blocked_futures.is_empty() {
            return;
        }

        let mut wakers = take(&mut *blocked_futures);
        drop(blocked_futures); // Unblock others.
        let awoken = min(available, wakers.len());
        for waker in wakers.drain(..awoken) {
            log::trace!(waker:?; "waking up future for submission");
            waker.wake();
        }

        // Reuse allocation.
        let mut blocked_futures = lock(&shared.blocked_futures);
        swap(&mut *blocked_futures, &mut wakers);
        if wakers.len() <= available - awoken {
            drop(blocked_futures); // Unblock others.
            for waker in wakers {
                log::trace!(waker:?; "waking up future for submission");
                waker.wake();
            }
        } else {
            // Can't wake up all the additional waiting futures, so add them
            // back to the waiting list.
            blocked_futures.extend(wakers);
            drop(blocked_futures); // Unblock others.
        }
    }
}

unsafe impl Send for Completions {}

unsafe impl Sync for Completions {}

impl Drop for Completions {
    fn drop(&mut self) {
        let entries_len = (self.entries_len as usize) * size_of::<Completion>();
        asan::poison_region(self.entries.as_ptr().cast(), entries_len);

        let ptr = self.ring;
        let len = self.ring_len as usize;
        if let Err(err) = munmap(ptr, len) {
            log::warn!(ptr:?, len; "error unmapping io_uring submission ring: {err}");
        }
    }
}

/// Event that represents a completion.
#[repr(transparent)]
pub(crate) struct Completion(pub(super) libc::io_uring_cqe);

pub(super) const MULTISHOT_TAG: usize = 0b01;
const TAG_MASK: usize = !MULTISHOT_TAG;

/// User data set for completions that can be ignored. For example when we're
/// only interested in waking the polling thread.
const NO_PROCESS: usize = 0;

pub(super) const WAKE_USER_DATA: u64 = NO_PROCESS as u64;

impl Completion {
    /// Process the completion event.
    ///
    /// # Safety
    ///
    /// MUST only be called once.
    unsafe fn process(&self) {
        // Skip the completion events that are inserted as padding to fill gaps
        // in the ring.
        if self.0.flags & libc::IORING_CQE_F_SKIP != 0 {
            return;
        }

        let user_data = self.0.user_data as usize;
        if user_data == NO_PROCESS {
            return;
        }

        let ptr: *const () = ptr::with_exposed_provenance(user_data);
        let update = if ptr.addr() & MULTISHOT_TAG == 0 {
            const _ALIGNMENT_CHECK: () = assert!(align_of::<op::SingleShared>() > 1);
            let head: &op::SingleShared = unsafe { &*ptr.cast() };
            lock(&head).update(self)
        } else {
            const _ALIGNMENT_CHECK: () = assert!(align_of::<op::MultiShared>() > 1);
            let head: &op::MultiShared = unsafe { &*ptr.map_addr(|addr| addr & TAG_MASK).cast() };
            lock(&head).update(self)
        };
        match update {
            op::StatusUpdate::Ok => { /* Done. */ }
            op::StatusUpdate::Wake(waker) => {
                log::trace!(waker:?; "waking up future to make progress");
                waker.wake()
            }
            op::StatusUpdate::Drop { drop, ptr } => {
                log::trace!(ptr:?; "dropping operation state");
                // SAFETY: `update` told use to drop all the operation data. The
                // operation future itself ensures that we only get here if the
                // Future is dropped and the state is no longer used.
                unsafe { drop(ptr) }
            }
        };
    }

    /// Returns the operation flags that need to be passed to
    /// [`QueuedOperation`].
    ///
    /// [`QueuedOperation`]: crate::QueuedOperation
    pub(super) const fn operation_flags(&self) -> u16 {
        if self.0.flags & libc::IORING_CQE_F_BUFFER != 0 {
            (self.0.flags >> libc::IORING_CQE_BUFFER_SHIFT) as u16
        } else {
            0
        }
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
            .field("user_data", &(self.0.user_data as *const ()))
            // NOTE this this isn't always an errno, so we can't use
            // `io::Error::from_raw_os_error` without being misleading.
            .field("res", &self.0.res)
            .field("flags", &CompletionFlags(self.0.flags))
            .field("operation_flags", &self.operation_flags())
            .finish()
    }
}

#![feature(generic_associated_types)]
#![allow(incomplete_features)]

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

use std::cmp::min;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::atomic::{AtomicU32, Ordering};
use std::{fmt, io, ptr, slice};

use log::error;

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

// TODO: replace this with definitions from the `libc` crate once available.
mod sys;
use sys as libc;

pub use config::Config;

#[derive(Debug)]
pub struct Ring {
    fd: RawFd,
    sq: SubmissionQueue,
    cq: CompletionQueue,
}

#[derive(Debug)]
struct SubmissionQueue {
    /// Mmap-ed pointer.
    ptr: *mut libc::c_void,
    /// Mmap-ed size in bytes.
    size: libc::c_uint,

    /// Local version of `tail`.
    /// Increased in `queue` and synced with the kernel (`tail`) in `wait`.
    pending_tail: u32,

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
    /// Array of `len` submission entries shared with the kernel. We're the only
    /// one modifiying the structures, but the kernel can read from it.
    ///
    /// This pointer is also used in the `unmmap` call.
    entries: *mut Submission,
    /// Array of `len` indices (into `entries`) shared with the kernel. We're
    /// the only one modifiying the structures, but the kernel can read from it.
    array: *mut AtomicU32,
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
    /// Number of completion events lost because the queue was full.
    overflow: *const AtomicU32,
    /// Array of `len` completion entries shared with the kernel. The kernel
    /// modifies this array, we're only reading from it.
    entries: *const Completion,
}

impl Ring {
    /// Create a new `Ring`.
    ///
    /// For more configuration options see [`Config`].
    pub fn new(entries: u32) -> io::Result<Ring> {
        Config::new(entries).build()
    }

    /// Queue a submission.
    ///
    /// Returns an error if the submission queue is full. To fix this call
    /// [`Ring::wait_for`] (and handle the completed operations) and try
    /// queueing again.
    pub fn queue<F>(&mut self, submit: F) -> Result<(), QueueFull>
    where
        // TODO: how do we force the user to change the submission?
        // Maybe create a opeque type returned by `submission` modifying
        // functions?
        F: FnOnce(&mut Submission),
    {
        let head = self.submission_head();
        let tail = self.sq.pending_tail;
        let next = tail + 1; // TODO: handle overflows.

        if next - head <= self.sq.len {
            // SAFETY: the `ring_mask` ensures we can never get an `idx` larger
            // then the size of the queue. We checked above that the kernel
            // isn't reading from the struct and we can safely write to it.
            let idx = (tail & self.sq.ring_mask) as usize;
            let submission = unsafe { &mut *self.sq.entries.add(idx) };

            // Let the user fill the submission.
            #[cfg(debug_assertions)]
            submission.reset();
            submit(submission);
            debug_assert!(!submission.is_unchanged());

            // Mark the submission as filled with the kernel.
            // SAFETY: `idx` is masked above to be within the correct bounds.
            // TODO: why is Relaxed acceptable?
            unsafe { (&*self.sq.array.add(idx)).store(idx as u32, Ordering::Relaxed) }
            self.sq.pending_tail = next;
            Ok(())
        } else {
            Err(QueueFull(()))
        }
    }

    /// Submit all submissions and wait for at least one completion.
    ///
    /// Also see [`Ring::wait_for`].
    pub fn wait(&mut self) -> io::Result<Completions> {
        self.wait_for(1)
    }

    /// Submit all submissions and wait for at least `n` completions.
    ///
    /// Setting `n` to zero will submit all queued operations and return any
    /// completions, without blocking.
    pub fn wait_for(&mut self, n: u32) -> io::Result<Completions> {
        let to_submit = {
            let pending_tail = self.sq.pending_tail;
            let tail = self.submission_tail();
            let to_submit = pending_tail - tail;
            if to_submit != 0 {
                // Let the kernel know we've submitted some events.
                // SAFETY: the kernel needs to read the value so we need `Release`.
                // The pointer itself is valid as long as `Ring.fd` is alive.
                unsafe { (&*self.sq.tail).store(pending_tail, Ordering::Release) }
            }
            to_submit
        };

        // TODO: reread manual when using `IORING_SETUP_IOPOLL`.
        // TODO: reread manual when using `IORING_ENTER_SQ_WAIT`.
        // TODO: reread manual when using `IORING_SETUP_SQPOLL`, using `IORING_ENTER_SQ_WAKEUP`.
        let flags = libc::IORING_ENTER_GETEVENTS; // Wait for at least `n` events.
        let n = syscall!(io_uring_enter(self.fd, to_submit, n, flags, ptr::null(), 0))?;

        // TODO: check `overflow`?

        let head = self.completion_head();
        let tail = self.completion_tail();
        debug_assert_eq!(head + n as u32, tail);
        Ok(Completions {
            entries: self.cq.entries,
            local_head: head,
            head: self.cq.head,
            tail,
            ring_mask: self.cq.ring_mask,
            _lifetime: PhantomData,
        })
    }

    /// Returns `SubmissionQueue.head`.
    fn submission_head(&self) -> u32 {
        // SAFETY: this written to by the kernel so we need to use `Acquire`
        // ordering. The pointer itself is valid as long as `Ring.fd` is alive.
        unsafe { (&*self.sq.head).load(Ordering::Acquire) }
    }

    /// Returns `SubmissionQueue.tail`.
    fn submission_tail(&self) -> u32 {
        // SAFETY: we're the only once writing to it so `Relaxed` is fine. The
        // pointer itself is valid as long as `Ring.fd` is alive.
        unsafe { (&*self.sq.tail).load(Ordering::Relaxed) }
    }

    /// Returns `CompletionQueue.head`.
    fn completion_head(&self) -> u32 {
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

impl Drop for Ring {
    fn drop(&mut self) {
        if let Err(err) = syscall!(close(self.fd)) {
            error!("error closing io_uring: {}", err);
        }
    }
}

impl Drop for SubmissionQueue {
    fn drop(&mut self) {
        // FIXME: do we need to unmap here? Or is closing the fd enough.
    }
}

impl Drop for CompletionQueue {
    fn drop(&mut self) {
        // FIXME: do we need to unmap here? Or is closing the fd enough.
    }
}

/// Error returned by [`Ring::queue`] when the submission queue is full.
pub struct QueueFull(());

impl fmt::Debug for QueueFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueueFull").finish()
    }
}

impl fmt::Display for QueueFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("io_uring submission queue is full")
    }
}

/// Associates [`Submission`]s with [`Completion`]s.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Token(pub u64);

#[repr(transparent)]
pub struct Submission {
    inner: libc::io_uring_sqe,
}

/// The manual says:
/// > If offs is set to -1, the offset will use (and advance) the file
/// > position, like the read(2) and write(2) system calls.
///
/// `-1` cast as `unsigned long long` in C is the same as as `u64::MAX`.
const NO_OFFSET: u64 = u64::MAX;

impl Submission {
    /// Reset the submission.
    #[cfg(debug_assertions)]
    fn reset(&mut self) {
        debug_assert!(Operation::Nop as u8 == 0);
        unsafe { (&mut self.inner as *mut libc::io_uring_sqe).write_bytes(0, 1) }
    }

    /// Returns `true` if the submission is unchanged after a [`reset`].
    ///
    /// [`reset`]: Submission::reset
    #[cfg(debug_assertions)]
    fn is_unchanged(&self) -> bool {
        self.inner.opcode == Operation::Nop as u8
            && self.inner.flags == 0
            && self.inner.user_data == 0
    }

    /*
    /// Set the `Operation` to [`Operation::Nop`].
    fn nop(&mut self) {
        self.inner.opcode = Operation::Nop as u8;
    }
    */

    pub unsafe fn read_vectored_at<F>(
        &mut self,
        token: Token,
        fd: &F,
        bufs: &mut [MaybeUninitSlice<'_>], // FIXME: lifetime.
        offset: u64,
    ) where
        F: io::Read + AsRawFd,
    {
        self.inner.opcode = Operation::Readv as u8;
        self.inner.fd = fd.as_raw_fd();
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: offset };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: bufs.as_mut_ptr() as _,
        };
        self.inner.len = min(bufs.len(), u32::MAX as usize) as u32;
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 { rw_flags: 0 };
        self.inner.user_data = token.0;
    }

    /// Create a read submission.
    ///
    /// Avaialable since Linux kernel 5.6.
    pub unsafe fn read<F>(
        &mut self,
        token: Token,
        fd: &F,
        // FIXME: use `MaybeInit`.
        buf: &mut [u8], // FIXME: lifetime.
    ) where
        F: io::Read + AsRawFd,
    {
        self.read_at(token, fd, buf, NO_OFFSET)
    }

    /// Create a read submission starting at `offset`.
    ///
    /// Avaialable since Linux kernel 5.6.
    pub unsafe fn read_at<F>(
        &mut self,
        token: Token,
        fd: &F,
        // FIXME: use `MaybeInit`.
        buf: &mut [u8], // FIXME: lifetime.
        offset: u64,
    ) where
        F: io::Read + AsRawFd,
    {
        self.inner.opcode = Operation::Read as u8;
        self.inner.fd = fd.as_raw_fd();
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: offset };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: buf.as_mut_ptr() as _,
        };
        self.inner.len = min(buf.len(), u32::MAX as usize) as u32;
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 { rw_flags: 0 };
        self.inner.user_data = token.0;
    }

    // TODO: add other operations, see `io_uring_enter` manual.

    /// Set the I/O priority.
    ///
    /// See the [`io_prio_get(2)`] manual for more information.
    ///
    /// [`io_prio_get(2)`]: https://man7.org/linux/man-pages/man2/ioprio_get.2.html
    pub fn set_io_priority(&mut self, io_prio: u16) {
        self.inner.ioprio = io_prio;
    }
}

impl fmt::Debug for Submission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let operation = Operation::from_u8(self.inner.opcode);
        let mut d = f.debug_struct("Submission");
        d.field("opcode", &operation)
            .field("flags", &self.inner.flags)
            .field("ioprio", &self.inner.ioprio)
            .field("fd", &self.inner.fd);
        match operation {
            Operation::Read => {
                d.field("off", unsafe { &self.inner.__bindgen_anon_1.off })
                    .field("addr", unsafe {
                        &(self.inner.__bindgen_anon_2.addr as *const libc::c_void)
                    });
            }
            _ => { /* TODO. */ }
        }
        d.field("len", &self.inner.len);
        match operation {
            Operation::Read => {
                d.field("rw_flags", unsafe { &self.inner.__bindgen_anon_3.rw_flags });
            }
            _ => { /* TODO. */ }
        }
        d.field("user_data", &Token(self.inner.user_data)).finish()
    }
}

#[derive(Debug)]
#[repr(u8)]
enum Operation {
    /// Do not perform any I/O.
    #[doc(alias = "IORING_OP_NOP")]
    Nop = libc::IORING_OP_NOP as u8,
    /// Vectored read operation.
    Readv = libc::IORING_OP_READV as u8,
    /// Vectored write operation.
    Writev = libc::IORING_OP_WRITEV as u8,
    /// File sync.
    Fsync = libc::IORING_OP_FSYNC as u8,
    /// Read from pre-mapped buffers.
    ReadFixed = libc::IORING_OP_READ_FIXED as u8,
    /// Write to pre-mapped buffers.
    WriteFixed = libc::IORING_OP_WRITE_FIXED as u8,
    PollAdd = libc::IORING_OP_POLL_ADD as u8,
    PollRemove = libc::IORING_OP_POLL_REMOVE as u8,
    SyncFileRange = libc::IORING_OP_SYNC_FILE_RANGE as u8,
    Sendmsg = libc::IORING_OP_SENDMSG as u8,
    Recvmsg = libc::IORING_OP_RECVMSG as u8,
    Timeout = libc::IORING_OP_TIMEOUT as u8,
    TimeoutFemove = libc::IORING_OP_TIMEOUT_REMOVE as u8,
    Accept = libc::IORING_OP_ACCEPT as u8,
    AsyncCancel = libc::IORING_OP_ASYNC_CANCEL as u8,
    LinkTimeout = libc::IORING_OP_LINK_TIMEOUT as u8,
    Connect = libc::IORING_OP_CONNECT as u8,
    Fallocate = libc::IORING_OP_FALLOCATE as u8,
    Openat = libc::IORING_OP_OPENAT as u8,
    Close = libc::IORING_OP_CLOSE as u8,
    FilesUpdate = libc::IORING_OP_FILES_UPDATE as u8,
    Statx = libc::IORING_OP_STATX as u8,
    Read = libc::IORING_OP_READ as u8,
    Write = libc::IORING_OP_WRITE as u8,
    Fadvise = libc::IORING_OP_FADVISE as u8,
    Madvise = libc::IORING_OP_MADVISE as u8,
    Send = libc::IORING_OP_SEND as u8,
    Recv = libc::IORING_OP_RECV as u8,
    Openat2 = libc::IORING_OP_OPENAT2 as u8,
    EpollCtl = libc::IORING_OP_EPOLL_CTL as u8,
    Splice = libc::IORING_OP_SPLICE as u8,
    ProvideBuffers = libc::IORING_OP_PROVIDE_BUFFERS as u8,
    RemoveBuffers = libc::IORING_OP_REMOVE_BUFFERS as u8,
    Tee = libc::IORING_OP_TEE as u8,
    Shutdown = libc::IORING_OP_SHUTDOWN as u8,
    Renameat = libc::IORING_OP_RENAMEAT as u8,
    Unlinkat = libc::IORING_OP_UNLINKAT as u8,
    Last = libc::IORING_OP_LAST as u8,

    #[doc(hidden)]
    Unknown = u8::MAX,
}

impl Operation {
    const fn from_u8(value: u8) -> Operation {
        match value as _ {
            libc::IORING_OP_NOP => Operation::Nop,
            libc::IORING_OP_READV => Operation::Readv,
            libc::IORING_OP_WRITEV => Operation::Writev,
            libc::IORING_OP_FSYNC => Operation::Fsync,
            libc::IORING_OP_READ_FIXED => Operation::ReadFixed,
            libc::IORING_OP_WRITE_FIXED => Operation::WriteFixed,
            libc::IORING_OP_POLL_ADD => Operation::PollAdd,
            libc::IORING_OP_POLL_REMOVE => Operation::PollRemove,
            libc::IORING_OP_SYNC_FILE_RANGE => Operation::SyncFileRange,
            libc::IORING_OP_SENDMSG => Operation::Sendmsg,
            libc::IORING_OP_RECVMSG => Operation::Recvmsg,
            libc::IORING_OP_TIMEOUT => Operation::Timeout,
            libc::IORING_OP_TIMEOUT_REMOVE => Operation::TimeoutFemove,
            libc::IORING_OP_ACCEPT => Operation::Accept,
            libc::IORING_OP_ASYNC_CANCEL => Operation::AsyncCancel,
            libc::IORING_OP_LINK_TIMEOUT => Operation::LinkTimeout,
            libc::IORING_OP_CONNECT => Operation::Connect,
            libc::IORING_OP_FALLOCATE => Operation::Fallocate,
            libc::IORING_OP_OPENAT => Operation::Openat,
            libc::IORING_OP_CLOSE => Operation::Close,
            libc::IORING_OP_FILES_UPDATE => Operation::FilesUpdate,
            libc::IORING_OP_STATX => Operation::Statx,
            libc::IORING_OP_READ => Operation::Read,
            libc::IORING_OP_WRITE => Operation::Write,
            libc::IORING_OP_FADVISE => Operation::Fadvise,
            libc::IORING_OP_MADVISE => Operation::Madvise,
            libc::IORING_OP_SEND => Operation::Send,
            libc::IORING_OP_RECV => Operation::Recv,
            libc::IORING_OP_OPENAT2 => Operation::Openat2,
            libc::IORING_OP_EPOLL_CTL => Operation::EpollCtl,
            libc::IORING_OP_SPLICE => Operation::Splice,
            libc::IORING_OP_PROVIDE_BUFFERS => Operation::ProvideBuffers,
            libc::IORING_OP_REMOVE_BUFFERS => Operation::RemoveBuffers,
            libc::IORING_OP_TEE => Operation::Tee,
            libc::IORING_OP_SHUTDOWN => Operation::Shutdown,
            libc::IORING_OP_RENAMEAT => Operation::Renameat,
            libc::IORING_OP_UNLINKAT => Operation::Unlinkat,
            libc::IORING_OP_LAST => Operation::Last,
            _ => Operation::Unknown,
        }
    }
}

/// Iterator of completed operations.
pub struct Completions<'ring> {
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
pub struct Completion {
    inner: libc::io_uring_cqe,
}

impl Completion {
    /// Returns the completion's token.
    pub fn token(&self) -> Token {
        Token(self.inner.user_data)
    }

    /// Get the result of the operation.
    pub fn result(&self) -> io::Result<u32> {
        // TODO: handle I/O uring specific errors here, read CQE ERRORS in the
        // manual.
        let res = self.inner.res;
        if res.is_negative() {
            Err(io::Error::from_raw_os_error(-res))
        } else {
            Ok(res as u32)
        }
    }
}

impl fmt::Debug for Completion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Completion")
            .field("user_data", &self.token())
            .field("res", &self.result())
            // NOTE: currently not used.
            .field("flags", &self.inner.flags)
            .finish()
    }
}

#[repr(transparent)]
pub struct MaybeUninitSlice<'a> {
    vec: libc::iovec,
    _lifetime: PhantomData<&'a mut [MaybeUninit<u8>]>,
}

impl<'a> MaybeUninitSlice<'a> {
    pub fn new(buf: &'a mut [MaybeUninit<u8>]) -> MaybeUninitSlice<'a> {
        MaybeUninitSlice {
            vec: libc::iovec {
                iov_base: buf.as_mut_ptr().cast(),
                iov_len: buf.len(),
            },
            _lifetime: PhantomData,
        }
    }

    pub fn as_slice(&self) -> &[MaybeUninit<u8>] {
        unsafe { slice::from_raw_parts(self.vec.iov_base.cast(), self.vec.iov_len) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [MaybeUninit<u8>] {
        unsafe { slice::from_raw_parts_mut(self.vec.iov_base.cast(), self.vec.iov_len) }
    }
}

impl<'a> fmt::Debug for MaybeUninitSlice<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_slice().fmt(f)
    }
}

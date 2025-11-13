//! Memory operations.

use std::io;

use crate::op::{Operation, operation};
use crate::{SubmissionQueue, man_link, new_flag, sys};

/// Give advice about use of memory.
///
/// Give advice or directions to the kernel about the address range beginning at
/// address `addr` and with size `length` bytes. In most cases, the goal of such
/// advice is to improve system or application performance.
#[doc = man_link!(madvise(2))]
#[doc(alias = "madvise")]
#[doc(alias = "posix_madvise")]
pub const fn advise(
    sq: SubmissionQueue,
    address: *mut (),
    length: u32,
    advice: AdviseFlag,
) -> Advise {
    Advise(Operation::new(sq, (), (address, length, advice)))
}

new_flag!(
    /// Advise about memory access.
    ///
    /// See [`advise`].
    pub struct AdviseFlag(u32) {
        /// No special treatment.
        NORMAL = libc::MADV_NORMAL,
        /// Expect page references in random order.
        RANDOM = libc::MADV_RANDOM,
        /// Expect page references in sequential order.
        SEQUENTIAL = libc::MADV_SEQUENTIAL,
        /// Expect access in the near future.
        WILL_NEED = libc::MADV_WILLNEED,
        /// Do not expect access in the near future.
        DONT_NEED = libc::MADV_DONTNEED,
        /// Free up a given range of pages and its associated backing store.
        REMOVE = libc::MADV_REMOVE,
        /// Do not make the pages in this range available to the child after a
        /// `fork(2)`.
        DONT_FORK = libc::MADV_DONTFORK,
        /// Undo the effect of `DONT_FORK`, restoring the default behavior,
        /// whereby a mapping is inherited across `fork(2)`.
        DO_FORK = libc::MADV_DOFORK,
        /// Poison the pages and handle subsequent references to those pages
        /// like a hardware memory corruption.
        HW_POISON = libc::MADV_HWPOISON,
        /// Enable Kernel Samepage Merging (KSM) for the pages.
        MERGEABLE = libc::MADV_MERGEABLE,
        /// Undo the effect of an earlier `MERGEABLE` operation.
        UNMERGEABLE = libc::MADV_UNMERGEABLE,
        /// Soft offline the pages in the range.
        SOFT_OFFLINE = libc::MADV_SOFT_OFFLINE,
        /// Enable Transparent Huge Pages (THP) for pages in the range.
        HUGE_PAGE = libc::MADV_HUGEPAGE,
        /// Ensures that memory in the address range will not be backed by
        /// transparent hugepages.
        NO_HUGE_PAGE = libc::MADV_NOHUGEPAGE,
        /// Perform a best-effort collapse of the native pages mapped by the
        /// memory range into Transparent Huge Pages (THPs).
        COLLAPSE = libc::MADV_COLLAPSE,
        /// Exclude from a core dump those pages in the range.
        DONT_DUMP = libc::MADV_DONTDUMP,
        /// Undo the effect of an earlier `DONT_DUMP`.
        DO_DUMP = libc::MADV_DODUMP,
        /// The application no longer requires the pages in the range.
        FREE = libc::MADV_FREE,
        /// Present the child process with zero-filled memory in this range
        /// after a `fork(2)`.
        WIPE_ON_FORK = libc::MADV_WIPEONFORK,
        /// Undo the effect of an earlier `WIPE_ON_FORK`.
        KEEP_ON_FORK = libc::MADV_KEEPONFORK,
        /// Deactivate a given range of pages.
        COLD = libc::MADV_COLD,
        /// Reclaim a given range of pages.
        PAGE_OUT = libc::MADV_PAGEOUT,
        /// Populate (prefault) page tables readable, faulting in all pages.
        POPULATE_READ = libc::MADV_POPULATE_READ,
        /// Populate (prefault) page tables writable, faulting in all pages.
        POPULATE_WRITE = libc::MADV_POPULATE_WRITE,
    }
);

operation!(
    /// [`Future`] behind [`advise`].
    pub struct Advise(sys::mem::AdviseOp) -> io::Result<()>;
);

// SAFETY: `!Send` due to address, but the future is `Send`.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Sync for Advise {}
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for Advise {}

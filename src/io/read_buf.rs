//! Module with read buffer pool.
//!
//! See [`ReadBufPool`].

use std::borrow::{Borrow, BorrowMut};
use std::mem::MaybeUninit;
use std::ops::{Bound, Deref, DerefMut, RangeBounds};
use std::ptr::{self, NonNull};
use std::sync::Arc;
use std::{fmt, io, slice};

use crate::io::{Buf, BufGroupId, BufId, BufMut};
use crate::{sys, SubmissionQueue};

/// A read buffer pool.
///
/// This is a special buffer pool that shares its buffers with the kernel. The
/// buffer pool is used by the kernel in `read(2)` and `recv(2)` like calls.
/// Instead of user space having to select a buffer before issueing the read
/// call, the kernel will select a buffer from the pool when it's ready for
/// reading. This avoids the need to have as many buffers as concurrent read
/// calls.
///
/// As a result of this the returned buffer, [`ReadBuf`], is somewhat limited.
/// For example it can't grow beyond the pool's buffer size. However it can be
/// used in write calls like any other buffer.
#[derive(Clone, Debug)]
#[allow(clippy::module_name_repetitions)] // Public in `crate::io`, so N/A.
pub struct ReadBufPool {
    /// Shared between one or more [`ReadBufPool`]s and one or more [`ReadBuf`]s.
    shared: Arc<sys::io::ReadBufPool>,
}

impl ReadBufPool {
    /// Create a new buffer pool.
    ///
    /// `pool_size` must be a power of 2, with a maximum of 2^15 (32768).
    /// `buf_size` is the maximum capacity of the buffer. Note that buffer can't
    /// grow beyond this capacity.
    #[doc(alias = "IORING_REGISTER_PBUF_RING")]
    pub fn new(sq: SubmissionQueue, pool_size: u16, buf_size: u32) -> io::Result<ReadBufPool> {
        let shared = sys::io::ReadBufPool::new(sq, pool_size, buf_size)?;
        Ok(ReadBufPool {
            shared: Arc::new(shared),
        })
    }

    /// Get a buffer reference to this pool.
    ///
    /// This can only be used in read I/O operations, such as [`AsyncFd::read`],
    /// but it won't yet select a buffer to use. This is done by the kernel once
    /// it actually has data to write into the buffer. Before it's used in a
    /// read call the returned buffer will be empty and can't be resized, it's
    /// effecitvely useless before a read call.
    ///
    /// [`AsyncFd::read`]: crate::AsyncFd::read
    pub fn get(&self) -> ReadBuf {
        ReadBuf {
            shared: self.shared.clone(),
            owned: None,
        }
    }

    /// Returns the group id for this pool.
    pub(crate) fn group_id(&self) -> BufGroupId {
        self.shared.group_id()
    }

    /// Initialise a new buffer with `index` with `len` size.
    ///
    /// # Safety
    ///
    /// The provided index must come from the kernel, reusing the same index
    /// will cause data races.
    pub(crate) unsafe fn new_buffer(&self, id: BufId, n: u32) -> ReadBuf {
        let owned = if n == 0 && id.0 == 0 {
            // If we read 0 bytes it means the kernel didn't actually allocate a
            // buffer.
            None
        } else {
            Some(self.shared.init_buffer(id, n))
        };
        ReadBuf {
            shared: self.shared.clone(),
            owned,
        }
    }

    /// Converts the queue into a raw pointer, used by the [`DropWake`]
    /// implementation.
    pub(crate) unsafe fn into_raw(self) -> *const () {
        Arc::into_raw(self.shared).cast()
    }

    /// Converts the queue into a raw pointer, used by the [`DropWake`]
    /// implementation.
    pub(crate) unsafe fn from_raw(ptr: *const ()) -> ReadBufPool {
        ReadBufPool {
            shared: Arc::from_raw(ptr.cast_mut().cast()),
        }
    }
}

/// Buffer reference from a [`ReadBufPool`].
///
/// Before a read system call, this will be empty and can't be resized. This is
/// really only useful in a call to a `read(2)` like system call.
///
/// # Notes
///
/// Do **not** use the [`BufMut`] implementation of this buffer to write into
/// it, it's a specialised implementation that is invalid use to outside of the
/// A10 crate.
pub struct ReadBuf {
    /// Buffer pool info.
    shared: Arc<sys::io::ReadBufPool>,
    /// This is `Some` if the buffer was assigned.
    owned: Option<NonNull<[u8]>>,
}

impl ReadBuf {
    /// Returns the capacity of the buffer.
    pub fn capacity(&self) -> usize {
        self.shared.buf_size()
    }

    /// Returns the length of the buffer.
    pub fn len(&self) -> usize {
        self.owned.map_or(0, NonNull::len)
    }

    /// Returns true if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.owned.is_none_or(|ptr| ptr.len() == 0)
    }

    /// Returns itself as slice.
    pub fn as_slice(&self) -> &[u8] {
        self
    }

    /// Returns itself as mutable slice.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self
    }

    /// Truncate the buffer to `len` bytes.
    ///
    /// If the buffer is shorter then `len` bytes this does nothing.
    pub fn truncate(&mut self, len: usize) {
        if let Some(ptr) = self.owned {
            if len > ptr.len() {
                return;
            }
            self.owned = Some(change_size(ptr, len));
        }
    }

    /// Clear the buffer.
    ///
    /// # Notes
    ///
    /// This is not the same as returning the buffer to the buffer pool, for
    /// that use [`ReadBuf::release`].
    pub fn clear(&mut self) {
        if let Some(ptr) = self.owned {
            self.owned = Some(change_size(ptr, 0));
        }
    }

    /// Remove the bytes in `range` from the buffer.
    ///
    /// # Panics
    ///
    /// This will panic if the `range` is invalid.
    pub fn remove<R>(&mut self, range: R)
    where
        R: RangeBounds<usize>,
    {
        let original_len = self.len();
        let start = match range.start_bound() {
            Bound::Unbounded => 0,
            Bound::Included(start_idx) => *start_idx,
            Bound::Excluded(start_idx) => start_idx + 1,
        };
        let end = match range.end_bound() {
            Bound::Unbounded => original_len,
            Bound::Included(end_idx) => end_idx + 1,
            Bound::Excluded(end_idx) => *end_idx,
        };

        if let Some(ptr) = self.owned {
            if start > end {
                panic!("slice index starts at {start} but ends at {end}");
            } else if end > original_len {
                panic!("range end index {end} out of range for slice of length {original_len}");
            }

            let remove_len = end - start;
            let new_len = original_len - remove_len;
            self.owned = Some(change_size(ptr, new_len));

            if new_len == 0 || start >= new_len {
                // No need to copy data round.
                return;
            }

            // We start copy where the remove range ends.
            let start_ptr = unsafe { ptr.as_ptr().cast::<u8>().add(end) };
            let to_copy = new_len - start;
            unsafe {
                ptr.as_ptr()
                    .cast::<u8>()
                    .add(start)
                    .copy_from(start_ptr, to_copy);
            }
        } else if start != 0 && end != 0 {
            panic!("attempting to remove range from empty buffer");
        }
    }

    /// Set the length of the buffer to `new_len`.
    ///
    /// # Safety
    ///
    /// The caller must ensure `new_len` bytes are initialised and that
    /// `new_len` is not larger than the buffer's capacity.
    pub unsafe fn set_len(&mut self, new_len: usize) {
        debug_assert!(new_len <= self.capacity());
        if let Some(ptr) = self.owned {
            self.owned = Some(change_size(ptr, new_len));
        }
    }

    /// Appends `other` to `self`.
    ///
    /// If `self` doesn't have sufficient capacity it will return `Err(())` and
    /// will not append anything.
    #[allow(clippy::result_unit_err)]
    pub fn extend_from_slice(&mut self, other: &[u8]) -> Result<(), ()> {
        if let Some(ptr) = self.owned {
            let new_len = ptr.len() + other.len();
            if new_len > self.capacity() {
                return Err(());
            }

            // SAFETY: the source, destination and len are all valid.
            // NOTE: we can't use `copy_from_nonoverlapping` as we can't
            // guarantee that `self` and `other` are not overlapping.
            unsafe {
                ptr.as_ptr()
                    .cast::<u8>()
                    .add(ptr.len())
                    .copy_from(other.as_ptr(), other.len());
            }
            self.owned = Some(change_size(ptr, new_len));
            Ok(())
        } else {
            Err(())
        }
    }

    /// Returns the remaining spare capacity of the buffer.
    #[allow(clippy::needless_pass_by_ref_mut)] // See https://github.com/rust-lang/rust-clippy/issues/12905.
    pub fn spare_capacity_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        if let Some(ptr) = self.owned {
            let unused_len = self.capacity() - ptr.len();
            // SAFETY: this won't overflow `isize`.
            let data = unsafe { ptr.as_ptr().cast::<u8>().add(ptr.len()) };
            // SAFETY: the pointer and length are correct.
            unsafe { slice::from_raw_parts_mut(data.cast(), unused_len) }
        } else {
            &mut []
        }
    }

    /// Release the buffer back to the buffer pool.
    ///
    /// If `self` isn't an allocated buffer this does nothing.
    ///
    /// The buffer can still be used in a `read(2)` system call, it's reset to
    /// the state as if it was just created by calling [`ReadBufPool::get`].
    ///
    /// # Notes
    ///
    /// This is automatically called in the `Drop` implementation.
    pub fn release(&mut self) {
        if let Some(ptr) = self.owned.take() {
            // SAFETY: this is safe because we're taking the address ensure we
            // can't call this method again.
            unsafe { self.shared.release(ptr) }
        }
    }
}

/// Changes the size of `slice` to `new_len`.
const fn change_size<T>(slice: NonNull<[T]>, new_len: usize) -> NonNull<[T]> {
    // SAFETY: `ptr` is `NonNull`, thus not NULL.
    let ptr = unsafe { NonNull::new_unchecked(slice.as_ptr().cast()) };
    NonNull::slice_from_raw_parts(ptr, new_len)
}

/// The implementation for `ReadBuf` is a special one as we don't actually pass
/// a "real" buffer. Instead we pass special flags to the kernel that allows it
/// to select a buffer from the connected [`ReadBufPool`] once the actual read
/// operation starts.
///
/// If the `ReadBuf` is used a second time in a read call this changes as at
/// that point it owns an actual buffer. At that point it will behave more like
/// the `Vec<u8>` implementation is that it only uses the unused capacity, so
/// any bytes already in the buffer will be untouched.
///
/// To revert to the original behaviour of allowing the kernel to select a
/// buffer call [`ReadBuf::release`] first.
///
/// Note that this can **not** be used in vectored I/O as a part of the
/// [`ButMutSlice`] trait.
///
/// [`ButMutSlice`]: crate::io::BufMutSlice
unsafe impl BufMut for ReadBuf {
    unsafe fn parts_mut(&mut self) -> (*mut u8, u32) {
        if let Some(ptr) = self.owned {
            let len = (self.capacity() - ptr.len()) as u32;
            (ptr.as_ptr().cast::<u8>().add(ptr.len()), len)
        } else {
            (ptr::null_mut(), self.capacity() as u32)
        }
    }

    unsafe fn set_init(&mut self, _: usize) {
        panic!("Don't call a10::Buf::set_init");
    }

    fn buffer_group(&self) -> Option<BufGroupId> {
        if self.owned.is_none() {
            Some(self.shared.group_id())
        } else {
            // Already have an allocated buffer, don't need another one.
            None
        }
    }

    unsafe fn buffer_init(&mut self, id: BufId, n: u32) {
        if let Some(ptr) = self.owned {
            // We shouldn't be assigned another buffer, we should be resizing
            // the current one.
            debug_assert!(id.0 == 0);
            self.owned = Some(change_size(ptr, ptr.len() + n as usize));
        } else {
            self.owned = Some(self.shared.init_buffer(id, n));
        }
    }
}

// SAFETY: `ReadBuf` manages the allocation of the bytes once it's assigned a
// buffer, so as long as it's alive, so is the slice of bytes.
unsafe impl Buf for ReadBuf {
    unsafe fn parts(&self) -> (*const u8, u32) {
        let slice = self.as_slice();
        (slice.as_ptr().cast(), slice.len() as u32)
    }
}

impl Deref for ReadBuf {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.owned.map_or(&[], |ptr| unsafe { ptr.as_ref() })
    }
}

impl DerefMut for ReadBuf {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.owned
            .map_or(&mut [], |mut ptr| unsafe { ptr.as_mut() })
    }
}

impl AsRef<[u8]> for ReadBuf {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl AsMut<[u8]> for ReadBuf {
    fn as_mut(&mut self) -> &mut [u8] {
        self
    }
}

impl Borrow<[u8]> for ReadBuf {
    fn borrow(&self) -> &[u8] {
        self
    }
}

impl BorrowMut<[u8]> for ReadBuf {
    fn borrow_mut(&mut self) -> &mut [u8] {
        self
    }
}

impl fmt::Debug for ReadBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_slice().fmt(f)
    }
}

unsafe impl Sync for ReadBuf {}
unsafe impl Send for ReadBuf {}

impl Drop for ReadBuf {
    fn drop(&mut self) {
        self.release();
    }
}

#[test]
fn size_assertion() {
    assert_eq!(std::mem::size_of::<ReadBufPool>(), 8);
    assert_eq!(std::mem::size_of::<ReadBuf>(), 24);
}

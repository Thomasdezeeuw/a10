use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Variable sized atomic bitmap.
#[repr(transparent)]
pub(crate) struct AtomicBitMap {
    data: [AtomicUsize],
}

impl AtomicBitMap {
    /// Create a new `AtomicBitMap`.
    pub(crate) fn new(entries: usize) -> Box<AtomicBitMap> {
        let mut size = entries / usize::BITS as usize;
        if (entries % usize::BITS as usize) != 0 {
            size += 1;
        }
        let mut vec = Vec::with_capacity(size);
        vec.resize_with(size, || AtomicUsize::new(0));
        // SAFETY: Due to the use of `repr(transparent)` on `AtomicBitMap` it
        // has the same layout as `[AtomicUsize]`.
        unsafe { Box::from_raw(Box::into_raw(vec.into_boxed_slice()) as _) }
    }

    /// Returns the number of indices the bitmap can manage.
    pub(crate) fn capacity(&self) -> usize {
        self.data.len() * usize::BITS as usize
    }

    /// Returns the index of the available slot, or `None`.
    pub(crate) fn next_unset(&self) -> Option<Index> {
        for idx in 0..self.data.len() {
            let mut value = self.data[idx].load(Ordering::Relaxed);
            if value == usize::MAX {
                continue; // All taken.
            }

            for i in 0..usize::BITS as usize {
                if is_unset(value, i) {
                    value = self.data[idx].fetch_or(1 << i, Ordering::SeqCst);
                    // Another thread could have attempted to set the same bit
                    // we're setting, so we need to make sure we actually set
                    // the bit (i.e. if was unset in the previous state).
                    if is_unset(value, i) {
                        return Some(Index((idx * usize::BITS as usize) + i));
                    }
                }
            }
        }
        None
    }

    /// Mark `index` as available.
    pub(crate) fn unset(&self, index: Index) {
        let idx = index.0 / usize::BITS as usize;
        let n = index.0 % usize::BITS as usize;
        let old_value = self.data[idx].fetch_and(!(1 << n), Ordering::SeqCst);
        debug_assert!(!is_unset(old_value, n));
    }
}

/// `n` is zero indexed.
const fn is_unset(value: usize, n: usize) -> bool {
    ((value >> n) & 1) == 0
}

impl fmt::Debug for AtomicBitMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AtomicBitMap").finish()
    }
}

/// Index into an array.
///
/// Contents made private so it must always come from [`AtomicBitMap`].
pub(crate) struct Index(usize);

impl Index {
    /// Get the value of index.
    pub(crate) const fn get(&self) -> usize {
        self.0
    }
}

#[test]
fn setting_and_unsetting_one() {
    setting_and_unsetting(64)
}

#[test]
fn setting_and_unsetting_two() {
    setting_and_unsetting(128)
}

#[test]
fn setting_and_unsetting_three() {
    setting_and_unsetting(192)
}

#[test]
fn setting_and_unsetting_four() {
    setting_and_unsetting(256)
}

#[test]
fn setting_and_unsetting_eight() {
    setting_and_unsetting(512)
}

#[test]
fn setting_and_unsetting_sixteen() {
    setting_and_unsetting(1024)
}

#[cfg(test)]
fn setting_and_unsetting(entries: usize) {
    let map = AtomicBitMap::new(entries);

    // Ask for all indices.
    for n in 0..entries {
        assert!(matches!(map.next_unset(), Some(Index(i)) if i == n));
    }
    // All bits should be set.
    for data in &map.data {
        assert!(data.load(Ordering::Relaxed) == usize::MAX);
    }
    // No more indices left.
    assert!(matches!(map.next_unset(), None));

    // Test unsetting an index not in order.
    map.unset(Index(63));
    map.unset(Index(0));
    assert!(matches!(map.next_unset(), Some(Index(i)) if i == 0));
    assert!(matches!(map.next_unset(), Some(Index(i)) if i == 63));

    // Unset all indices again.
    for n in (0..entries).into_iter().rev() {
        map.unset(Index(n));
    }
    // Bitmap should be zeroed.
    for data in &map.data {
        assert!(data.load(Ordering::Relaxed) == 0);
    }
    // Next avaiable index should be 0 again.
    assert!(matches!(map.next_unset(), Some(Index(i)) if i == 0));
}

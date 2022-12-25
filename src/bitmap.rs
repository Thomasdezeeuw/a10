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
    pub(crate) const fn capacity(&self) -> usize {
        self.data.len() * usize::BITS as usize
    }

    /// Returns the index of the available slot, or `None`.
    pub(crate) fn next_available(&self) -> Option<usize> {
        for (idx, data) in self.data.iter().enumerate() {
            let mut value = data.load(Ordering::Relaxed);
            let mut i = value.trailing_ones();
            while i < usize::BITS {
                // Attempt to set the bit, claiming the slot.
                value = data.fetch_or(1 << i, Ordering::SeqCst);
                // Another thread could have attempted to set the same bit we're
                // setting, so we need to make sure we actually set the bit
                // (i.e. check if was unset in the previous state).
                if is_unset(value, i as usize) {
                    return Some((idx * usize::BITS as usize) + i as usize);
                }
                i += (value >> i).trailing_ones();
            }
        }
        None
    }

    /// Mark `index` as available.
    pub(crate) fn make_available(&self, index: usize) {
        let idx = index / usize::BITS as usize;
        let n = index % usize::BITS as usize;
        let old_value = self.data[idx].fetch_and(!(1 << n), Ordering::SeqCst);
        debug_assert!(!is_unset(old_value, n));
    }
}

/// Returns true if bit `n` is set in `value`. `n` is zero indexed, i.e. must be
/// in the range 0..usize::BITS (64).
const fn is_unset(value: usize, n: usize) -> bool {
    ((value >> n) & 1) == 0
}

impl fmt::Debug for AtomicBitMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const WIDTH: usize = usize::BITS as usize;
        for data in self.data.iter() {
            let value = data.load(Ordering::Relaxed);
            write!(f, "{value:0WIDTH$b}")?;
        }
        Ok(())
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
    assert_eq!(map.capacity(), entries);

    // Ask for all indices.
    for n in 0..entries {
        assert!(matches!(map.next_available(), Some(i) if i == n));
    }
    // All bits should be set.
    for data in &map.data {
        assert!(data.load(Ordering::Relaxed) == usize::MAX);
    }
    // No more indices left.
    assert!(matches!(map.next_available(), None));

    // Test unsetting an index not in order.
    map.make_available(63);
    map.make_available(0);
    assert!(matches!(map.next_available(), Some(i) if i == 0));
    assert!(matches!(map.next_available(), Some(i) if i == 63));

    // Unset all indices again.
    for n in (0..entries).into_iter().rev() {
        map.make_available(n);
    }
    // Bitmap should be zeroed.
    for data in &map.data {
        assert!(data.load(Ordering::Relaxed) == 0);
    }
    // Next avaiable index should be 0 again.
    assert!(matches!(map.next_available(), Some(i) if i == 0));
}

#[test]
fn setting_and_unsetting_concurrent() {
    use std::sync::{Arc, Barrier};
    use std::thread;

    const N: usize = 4;
    const M: usize = 1024;

    let bitmap = Arc::new(AtomicBitMap::new(N * M));
    let barrier = Arc::new(Barrier::new(N + 1));
    let handles = (0..N)
        .map(|i| {
            let bitmap = bitmap.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let mut indices = Vec::with_capacity(M);
                barrier.wait();

                if i % 2 == 0 {
                    for _ in 0..M {
                        let idx = bitmap.next_available().expect("failed to get index");
                        indices.push(idx);
                    }

                    for idx in indices {
                        bitmap.make_available(idx);
                    }
                } else {
                    for _ in 0..M {
                        let idx = bitmap.next_available().expect("failed to get index");
                        bitmap.make_available(idx);
                    }
                }
            })
        })
        .collect::<Vec<_>>();

    barrier.wait();
    handles
        .into_iter()
        .map(|handle| handle.join())
        .collect::<thread::Result<()>>()
        .unwrap();
}

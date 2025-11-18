use std::alloc::{Layout, alloc, dealloc};

use a10::mem::{self, AdviseFlag};

use crate::util::{Waker, defer, is_send, is_sync, test_queue};

#[test]
fn advise_is_send_and_sync() {
    is_send::<mem::Advise>();
    is_sync::<mem::Advise>();
}

#[test]
fn madvise() {
    let sq = test_queue();
    let waker = Waker::new();

    // The address in `madvise(2)` needs to be page aligned.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
    let layout =
        Layout::from_size_align(2 * page_size, page_size).expect("failed to create alloc layout");
    let ptr = unsafe { alloc(layout) };
    let _d = defer(|| unsafe { dealloc(ptr, layout) });

    waker
        .block_on(mem::advise(
            sq,
            ptr.cast(),
            page_size as u32,
            AdviseFlag::WILL_NEED,
        ))
        .expect("failed madvise");
}

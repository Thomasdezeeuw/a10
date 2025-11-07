use crate::kqueue;

pub(crate) fn use_direct_flags(submission: &mut kqueue::Event) {
    unreachable!("use_direct_flags: kqueue doesn't have direct descriptors")
}

pub(crate) fn create_direct_flags(submission: &mut kqueue::Event) {
    unreachable!("create_direct_flags: kqueue doesn't have direct descriptors")
}

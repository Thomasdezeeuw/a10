use crate::process::Signal;

// kqueue doesn't give us a lot of info.
pub(crate) use crate::process::Signal as SignalInfo;

#[allow(clippy::trivially_copy_pass_by_ref)]
pub(crate) const fn signal(info: &SignalInfo) -> Signal {
    *info
}

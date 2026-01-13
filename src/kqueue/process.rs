use crate::process::Signal;

// kqueue doesn't give us a lot of info.
pub(crate) use crate::process::Signal as SignalInfo;

pub(crate) const fn signal(info: &SignalInfo) -> Signal {
    *info
}

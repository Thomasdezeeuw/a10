//! [`Config`]uration module.

use std::io;

use crate::Ring;

/// Configuration of a [`Ring`].
///
/// Created by calling [`Ring::config`].
#[derive(Debug, Clone)]
#[must_use = "no ring is created until `a10::Config::build` is called"]
pub struct Config<'r> {
    pub(crate) queued_operations: usize,
    /// Implementation specific configuration.
    pub(crate) sys: crate::sys::config::Config<'r>,
}

impl<'r> Config<'r> {
    /// Build a new [`Ring`].
    #[doc(alias = "kqueue")]
    #[doc(alias = "io_uring_setup")]
    pub fn build(self) -> io::Result<Ring> {
        // NOTE: defined in the implementation specific configuration code.
        let queued_operations = self.queued_operations; // TODO: add option for # queued operations.
        let (submissions, shared, completions) = self.build_sys()?;
        let ring = Ring::build(submissions, shared, completions, queued_operations);
        Ok(ring)
    }
}

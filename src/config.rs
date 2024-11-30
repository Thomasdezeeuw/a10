//! [`Config`]uration module.

use std::io;

use crate::Ring;

/// Configuration of a [`Ring`].
///
/// Created by calling [`Ring::config`].
#[derive(Debug, Clone)]
#[must_use = "no ring is created until `a10::Config::build` is called"]
pub struct Config<'r> {
    pub(crate) queued_operations: u32,
    /// Implementation specific configuration.
    pub(crate) sys: crate::sys::config::Config<'r>,
}

impl<'r> Config<'r> {
    pub(crate) const fn new(queued_operations: u32) -> Config<'r> {
        Config {
            queued_operations,
            sys: crate::sys::Config::new(),
        }
    }

    /// Build a new [`Ring`].
    #[doc(alias = "kqueue")]
    #[doc(alias = "io_uring_setup")]
    pub fn build(self) -> io::Result<Ring> {
        // NOTE: defined in the implementation specific configuration code.
        let queued_operations = self.queued_operations; // TODO: add option for # queued operations.
        let (submissions, shared, completions) = self._build()?;
        Ring::build(submissions, shared, completions, queued_operations as usize)
    }
}

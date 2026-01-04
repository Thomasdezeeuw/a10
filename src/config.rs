//! [`Config`]uration module.

use std::io;

use crate::Ring;

/// Configuration of a [`Ring`].
///
/// Created by calling [`Ring::config`].
#[derive(Debug, Clone)]
#[must_use = "no ring is created until `a10::Config::build` is called"]
pub struct Config<'r> {
    /// Implementation specific configuration.
    pub(crate) sys: crate::sys::config::Config<'r>,
}

impl<'r> Config<'r> {
    /// Build a new [`Ring`].
    #[doc(alias = "kqueue")]
    #[doc(alias = "io_uring_setup")]
    pub fn build(self) -> io::Result<Ring> {
        let (sq, cq) = self.build_sys()?;
        Ok(Ring { sq, cq })
    }
}

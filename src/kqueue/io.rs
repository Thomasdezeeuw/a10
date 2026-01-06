// Re-export so we don't have to worry about import `std::io` and `crate::io`.
pub(crate) use std::io::*;

pub(crate) use crate::unix::{IoMutSlice, IoSlice};

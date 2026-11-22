//! Core logic for `ll`: argument analysis and size-column post-processing.

pub mod args;
pub mod format;

pub use args::{analyze, build_args, Analysis, Flavor};
pub use format::{group_thousands, process, Layout};

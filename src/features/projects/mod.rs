//! Local project state and Git integration.

pub(crate) mod git_status;

pub(crate) use git_status::{GitStatus, inspect};

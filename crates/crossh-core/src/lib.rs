//! Crossh's UI-independent domain and local-system contracts.
//!
//! This crate deliberately has no GPUI dependency. It owns data that can be
//! tested and reused by both the application UI and background transports.

pub mod config;
pub mod connection;
pub mod format;
pub mod git;
pub mod git_branch;
pub mod git_conflict;
pub mod git_history;
pub mod git_history_graph;
pub mod git_remote;
pub mod git_stash;
pub mod git_status;
pub mod process;
pub mod system_stats;
pub mod terminal;

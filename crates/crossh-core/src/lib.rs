//! Crossh's UI-independent domain and local-system contracts.
//!
//! This crate deliberately has no GPUI dependency. It owns data that can be
//! tested and reused by both the application UI and background transports.

rust_i18n::i18n!("../../locales", fallback = "en");

pub mod config;
pub mod connection;
pub mod editor_launcher;
pub mod format;
pub mod git;
pub mod git_branch;
pub mod git_conflict;
pub mod git_history;
pub mod git_history_graph;
pub mod git_launcher;
pub mod git_remote;
pub mod git_stash;
pub mod git_status;
pub mod i18n;
pub mod input_handler;
pub mod locale;
pub mod note_launcher;
pub mod process;
pub mod single_instance;
pub mod system_stats;
pub mod terminal;
pub mod text_editing;

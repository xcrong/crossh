//! Crossh's UI-independent domain and local-system contracts.
//!
//! This crate deliberately has no GPUI dependency. It owns data that can be
//! tested and reused by both the application UI and background transports.

pub mod commands;
pub mod config;
pub mod connection;
pub mod git;
pub mod process;
pub mod project;
pub mod terminal;

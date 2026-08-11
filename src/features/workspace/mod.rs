//! Main workspace: navigation, tabs, local projects, and pane composition.

pub(crate) mod command_editor;
pub(crate) mod empty_state;
pub(crate) mod pane;
pub(crate) mod registry;
pub(crate) mod settings;
pub(crate) mod shell;
pub(crate) mod sidebar;
pub(crate) mod view;

pub(crate) use shell::{AppShell, open_main_window};

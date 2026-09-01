//! Main workspace: navigation, tabs, local projects, and pane composition.

pub(crate) mod compose_bar;
pub(crate) mod empty_state;
pub(crate) mod linux_titlebar;
pub(crate) mod local_paths;
pub(crate) mod modal_editor;
pub(crate) mod pane;
pub(crate) mod pinned;
pub(crate) mod registry;
pub(crate) mod scratch_bar;
pub(crate) mod settings;
pub(crate) mod shell;
pub(crate) mod sidebar;
pub(crate) mod state;
pub(crate) mod status;
pub(crate) mod system_monitor;
pub(crate) mod tab_strip;
pub(crate) mod toaster;
pub(crate) mod view;

pub(crate) use shell::{AppShell, open_main_window};

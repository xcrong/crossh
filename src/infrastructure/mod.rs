//! Platform and external-system integrations.

pub(crate) mod app_menu;
#[cfg(target_os = "macos")]
pub(crate) mod tray;

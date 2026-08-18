//! Application bootstrap and composition entry points.

use gpui::App;

pub(crate) use crate::features::workspace::open_main_window;

/// Launch targets are the product of CLI argument parsing and are reused by
/// the `on_reopen` callback to restore the same entry point on window reopen.
/// Only the main window exists today; future CLI entries (e.g. opening a
/// project or SSH session directly) extend this enum instead of threading
/// new arguments through `main`.
#[derive(Clone)]
pub(crate) enum LaunchTarget {
    Main,
}

/// Opens the window for the given launch target. Kept as an indirection
/// instead of calling `open_main_window` directly so that startup logic stays
/// centralized here and neither `main` nor the `on_reopen` closure needs to
/// know about concrete targets.
pub(crate) fn open_launch_target(target: LaunchTarget, cx: &mut App) {
    match target {
        LaunchTarget::Main => open_main_window(cx),
    }
}

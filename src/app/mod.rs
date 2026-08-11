//! Application bootstrap and composition entry points.

use std::path::PathBuf;

use gpui::App;

pub(crate) use crate::features::workspace::open_main_window;

#[derive(Clone)]
pub(crate) enum LaunchTarget {
    Main,
    Git(PathBuf),
}

pub(crate) fn open_launch_target(target: LaunchTarget, cx: &mut App) {
    match target {
        LaunchTarget::Main => open_main_window(cx),
        LaunchTarget::Git(cwd) => crate::features::git::open_git_window(cwd, cx),
    }
}

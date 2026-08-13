//! Git 可视化功能：独立的 Git 窗口（VS Code 源码管理风格）。

use gpui::{App, KeyBinding, actions};

mod cli;

actions!(
    git_window,
    [
        MoveSelectionUp,
        MoveSelectionDown,
        ToggleSelectedStage,
        CommitChanges,
        RefreshChanges,
        BackToChanges
    ]
);

const GIT_WINDOW_CONTEXT: &str = "GitWindow";
const GIT_CHANGES_CONTEXT: &str = "GitChanges";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", MoveSelectionUp, Some(GIT_CHANGES_CONTEXT)),
        KeyBinding::new("down", MoveSelectionDown, Some(GIT_CHANGES_CONTEXT)),
        KeyBinding::new("space", ToggleSelectedStage, Some(GIT_CHANGES_CONTEXT)),
        KeyBinding::new("cmd-enter", CommitChanges, Some(GIT_WINDOW_CONTEXT)),
        KeyBinding::new("cmd-r", RefreshChanges, Some(GIT_WINDOW_CONTEXT)),
        KeyBinding::new("escape", BackToChanges, Some(GIT_WINDOW_CONTEXT)),
    ]);
}

mod editor;
mod input;
mod model;
mod render;
mod window;

#[allow(unused_imports)]
pub(crate) use cli::{
    GitCliCommand, parse as parse_cli, print_help as print_cli_help,
    print_standalone_help as print_standalone_cli_help, spawn_git_process,
};
pub(crate) use window::open_git_window;

#[cfg(feature = "visual-tests")]
#[allow(dead_code)]
pub(crate) fn visual_fixture(
    cwd: std::path::PathBuf,
    show_compact_diff: bool,
    show_error: bool,
    cx: &mut App,
) -> gpui::Entity<window::GitWindow> {
    window::GitWindow::visual_fixture(cwd, show_compact_diff, show_error, cx)
}

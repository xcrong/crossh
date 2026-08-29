//! Git Viewer UI：由独立的 `crossh-git` 进程承载。

use gpui::{App, KeyBinding, actions};

actions!(
    git_window,
    [
        MoveSelectionUp,
        MoveSelectionDown,
        ToggleSelectedStage,
        CommitChanges,
        RefreshChanges,
        BackToChanges,
        SelectAllChanges,
        MoveHistoryUp,
        MoveHistoryDown,
        MoveBranchUp,
        MoveBranchDown,
        SwitchSelectedBranch,
        MoveStashUp,
        MoveStashDown,
        ApplySelectedStash
    ]
);

const GIT_WINDOW_CONTEXT: &str = "GitWindow";
const GIT_CHANGES_CONTEXT: &str = "GitChanges";
const GIT_HISTORY_CONTEXT: &str = "GitHistory";
const GIT_BRANCH_CONTEXT: &str = "GitBranch";
const GIT_STASH_CONTEXT: &str = "GitStash";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", MoveSelectionUp, Some(GIT_CHANGES_CONTEXT)),
        KeyBinding::new("down", MoveSelectionDown, Some(GIT_CHANGES_CONTEXT)),
        KeyBinding::new("space", ToggleSelectedStage, Some(GIT_CHANGES_CONTEXT)),
        KeyBinding::new("cmd-enter", CommitChanges, Some(GIT_WINDOW_CONTEXT)),
        KeyBinding::new("cmd-r", RefreshChanges, Some(GIT_WINDOW_CONTEXT)),
        KeyBinding::new("cmd-a", SelectAllChanges, Some(GIT_CHANGES_CONTEXT)),
        KeyBinding::new("up", MoveHistoryUp, Some(GIT_HISTORY_CONTEXT)),
        KeyBinding::new("down", MoveHistoryDown, Some(GIT_HISTORY_CONTEXT)),
        KeyBinding::new("up", MoveBranchUp, Some(GIT_BRANCH_CONTEXT)),
        KeyBinding::new("down", MoveBranchDown, Some(GIT_BRANCH_CONTEXT)),
        KeyBinding::new("enter", SwitchSelectedBranch, Some(GIT_BRANCH_CONTEXT)),
        KeyBinding::new("up", MoveStashUp, Some(GIT_STASH_CONTEXT)),
        KeyBinding::new("down", MoveStashDown, Some(GIT_STASH_CONTEXT)),
        KeyBinding::new("enter", ApplySelectedStash, Some(GIT_STASH_CONTEXT)),
        KeyBinding::new("escape", BackToChanges, Some(GIT_WINDOW_CONTEXT)),
    ]);
}

mod branch;
mod branch_render;
mod context_menu;
mod editor;
mod history;
mod history_render;
mod input;
mod model;
mod render;
mod session;
mod stash;
mod stash_render;
mod window;

pub(crate) use window::open_git_window;

#[cfg(feature = "visual-tests")]
pub(crate) fn visual_fixture(
    cwd: std::path::PathBuf,
    show_compact_diff: bool,
    show_error: bool,
    cx: &mut App,
) -> gpui::Entity<window::GitWindow> {
    window::GitWindow::visual_fixture(cwd, show_compact_diff, show_error, cx)
}

#[cfg(feature = "visual-tests")]
pub(crate) fn visual_history_fixture(
    cwd: std::path::PathBuf,
    show_detail: bool,
    cx: &mut App,
) -> gpui::Entity<window::GitWindow> {
    window::GitWindow::visual_history_fixture(cwd, show_detail, cx)
}

#[cfg(feature = "visual-tests")]
pub(crate) fn visual_branch_fixture(
    cwd: std::path::PathBuf,
    cx: &mut App,
) -> gpui::Entity<window::GitWindow> {
    window::GitWindow::visual_branch_fixture(cwd, cx)
}

#[cfg(feature = "visual-tests")]
pub(crate) fn visual_stash_fixture(
    cwd: std::path::PathBuf,
    cx: &mut App,
) -> gpui::Entity<window::GitWindow> {
    window::GitWindow::visual_stash_fixture(cwd, cx)
}

#[cfg(feature = "visual-tests")]
pub(crate) fn visual_conflict_fixture(
    cwd: std::path::PathBuf,
    cx: &mut App,
) -> gpui::Entity<window::GitWindow> {
    window::GitWindow::visual_conflict_fixture(cwd, cx)
}

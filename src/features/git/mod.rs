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
        ApplySelectedStash,
        MoveRemoteUp,
        MoveRemoteDown,
        FetchSelectedRemote
    ]
);

const GIT_WINDOW_CONTEXT: &str = "GitWindow";
const GIT_CHANGES_CONTEXT: &str = "GitChanges";
const GIT_HISTORY_CONTEXT: &str = "GitHistory";
const GIT_BRANCH_CONTEXT: &str = "GitBranch";
const GIT_STASH_CONTEXT: &str = "GitStash";
const GIT_REMOTE_CONTEXT: &str = "GitRemote";

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
        KeyBinding::new("up", MoveRemoteUp, Some(GIT_REMOTE_CONTEXT)),
        KeyBinding::new("down", MoveRemoteDown, Some(GIT_REMOTE_CONTEXT)),
        KeyBinding::new("enter", FetchSelectedRemote, Some(GIT_REMOTE_CONTEXT)),
        KeyBinding::new("escape", BackToChanges, Some(GIT_WINDOW_CONTEXT)),
    ]);
}

mod branch;
mod branch_render;
mod history;
mod history_render;
mod input;
mod remote;
mod remote_render;
mod render;
mod session;
mod stash;
mod stash_render;
mod window;

pub(crate) use window::open_git_window;

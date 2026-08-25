//! Git 窗口状态、后台任务与窗口生命周期。

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, AppContext, Bounds, Context, FocusHandle, Pixels, Point, ScrollHandle, Size, Task,
    TitlebarOptions, UniformListScrollHandle, WindowBounds, WindowOptions, px,
};

use crate::shared::text_editing::TextEditingState;
use crossh_core::git::{
    commit, diff, discard_worktree, pull, push, scan_changes, stage, stage_hunk, unstage,
    unstage_hunk,
};
use crossh_core::git_branch::{list_branches, switch_branch as checkout_branch};
use crossh_core::git_conflict::{ConflictResolution, resolve_conflict};
#[cfg(feature = "visual-tests")]
use crossh_core::git_history::{CommitDetail, CommitFileChange, CommitSummary};
use crossh_core::git_history::{list_history, show_commit};
use crossh_core::git_stash::{
    apply_stash as apply_git_stash, drop_stash as drop_git_stash, list_stashes,
    pop_stash as pop_git_stash, push_stash as push_git_stash,
};
use crossh_core::terminal::path_display_name;
use crossh_ui_component::context_menu::ContextMenuState;

use super::context_menu::{self, GitMenuAction};
use super::editor::CommitEditor;
#[cfg(feature = "visual-tests")]
use super::history::{HistoryDetailState, HistoryListState};
use super::model::{CHANGES_PANE_DEFAULT_WIDTH, CompactPage};
use super::session::{ChangeKey, GitOperation, GitSession, OperationState, selected_index};

const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

pub struct GitWindow {
    pub(super) session: GitSession,
    pub(super) compact_layout: bool,
    pub(super) compact_page: CompactPage,
    pub(super) staged_collapsed: bool,
    pub(super) working_collapsed: bool,
    pub(super) commit_editor: CommitEditor,
    pub(super) changes_focus: FocusHandle,
    pub(super) history_focus: FocusHandle,
    pub(super) history_search_focus: FocusHandle,
    pub(super) branch_focus: FocusHandle,
    pub(super) stash_focus: FocusHandle,
    /// 刷新循环已启动标记。仅用于防重入（is_some 检查），从不置回 None：
    /// 持有 Task 句柄使循环随实体 drop 自动取消。
    pub(super) refresh_task: Option<Task<()>>,
    pub(super) changes_scroll: UniformListScrollHandle,
    pub(super) diff_scroll: UniformListScrollHandle,
    pub(super) history_scroll: UniformListScrollHandle,
    pub(super) history_detail_scroll: UniformListScrollHandle,
    pub(super) history_message_scroll: ScrollHandle,
    pub(super) branch_scroll: UniformListScrollHandle,
    pub(super) stash_scroll: UniformListScrollHandle,
    pub(super) changes_pane_width: Rc<Cell<f32>>,
    pub(super) changes_pane_dragging: Rc<Cell<bool>>,
    pub(super) context_menu: Option<ContextMenuState<GitMenuAction>>,
    pub(super) pending_discard: Option<Vec<String>>,
    pub(super) pending_stash_drop: Option<String>,
    pub(super) history_query: TextEditingState,
}

impl GitWindow {
    pub(crate) fn new(cwd: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut git_window = Self {
            session: GitSession::new(cwd),
            compact_layout: false,
            compact_page: CompactPage::Changes,
            staged_collapsed: false,
            working_collapsed: false,
            commit_editor: CommitEditor::new(cx.focus_handle()),
            changes_focus: cx.focus_handle(),
            history_focus: cx.focus_handle(),
            history_search_focus: cx.focus_handle(),
            branch_focus: cx.focus_handle(),
            stash_focus: cx.focus_handle(),
            refresh_task: None,
            changes_scroll: UniformListScrollHandle::new(),
            diff_scroll: UniformListScrollHandle::new(),
            history_scroll: UniformListScrollHandle::new(),
            history_detail_scroll: UniformListScrollHandle::new(),
            history_message_scroll: ScrollHandle::new(),
            branch_scroll: UniformListScrollHandle::new(),
            stash_scroll: UniformListScrollHandle::new(),
            changes_pane_width: Rc::new(Cell::new(CHANGES_PANE_DEFAULT_WIDTH)),
            changes_pane_dragging: Rc::new(Cell::new(false)),
            context_menu: None,
            pending_discard: None,
            pending_stash_drop: None,
            history_query: TextEditingState::new(String::new()),
        };
        git_window.refresh_list(cx);
        git_window.ensure_refresh_loop(cx);
        git_window
    }

    #[cfg(feature = "visual-tests")]
    pub(crate) fn visual_fixture(
        cwd: PathBuf,
        show_compact_diff: bool,
        show_error: bool,
        cx: &mut App,
    ) -> gpui::Entity<Self> {
        cx.new(|cx| {
            let mut git_window = Self::new(cwd, cx);
            git_window.commit_editor.state.value = "完善 Git 窗口\n保持紧凑布局可用".to_string();
            git_window.commit_editor.state.cursor = git_window.commit_editor.state.value.len();
            if show_compact_diff {
                git_window.compact_page = CompactPage::Diff;
            }
            if show_error {
                git_window.session.operation = OperationState::Error(
                    "提交失败：请先配置 Git 用户名和邮箱，然后重试。".to_string(),
                );
            }
            git_window
        })
    }

    #[cfg(feature = "visual-tests")]
    pub(crate) fn visual_history_fixture(
        cwd: PathBuf,
        show_detail: bool,
        cx: &mut App,
    ) -> gpui::Entity<Self> {
        cx.new(|cx| {
            let mut git_window = Self::new(cwd, cx);
            if show_detail {
                let id = "1111111111111111111111111111111111111111".to_string();
                let summary = CommitSummary {
                    id: id.clone(),
                    short_id: id[..7].to_string(),
                    author: "Crossh Visual".to_string(),
                    date: "2026-08-16T12:00:00+08:00".to_string(),
                    subject: "Refine Git workbench history".to_string(),
                    parents: Vec::new(),
                };
                git_window.session.history.entries = vec![summary.clone()];
                git_window.session.history.selected = Some(id);
                git_window.session.history.list_state = HistoryListState::Ready;
                git_window.session.history.detail = HistoryDetailState::Ready(CommitDetail {
                    summary,
                    body: "Separate Git history from the workspace and keep commit detail easy to scan.".to_string(),
                    files: vec![
                        CommitFileChange {
                            path: "src/features/git/history.rs".to_string(),
                            old_path: None,
                            insertions: 86,
                            deletions: 12,
                            binary: false,
                        },
                        CommitFileChange {
                            path: "docs/architecture.md".to_string(),
                            old_path: Some("docs/git.md".to_string()),
                            insertions: 4,
                            deletions: 2,
                            binary: false,
                        },
                    ],
                });
                git_window.compact_page = CompactPage::HistoryDetail;
            } else {
                git_window.show_history(cx);
            }
            git_window
        })
    }

    #[cfg(feature = "visual-tests")]
    pub(crate) fn visual_branch_fixture(cwd: PathBuf, cx: &mut App) -> gpui::Entity<Self> {
        cx.new(|cx| {
            let mut git_window = Self::new(cwd, cx);
            git_window.show_branches(cx);
            git_window
        })
    }

    #[cfg(feature = "visual-tests")]
    pub(crate) fn visual_stash_fixture(cwd: PathBuf, cx: &mut App) -> gpui::Entity<Self> {
        cx.new(|cx| {
            let mut git_window = Self::new(cwd, cx);
            git_window.show_stashes(cx);
            git_window
        })
    }

    #[cfg(feature = "visual-tests")]
    pub(crate) fn visual_conflict_fixture(cwd: PathBuf, cx: &mut App) -> gpui::Entity<Self> {
        cx.new(|cx| {
            let mut git_window = Self::new(cwd, cx);
            git_window.compact_page = CompactPage::Diff;
            git_window
        })
    }

    pub(super) fn refresh_list(&mut self, cx: &mut Context<Self>) {
        self.refresh_list_with_diff_reload(false, cx);
    }

    pub(super) fn force_refresh_list(&mut self, cx: &mut Context<Self>) {
        self.refresh_list_with_diff_reload(true, cx);
    }

    pub(super) fn refresh_current_page(&mut self, cx: &mut Context<Self>) {
        if self.is_history_page() {
            self.refresh_history(true, cx);
        } else if self.is_branch_page() {
            self.refresh_branches(true, cx);
        } else if self.is_stash_page() {
            self.refresh_stashes(true, cx);
        } else {
            self.force_refresh_list(cx);
        }
    }

    pub(super) fn show_history(&mut self, cx: &mut Context<Self>) {
        self.compact_page = CompactPage::History;
        if self.session.history.entries.is_empty() && !self.session.history.list_state.is_loading()
        {
            self.refresh_history(false, cx);
        }
        cx.notify();
    }

    pub(super) fn show_branches(&mut self, cx: &mut Context<Self>) {
        self.compact_page = CompactPage::Branches;
        if self.session.branch.entries.is_empty() && !self.session.branch.list_state.is_loading() {
            self.refresh_branches(false, cx);
        }
        cx.notify();
    }

    pub(super) fn show_stashes(&mut self, cx: &mut Context<Self>) {
        self.compact_page = CompactPage::Stashes;
        if self.session.stash.entries.is_empty() && !self.session.stash.list_state.is_loading() {
            self.refresh_stashes(false, cx);
        }
        cx.notify();
    }

    pub(super) fn show_changes(&mut self, cx: &mut Context<Self>) {
        self.compact_page = CompactPage::Changes;
        cx.notify();
    }

    pub(super) fn is_history_page(&self) -> bool {
        matches!(
            self.compact_page,
            CompactPage::History | CompactPage::HistoryDetail
        )
    }

    pub(super) fn is_branch_page(&self) -> bool {
        matches!(self.compact_page, CompactPage::Branches)
    }

    pub(super) fn is_stash_page(&self) -> bool {
        matches!(self.compact_page, CompactPage::Stashes)
    }

    pub(super) fn refresh_history(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(request) = self
            .session
            .history
            .begin_list(self.session.cwd.clone(), force)
        else {
            return;
        };
        let cwd = request.cwd.clone();
        let limit = request.limit;
        cx.spawn(async move |weak, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { list_history(&cwd, limit) })
                .await;
            let _ = weak.update(cx, |this, cx| {
                if !this.session.history.apply_list(request, result) {
                    return;
                }
                if this.session.history.list_state.is_ready()
                    && this.session.history.selected.is_some()
                    && this.session.history.detail.selected_id()
                        != this.session.history.selected.as_deref()
                {
                    this.refresh_history_detail(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn refresh_history_detail(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.session.history.begin_detail(self.session.cwd.clone()) else {
            return;
        };
        let cwd = request.cwd.clone();
        let id = request.id.clone();
        cx.spawn(async move |weak, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { show_commit(&cwd, &id) })
                .await;
            let _ = weak.update(cx, |this, cx| {
                if this.session.history.apply_detail(request, result) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn refresh_branches(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(request) = self
            .session
            .branch
            .begin_list(self.session.cwd.clone(), force)
        else {
            return;
        };
        let cwd = request.cwd.clone();
        cx.spawn(async move |weak, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { list_branches(&cwd) })
                .await;
            let _ = weak.update(cx, |this, cx| {
                if this.session.branch.apply_list(request, result) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn refresh_stashes(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(request) = self
            .session
            .stash
            .begin_list(self.session.cwd.clone(), force)
        else {
            return;
        };
        let cwd = request.cwd.clone();
        cx.spawn(async move |weak, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { list_stashes(&cwd) })
                .await;
            let _ = weak.update(cx, |this, cx| {
                if this.session.stash.apply_list(request, result) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn select_history_commit(&mut self, id: String, cx: &mut Context<Self>) {
        let changed = self.session.history.select(id.clone());
        if !changed {
            if self.compact_layout && self.session.history.selected.as_deref() == Some(id.as_str())
            {
                self.compact_page = CompactPage::HistoryDetail;
                cx.notify();
            }
            return;
        }
        if self.compact_layout {
            self.compact_page = CompactPage::HistoryDetail;
            self.history_detail_scroll
                .scroll_to_item_strict(0, gpui::ScrollStrategy::Top);
        }
        self.refresh_history_detail(cx);
        cx.notify();
    }

    pub(super) fn move_history_selection(&mut self, direction: i8, cx: &mut Context<Self>) {
        let entries = self.session.history.visible_rows();
        if entries.is_empty() {
            return;
        }
        let current = self
            .session
            .history
            .selected
            .as_ref()
            .and_then(|id| entries.iter().position(|entry| &entry.entry.id == id))
            .unwrap_or(0);
        let next = if direction < 0 {
            current.saturating_sub(1)
        } else {
            (current + 1).min(entries.len() - 1)
        };
        self.select_history_commit(entries[next].entry.id.clone(), cx);
    }

    pub(super) fn set_history_query(&mut self, query: String, cx: &mut Context<Self>) {
        if !self.session.history.set_query(query) {
            return;
        }
        if self.session.history.selected.is_some()
            && self.session.history.detail.selected_id() != self.session.history.selected.as_deref()
        {
            self.refresh_history_detail(cx);
        }
        cx.notify();
    }

    pub(super) fn select_branch(&mut self, name: String, cx: &mut Context<Self>) {
        if self.session.branch.select(name) {
            cx.notify();
        }
    }

    pub(super) fn move_branch_selection(&mut self, direction: i8, cx: &mut Context<Self>) {
        if self.session.branch.entries.is_empty() {
            return;
        }
        let current = self
            .session
            .branch
            .selected
            .as_ref()
            .and_then(|name| {
                self.session
                    .branch
                    .entries
                    .iter()
                    .position(|entry| &entry.name == name)
            })
            .unwrap_or(0);
        let next = if direction < 0 {
            current.saturating_sub(1)
        } else {
            (current + 1).min(self.session.branch.entries.len() - 1)
        };
        self.select_branch(self.session.branch.entries[next].name.clone(), cx);
    }

    pub(super) fn switch_branch(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(branch) = self
            .session
            .branch
            .entries
            .iter()
            .find(|entry| entry.name == name)
        else {
            return;
        };
        if branch.current || matches!(self.session.operation, OperationState::Running) {
            return;
        }
        self.session.branch.select(name.clone());
        self.run_operation(
            GitOperation::SwitchBranch(name),
            self.session.selected.clone(),
            false,
            cx,
        );
    }

    pub(super) fn switch_selected_branch(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self
            .session
            .branch
            .selected_branch()
            .map(|branch| branch.name.clone())
        else {
            return;
        };
        self.switch_branch(name, cx);
    }

    pub(super) fn select_stash(&mut self, selector: String, cx: &mut Context<Self>) {
        if self.session.stash.select(selector) {
            cx.notify();
        }
    }

    pub(super) fn move_stash_selection(&mut self, direction: i8, cx: &mut Context<Self>) {
        if self.session.stash.entries.is_empty() {
            return;
        }
        let current = self
            .session
            .stash
            .selected
            .as_ref()
            .and_then(|selector| {
                self.session
                    .stash
                    .entries
                    .iter()
                    .position(|entry| &entry.selector == selector)
            })
            .unwrap_or(0);
        let next = if direction < 0 {
            current.saturating_sub(1)
        } else {
            (current + 1).min(self.session.stash.entries.len() - 1)
        };
        self.select_stash(self.session.stash.entries[next].selector.clone(), cx);
    }

    pub(super) fn stash_changes(&mut self, cx: &mut Context<Self>) {
        if matches!(self.session.operation, OperationState::Running) {
            return;
        }
        self.run_operation(GitOperation::StashPush, None, false, cx);
    }

    pub(super) fn apply_selected_stash(&mut self, cx: &mut Context<Self>) {
        let Some(selector) = self.session.stash.selected.clone() else {
            return;
        };
        self.apply_stash(selector, cx);
    }

    pub(super) fn apply_stash(&mut self, selector: String, cx: &mut Context<Self>) {
        if self
            .session
            .stash
            .entries
            .iter()
            .all(|entry| entry.selector != selector)
            || matches!(self.session.operation, OperationState::Running)
        {
            return;
        }
        self.run_operation(
            GitOperation::StashApply(selector),
            self.session.selected.clone(),
            false,
            cx,
        );
    }

    pub(super) fn pop_stash(&mut self, selector: String, cx: &mut Context<Self>) {
        if self
            .session
            .stash
            .entries
            .iter()
            .all(|entry| entry.selector != selector)
            || matches!(self.session.operation, OperationState::Running)
        {
            return;
        }
        self.run_operation(
            GitOperation::StashPop(selector),
            self.session.selected.clone(),
            false,
            cx,
        );
    }

    pub(super) fn request_drop_stash(&mut self, selector: String, cx: &mut Context<Self>) {
        if self
            .session
            .stash
            .entries
            .iter()
            .any(|entry| entry.selector == selector)
            && !matches!(self.session.operation, OperationState::Running)
        {
            self.pending_stash_drop = Some(selector);
            cx.notify();
        }
    }

    pub(super) fn cancel_drop_stash(&mut self, cx: &mut Context<Self>) {
        if self.pending_stash_drop.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn confirm_drop_stash(&mut self, cx: &mut Context<Self>) {
        if matches!(self.session.operation, OperationState::Running) {
            return;
        }
        let Some(selector) = self.pending_stash_drop.take() else {
            return;
        };
        self.run_operation(
            GitOperation::StashDrop(selector),
            self.session.selected.clone(),
            false,
            cx,
        );
    }

    fn refresh_list_with_diff_reload(&mut self, force_diff_reload: bool, cx: &mut Context<Self>) {
        let Some(request) = self.session.begin_refresh(force_diff_reload) else {
            return;
        };
        let cwd = request.cwd.clone();

        cx.spawn(async move |weak, cx| {
            let scan = cx
                .background_executor()
                .spawn(async move { scan_changes(&cwd) })
                .await;
            let _ = weak.update(cx, |this, cx| {
                let completion = this.session.apply_scan(request, scan);
                if completion.reload_diff {
                    this.refresh_diff(cx);
                }
                if completion.refresh_again {
                    this.refresh_list(cx);
                }
                if completion.state_changed {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn refresh_diff(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.session.begin_diff() else {
            return;
        };
        let cwd = request.cwd.clone();
        let entry = request.entry.clone();

        cx.spawn(async move |weak, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { diff(&cwd, &entry, entry.staged) })
                .await;
            let _ = weak.update(cx, |this, cx| {
                if this.session.apply_diff(request, result) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn ensure_refresh_loop(&mut self, cx: &mut Context<Self>) {
        if self.refresh_task.is_some() {
            return;
        }
        self.refresh_task = Some(cx.spawn(async move |weak, cx| {
            loop {
                cx.background_executor().timer(REFRESH_INTERVAL).await;
                if weak.update(cx, |this, cx| this.refresh_list(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    pub(super) fn select(
        &mut self,
        key: ChangeKey,
        additive: bool,
        range: bool,
        cx: &mut Context<Self>,
    ) {
        let changed = self.session.selected.as_ref() != Some(&key);
        self.session.select(key, additive, range);
        if self.compact_layout {
            self.compact_page = if self.session.selected.is_some() {
                CompactPage::Diff
            } else {
                CompactPage::Changes
            };
        }
        if changed || self.session.selected.is_none() {
            self.diff_scroll
                .scroll_to_item_strict(0, gpui::ScrollStrategy::Top);
            self.refresh_diff(cx);
        }
        cx.notify();
    }

    pub(super) fn move_selection(&mut self, direction: i8, cx: &mut Context<Self>) {
        if self.session.changes.is_empty() {
            return;
        }
        let current =
            selected_index(&self.session.changes, self.session.selected.as_ref()).unwrap_or(0);
        let next = if direction < 0 {
            current.saturating_sub(1)
        } else {
            (current + 1).min(self.session.changes.len() - 1)
        };
        self.select(
            ChangeKey::from(&self.session.changes[next]),
            false,
            false,
            cx,
        );
    }

    pub(super) fn toggle_selected_stage(&mut self, cx: &mut Context<Self>) {
        let working_paths = self.session.selected_paths(false);
        if !working_paths.is_empty() {
            self.stage_paths(working_paths, cx);
        } else {
            let staged_paths = self.session.selected_paths(true);
            self.unstage_paths(staged_paths, cx);
        }
    }

    pub(super) fn stage_paths(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        self.run_paths_operation(false, paths, cx);
    }

    pub(super) fn unstage_paths(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        self.run_paths_operation(true, paths, cx);
    }

    pub(super) fn stage_selected(&mut self, cx: &mut Context<Self>) {
        self.stage_paths(self.session.selected_paths(false), cx);
    }

    pub(super) fn unstage_selected(&mut self, cx: &mut Context<Self>) {
        self.unstage_paths(self.session.selected_paths(true), cx);
    }

    pub(super) fn toggle_hunk(&mut self, hunk_index: usize, cx: &mut Context<Self>) {
        if matches!(self.session.operation, OperationState::Running) {
            return;
        }
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        if matches!(
            entry.status,
            crossh_core::git::ChangeStatus::Untracked | crossh_core::git::ChangeStatus::Conflict
        ) {
            return;
        }
        let desired = Some(ChangeKey {
            path: entry.path.clone(),
            staged: !entry.staged,
        });
        let operation = if entry.staged {
            GitOperation::UnstageHunk { entry, hunk_index }
        } else {
            GitOperation::StageHunk { entry, hunk_index }
        };
        self.run_operation(operation, desired, false, cx);
    }

    pub(super) fn resolve_conflict(
        &mut self,
        path: String,
        resolution: ConflictResolution,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.session.operation, OperationState::Running)
            || !self.session.changes.iter().any(|entry| {
                entry.path == path && entry.status == crossh_core::git::ChangeStatus::Conflict
            })
        {
            return;
        }
        self.run_operation(
            GitOperation::ResolveConflict { path, resolution },
            self.session.selected.clone(),
            false,
            cx,
        );
    }

    pub(super) fn select_all_changes(&mut self, cx: &mut Context<Self>) {
        let previous = self.session.selected.clone();
        self.session.select_all();
        if previous != self.session.selected {
            self.refresh_diff(cx);
        }
        cx.notify();
    }

    pub(super) fn clear_selection(&mut self, cx: &mut Context<Self>) {
        let had_selection = self.session.selected.is_some();
        self.session.clear_selection();
        if had_selection {
            self.refresh_diff(cx);
        }
        cx.notify();
    }

    pub(super) fn request_discard_selected(&mut self, cx: &mut Context<Self>) {
        if self.session.can_discard_selection() {
            self.pending_discard = Some(self.session.discard_paths());
            self.close_context_menu(cx);
            cx.notify();
        }
    }

    pub(super) fn cancel_discard(&mut self, cx: &mut Context<Self>) {
        if self.pending_discard.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn confirm_discard(&mut self, cx: &mut Context<Self>) {
        if matches!(self.session.operation, OperationState::Running) {
            return;
        }
        let Some(paths) = self.pending_discard.take() else {
            return;
        };
        let desired = self.session.selected.clone();
        self.run_operation(GitOperation::Discard(paths), desired, false, cx);
    }

    fn run_paths_operation(
        &mut self,
        currently_staged: bool,
        paths: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() || matches!(self.session.operation, OperationState::Running) {
            return;
        }
        let desired = if paths.len() == 1 {
            Some(ChangeKey {
                path: paths[0].clone(),
                staged: !currently_staged,
            })
        } else {
            self.session.selected.clone()
        };
        let operation = if currently_staged {
            GitOperation::Unstage(paths)
        } else {
            GitOperation::Stage(paths)
        };
        self.run_operation(operation, desired, false, cx);
    }

    pub(super) fn commit_changes(&mut self, cx: &mut Context<Self>) {
        if !self.can_commit() {
            return;
        }
        self.run_operation(
            GitOperation::Commit(self.commit_editor.state.value.clone()),
            None,
            true,
            cx,
        );
    }

    pub(super) fn push_changes(&mut self, cx: &mut Context<Self>) {
        if !self.can_push() {
            return;
        }
        self.run_operation(GitOperation::Push, None, false, cx);
    }

    pub(super) fn pull_changes(&mut self, cx: &mut Context<Self>) {
        if !self.can_pull() {
            return;
        }
        self.run_operation(GitOperation::Pull, None, false, cx);
    }

    fn run_operation(
        &mut self,
        operation: GitOperation,
        desired_selection: Option<ChangeKey>,
        clear_message: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(request) =
            self.session
                .begin_operation(operation, desired_selection, clear_message)
        else {
            return;
        };
        let cwd = request.cwd.clone();
        let operation = request.operation.clone();
        let refresh_branches = matches!(&operation, GitOperation::SwitchBranch(_));
        let refresh_stashes = matches!(
            &operation,
            GitOperation::StashPush
                | GitOperation::StashApply(_)
                | GitOperation::StashPop(_)
                | GitOperation::StashDrop(_)
        );
        cx.notify();

        cx.spawn(async move |weak, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match operation {
                        GitOperation::Stage(paths) => stage(&cwd, &paths),
                        GitOperation::Unstage(paths) => unstage(&cwd, &paths),
                        GitOperation::Discard(paths) => discard_worktree(&cwd, &paths),
                        GitOperation::StageHunk { entry, hunk_index } => {
                            stage_hunk(&cwd, &entry, hunk_index)
                        }
                        GitOperation::UnstageHunk { entry, hunk_index } => {
                            unstage_hunk(&cwd, &entry, hunk_index)
                        }
                        GitOperation::Commit(message) => commit(&cwd, &message),
                        GitOperation::Push => push(&cwd),
                        GitOperation::Pull => pull(&cwd),
                        GitOperation::SwitchBranch(name) => checkout_branch(&cwd, &name),
                        GitOperation::StashPush => push_git_stash(&cwd),
                        GitOperation::StashApply(selector) => apply_git_stash(&cwd, &selector),
                        GitOperation::StashPop(selector) => pop_git_stash(&cwd, &selector),
                        GitOperation::StashDrop(selector) => drop_git_stash(&cwd, &selector),
                        GitOperation::ResolveConflict { path, resolution } => {
                            resolve_conflict(&cwd, &path, resolution)
                        }
                    }
                })
                .await;
            let _ = weak.update(cx, |this, cx| {
                let completion = this.session.apply_operation(request, result);
                if !completion.accepted {
                    return;
                }
                if completion.clear_message {
                    this.commit_editor.state.clear();
                }
                this.refresh_list(cx);
                if refresh_branches {
                    this.refresh_branches(true, cx);
                }
                if refresh_stashes {
                    this.refresh_stashes(true, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn staged_count(&self) -> usize {
        self.session
            .changes
            .iter()
            .filter(|change| change.staged)
            .count()
    }

    pub(super) fn can_commit(&self) -> bool {
        self.staged_count() > 0
            && !self.commit_editor.state.value.trim().is_empty()
            && !matches!(self.session.operation, OperationState::Running)
    }

    pub(super) fn can_push(&self) -> bool {
        self.session.status.is_some() && !matches!(self.session.operation, OperationState::Running)
    }

    pub(super) fn can_pull(&self) -> bool {
        self.session.status.is_some() && !matches!(self.session.operation, OperationState::Running)
    }

    pub(super) fn selected_entry(&self) -> Option<&crossh_core::git::FileChange> {
        selected_index(&self.session.changes, self.session.selected.as_ref())
            .and_then(|index| self.session.changes.get(index))
    }

    pub(super) fn open_context_menu(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let entries = context_menu::menu_entries(
            self.session.selected_count(),
            self.session.selected_paths(false).len(),
            self.session.selected_paths(true).len(),
            self.session.can_discard_selection(),
        );
        self.context_menu = Some(ContextMenuState { position, entries });
        cx.notify();
    }

    pub(super) fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn dispatch_menu_action(
        &mut self,
        action: GitMenuAction,
        _window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        self.close_context_menu(cx);
        match action {
            GitMenuAction::StageSelected => self.stage_selected(cx),
            GitMenuAction::UnstageSelected => self.unstage_selected(cx),
            GitMenuAction::DiscardSelected => self.request_discard_selected(cx),
            GitMenuAction::SelectAll => self.select_all_changes(cx),
            GitMenuAction::ClearSelection => self.clear_selection(cx),
        }
    }

    pub(super) fn back_to_changes(&mut self, cx: &mut Context<Self>) {
        match self.compact_page {
            CompactPage::Diff => self.compact_page = CompactPage::Changes,
            CompactPage::HistoryDetail => self.compact_page = CompactPage::History,
            CompactPage::History => self.compact_page = CompactPage::Changes,
            CompactPage::Branches => self.compact_page = CompactPage::Changes,
            CompactPage::Stashes => self.compact_page = CompactPage::Changes,
            CompactPage::Changes => return,
        }
        cx.notify();
    }
}

/// 打开或聚焦 Git 窗口；切换目录时复用现有窗口。
pub fn open_git_window(cwd: PathBuf, cx: &mut App) {
    if let Some(window) = cx
        .windows()
        .iter()
        .find_map(|handle| handle.downcast::<GitWindow>())
    {
        let _ = window.update(cx, |this, window, cx| {
            if this.session.cwd != cwd {
                this.session = GitSession::new(cwd);
                this.compact_page = CompactPage::Changes;
                this.commit_editor.state.clear();
                this.context_menu = None;
                this.pending_discard = None;
                this.pending_stash_drop = None;
                this.refresh_list(cx);
            }
            window.activate_window();
            cx.notify();
        });
        return;
    }

    if cx.windows().is_empty() {
        create_git_window(cwd, cx);
    } else {
        cx.defer(move |cx| create_git_window(cwd, cx));
    }
}

fn create_git_window(cwd: PathBuf, cx: &mut App) {
    let bounds = Bounds::centered(
        None,
        Size {
            width: px(1000.),
            height: px(640.),
        },
        cx,
    );
    cx.open_window(
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(path_display_name(&cwd).into()),
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(Size {
                width: px(720.),
                height: px(480.),
            }),
            ..Default::default()
        },
        |_window, cx| cx.new(|cx| GitWindow::new(cwd.clone(), cx)),
    )
    .expect("Git window should open");
    cx.activate(true);
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::open_git_window;

    #[gpui::test]
    fn cold_start_opens_git_window_synchronously(cx: &mut TestAppContext) {
        cx.update(|cx| {
            assert!(cx.windows().is_empty());
            open_git_window(std::env::temp_dir(), cx);
            assert_eq!(cx.windows().len(), 1);
        });
    }
}

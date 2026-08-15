//! Git 窗口状态、后台任务与窗口生命周期。

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, AppContext, Bounds, Context, FocusHandle, Size, Task, TitlebarOptions,
    UniformListScrollHandle, WindowBounds, WindowOptions, px,
};

use crossh_core::git::{FileChange, commit, diff, push, scan_changes, stage, unstage};
use crossh_core::project::GitStatus;

use super::editor::CommitEditor;
use super::model::{
    CHANGES_PANE_DEFAULT_WIDTH, ChangeKey, CompactPage, DiffState, OperationState, RefreshState,
    diff_uses_staged_baseline, reconcile_selection, selected_index, should_refresh_diff,
};

const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

enum GitOperation {
    Stage(Vec<String>),
    Unstage(Vec<String>),
    Commit(String),
    Push,
}

pub struct GitWindow {
    pub(super) cwd: PathBuf,
    pub(super) label: String,
    pub(super) changes: Vec<FileChange>,
    pub(super) status: Option<GitStatus>,
    pub(super) selected: Option<ChangeKey>,
    pub(super) diff: DiffState,
    pub(super) initial_loading: bool,
    pub(super) refresh: RefreshState,
    pub(super) load_error: Option<String>,
    pub(super) operation: OperationState,
    pub(super) compact_layout: bool,
    pub(super) compact_page: CompactPage,
    pub(super) staged_collapsed: bool,
    pub(super) working_collapsed: bool,
    pub(super) commit_editor: CommitEditor,
    pub(super) changes_focus: FocusHandle,
    pub(super) list_generation: u64,
    pub(super) diff_generation: u64,
    pub(super) operation_generation: u64,
    pub(super) _refresh_task: Option<Task<()>>,
    pub(super) changes_scroll: gpui::ScrollHandle,
    pub(super) diff_scroll: UniformListScrollHandle,
    force_diff_refresh_pending: bool,
    pub(super) changes_pane_width: Rc<Cell<f32>>,
    pub(super) changes_pane_dragging: Rc<Cell<bool>>,
}

impl GitWindow {
    pub(crate) fn new(cwd: PathBuf, cx: &mut Context<Self>) -> Self {
        let label = directory_label(&cwd);
        let mut git_window = Self {
            cwd,
            label,
            changes: Vec::new(),
            status: None,
            selected: None,
            diff: DiffState::Idle,
            initial_loading: true,
            refresh: RefreshState::default(),
            load_error: None,
            operation: OperationState::Idle,
            compact_layout: false,
            compact_page: CompactPage::Changes,
            staged_collapsed: false,
            working_collapsed: false,
            commit_editor: CommitEditor::new(cx.focus_handle()),
            changes_focus: cx.focus_handle(),
            list_generation: 0,
            diff_generation: 0,
            operation_generation: 0,
            _refresh_task: None,
            changes_scroll: gpui::ScrollHandle::new(),
            diff_scroll: UniformListScrollHandle::new(),
            force_diff_refresh_pending: false,
            changes_pane_width: Rc::new(Cell::new(CHANGES_PANE_DEFAULT_WIDTH)),
            changes_pane_dragging: Rc::new(Cell::new(false)),
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
            git_window.commit_editor.value = "完善 Git 窗口\n保持紧凑布局可用".to_string();
            git_window.commit_editor.cursor = git_window.commit_editor.value.len();
            if show_compact_diff {
                git_window.compact_page = CompactPage::Diff;
            }
            if show_error {
                git_window.operation = OperationState::Error(
                    "提交失败：请先配置 Git 用户名和邮箱，然后重试。".to_string(),
                );
            }
            git_window
        })
    }

    pub(super) fn refresh_list(&mut self, cx: &mut Context<Self>) {
        self.refresh_list_with_diff_reload(false, cx);
    }

    pub(super) fn force_refresh_list(&mut self, cx: &mut Context<Self>) {
        self.refresh_list_with_diff_reload(true, cx);
    }

    fn refresh_list_with_diff_reload(&mut self, force_diff_reload: bool, cx: &mut Context<Self>) {
        self.force_diff_refresh_pending |= force_diff_reload;
        if !self.refresh.request() {
            return;
        }
        let force_diff_reload = std::mem::take(&mut self.force_diff_refresh_pending);
        self.list_generation = self.list_generation.wrapping_add(1);
        let generation = self.list_generation;
        let cwd = self.cwd.clone();
        let previous_index = selected_index(&self.changes, self.selected.as_ref());
        let previous_changes = self.changes.clone();
        let previous_selected = self.selected.clone();
        let was_initial_loading = self.initial_loading;
        self.initial_loading = self.changes.is_empty();

        cx.spawn(async move |weak, cx| {
            let scan = cx
                .background_executor()
                .spawn(async move { scan_changes(&cwd) })
                .await;
            let _ = weak.update(cx, |this, cx| {
                let refresh_again = this.refresh.finish();
                if this.list_generation != generation {
                    if refresh_again {
                        this.refresh_list(cx);
                    }
                    return;
                }
                let mut state_changed = was_initial_loading;
                match scan {
                    Ok(scan) => {
                        let next_selected = reconcile_selection(
                            &scan.changes,
                            this.selected.as_ref(),
                            previous_index,
                        );
                        let reload_diff = should_refresh_diff(
                            force_diff_reload,
                            &previous_changes,
                            &scan.changes,
                            previous_selected.as_ref(),
                            next_selected.as_ref(),
                        );
                        state_changed |= this.changes != scan.changes;
                        state_changed |= this.selected != next_selected;
                        state_changed |= this.status != scan.status;
                        state_changed |= this.load_error.take().is_some();
                        this.changes = scan.changes;
                        this.selected = next_selected;
                        this.status = scan.status;
                        if reload_diff {
                            this.refresh_diff(cx);
                        }
                    }
                    Err(error) => {
                        let error = error.to_string();
                        state_changed |= this.load_error.as_deref() != Some(error.as_str());
                        this.load_error = Some(error);
                    }
                }
                this.initial_loading = false;
                if refresh_again {
                    this.refresh_list(cx);
                }
                if state_changed {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn refresh_diff(&mut self, cx: &mut Context<Self>) {
        let Some(index) = selected_index(&self.changes, self.selected.as_ref()) else {
            self.diff = DiffState::Idle;
            return;
        };
        let entry = self.changes[index].clone();
        let key = ChangeKey::from(&entry);
        let cwd = self.cwd.clone();
        self.diff_generation = self.diff_generation.wrapping_add(1);
        let generation = self.diff_generation;
        let keep_current_diff = matches!(
            &self.diff,
            DiffState::Ready(current_key, _) if current_key == &key
        );
        if !keep_current_diff {
            self.diff = DiffState::Loading(key.clone());
        }

        cx.spawn(async move |weak, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let key = key.clone();
                    async move {
                        let staged = diff_uses_staged_baseline(&entry);
                        (key, diff(&cwd, &entry, staged))
                    }
                })
                .await;
            let _ = weak.update(cx, |this, cx| {
                if this.diff_generation != generation || this.selected.as_ref() != Some(&result.0) {
                    return;
                }
                this.diff = match result.1 {
                    Ok(file_diff) => DiffState::Ready(result.0, file_diff),
                    Err(error) => DiffState::Error(result.0, error.to_string()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn ensure_refresh_loop(&mut self, cx: &mut Context<Self>) {
        if self._refresh_task.is_some() {
            return;
        }
        self._refresh_task = Some(cx.spawn(async move |weak, cx| {
            loop {
                cx.background_executor().timer(REFRESH_INTERVAL).await;
                if weak.update(cx, |this, cx| this.refresh_list(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    pub(super) fn select(&mut self, key: ChangeKey, cx: &mut Context<Self>) {
        let changed = self.selected.as_ref() != Some(&key);
        self.selected = Some(key);
        if self.compact_layout {
            self.compact_page = CompactPage::Diff;
        }
        if changed {
            self.diff_scroll
                .scroll_to_item_strict(0, gpui::ScrollStrategy::Top);
            self.refresh_diff(cx);
        }
        cx.notify();
    }

    pub(super) fn move_selection(&mut self, direction: i8, cx: &mut Context<Self>) {
        if self.changes.is_empty() {
            return;
        }
        let current = selected_index(&self.changes, self.selected.as_ref()).unwrap_or(0);
        let next = if direction < 0 {
            current.saturating_sub(1)
        } else {
            (current + 1).min(self.changes.len() - 1)
        };
        self.select(ChangeKey::from(&self.changes[next]), cx);
    }

    pub(super) fn toggle_selected_stage(&mut self, cx: &mut Context<Self>) {
        let Some(index) = selected_index(&self.changes, self.selected.as_ref()) else {
            return;
        };
        let entry = &self.changes[index];
        self.run_paths_operation(entry.staged, vec![entry.path.clone()], cx);
    }

    pub(super) fn stage_paths(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        self.run_paths_operation(false, paths, cx);
    }

    pub(super) fn unstage_paths(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        self.run_paths_operation(true, paths, cx);
    }

    fn run_paths_operation(
        &mut self,
        currently_staged: bool,
        paths: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() || matches!(self.operation, OperationState::Running) {
            return;
        }
        let desired = (paths.len() == 1).then(|| ChangeKey {
            path: paths[0].clone(),
            staged: !currently_staged,
        });
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
            GitOperation::Commit(self.commit_editor.value.clone()),
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

    fn run_operation(
        &mut self,
        operation: GitOperation,
        desired_selection: Option<ChangeKey>,
        clear_message: bool,
        cx: &mut Context<Self>,
    ) {
        self.operation_generation = self.operation_generation.wrapping_add(1);
        let generation = self.operation_generation;
        let cwd = self.cwd.clone();
        self.operation = OperationState::Running;
        cx.notify();

        cx.spawn(async move |weak, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match operation {
                        GitOperation::Stage(paths) => stage(&cwd, &paths),
                        GitOperation::Unstage(paths) => unstage(&cwd, &paths),
                        GitOperation::Commit(message) => commit(&cwd, &message),
                        GitOperation::Push => push(&cwd),
                    }
                })
                .await;
            let _ = weak.update(cx, |this, cx| {
                if this.operation_generation != generation {
                    return;
                }
                match result {
                    Ok(()) => {
                        this.operation = OperationState::Idle;
                        this.selected = desired_selection;
                        if clear_message {
                            this.commit_editor.value.clear();
                            this.commit_editor.cursor = 0;
                            this.commit_editor.anchor = None;
                        }
                        this.refresh_list(cx);
                    }
                    Err(error) => {
                        this.operation = OperationState::Error(error.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn staged_count(&self) -> usize {
        self.changes.iter().filter(|change| change.staged).count()
    }

    pub(super) fn can_commit(&self) -> bool {
        self.staged_count() > 0
            && !self.commit_editor.value.trim().is_empty()
            && !matches!(self.operation, OperationState::Running)
    }

    pub(super) fn can_push(&self) -> bool {
        self.status.is_some() && !matches!(self.operation, OperationState::Running)
    }

    pub(super) fn selected_entry(&self) -> Option<&FileChange> {
        selected_index(&self.changes, self.selected.as_ref())
            .and_then(|index| self.changes.get(index))
    }

    pub(super) fn back_to_changes(&mut self, cx: &mut Context<Self>) {
        if self.compact_layout && self.compact_page == CompactPage::Diff {
            self.compact_page = CompactPage::Changes;
            cx.notify();
        }
    }
}

fn directory_label(cwd: &Path) -> String {
    cwd.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| cwd.to_string_lossy().into_owned())
}

/// 打开或聚焦 Git 窗口；切换目录时复用现有窗口。
pub fn open_git_window(cwd: PathBuf, cx: &mut App) {
    if let Some(window) = cx
        .windows()
        .iter()
        .find_map(|handle| handle.downcast::<GitWindow>())
    {
        let _ = window.update(cx, |this, window, cx| {
            if this.cwd != cwd {
                this.list_generation = this.list_generation.wrapping_add(1);
                this.diff_generation = this.diff_generation.wrapping_add(1);
                this.operation_generation = this.operation_generation.wrapping_add(1);
                this.cwd = cwd;
                this.label = directory_label(&this.cwd);
                this.changes.clear();
                this.selected = None;
                this.diff = DiffState::Idle;
                this.compact_page = CompactPage::Changes;
                this.operation = OperationState::Idle;
                this.load_error = None;
                this.commit_editor.value.clear();
                this.commit_editor.cursor = 0;
                this.commit_editor.anchor = None;
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
                title: Some(directory_label(&cwd).into()),
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

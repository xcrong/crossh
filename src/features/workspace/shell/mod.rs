//! 应用外壳：项目侧栏 + tab 条 + daemon 工作区 + 终端视图。
//!
//! 布局归 daemon 所有：外壳只读快照、把用户操作翻译成 `WorkspaceOp`，
//! 并按 `PaneId` 持有终端视图。渲染与连接的拆分见兄弟模块。

use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, AppContext, Context, Entity, FocusHandle, InteractiveElement, IntoElement, KeyBinding,
    KeyDownEvent, ParentElement, PathPromptOptions, Pixels, Point, Render, Styled, Subscription,
    Task, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, px, size,
};

use crate::features::settings::{self, SettingsSnapshot};
use crate::features::updates::{UpdateController, UpdateNotice, UpdateSettings};
use crate::features::workspace::command_palette::CommandPaletteState;
use crate::features::workspace::modal_editor::{DefaultCommandEditor, RenameEditor};
use crate::features::workspace::registry::WorkspaceState;
use crate::features::workspace::settings::WorkspaceSettings;
use crate::features::workspace::sidebar::{render_sidebar, render_sidebar_rail};
use crate::features::workspace::toaster::{ToastNotice, ToastTone};
use crate::features::workspace::view::{
    render_default_command_editor, render_main, render_rename_editor, render_workspace_status_bar,
};
use crossh_core::editor_launcher;
use crossh_core::git::{pull, push};
use crossh_core::git_remote::fetch_all_remotes;
use crossh_core::git_status::inspect;
use crossh_core::i18n::{self, LanguagePreference};
use crossh_core::system_stats::{SystemMonitorState, SystemSampler};
use crossh_core::text_editing::{EditingKeystroke, TextEditingState, handle_text_editing_key};
use crossh_session::WorkspaceConnection;
use crossh_session::workspace::{PaneId, ProjectId, TabId, WorkspaceOp};
use crossh_terminal::TerminalSettings;
use crossh_terminal_view::{TerminalItem as _, TerminalView};
use crossh_ui::context_menu::ShellMenuAction;
use crossh_ui::theme;
use crossh_ui_component::context_menu::{ContextMenuState, MenuEntry, render_context_menu};
use crossh_ui_component::{TextPointerEvent, TextPointerPhase};

use super::local_paths::{current_local_cwd, normalize_local_cwd};

mod compose;
#[cfg(any(target_os = "macos", windows))]
#[path = "link_preview.rs"]
pub(crate) mod link_preview;
#[cfg(not(any(target_os = "macos", windows)))]
#[path = "link_preview_unsupported.rs"]
pub(crate) mod link_preview;
mod notifications;
mod quit;
pub(crate) mod scratch;
mod settings_actions;
mod shell_input;
mod shell_render;
mod split;
mod tabs;
mod update_indicator;
mod workspace_connection;

actions!(
    shell,
    [
        CycleNextTab,
        CyclePrevTab,
        SwitchToTab1,
        SwitchToTab2,
        SwitchToTab3,
        SwitchToTab4,
        SwitchToTab5,
        SwitchToTab6,
        SwitchToTab7,
        SwitchToTab8,
        SwitchToTab9,
        SplitTerminalHorizontal,
        SplitTerminalVertical,
        FocusSplitLeft,
        FocusSplitRight,
        FocusSplitUp,
        FocusSplitDown,
    ]
);

/// 注册声明式快捷键：`cmd/ctrl+数字` 直达标签、`cmd/ctrl+tab/shift+tab` 循环标签。
pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-tab", CycleNextTab, Some("AppShell")),
        KeyBinding::new("ctrl-tab", CycleNextTab, Some("AppShell")),
        KeyBinding::new("cmd-shift-tab", CyclePrevTab, Some("AppShell")),
        KeyBinding::new("ctrl-shift-tab", CyclePrevTab, Some("AppShell")),
        KeyBinding::new("cmd-1", SwitchToTab1, Some("AppShell")),
        KeyBinding::new("ctrl-1", SwitchToTab1, Some("AppShell")),
        KeyBinding::new("cmd-2", SwitchToTab2, Some("AppShell")),
        KeyBinding::new("ctrl-2", SwitchToTab2, Some("AppShell")),
        KeyBinding::new("cmd-3", SwitchToTab3, Some("AppShell")),
        KeyBinding::new("ctrl-3", SwitchToTab3, Some("AppShell")),
        KeyBinding::new("cmd-4", SwitchToTab4, Some("AppShell")),
        KeyBinding::new("ctrl-4", SwitchToTab4, Some("AppShell")),
        KeyBinding::new("cmd-5", SwitchToTab5, Some("AppShell")),
        KeyBinding::new("ctrl-5", SwitchToTab5, Some("AppShell")),
        KeyBinding::new("cmd-6", SwitchToTab6, Some("AppShell")),
        KeyBinding::new("ctrl-6", SwitchToTab6, Some("AppShell")),
        KeyBinding::new("cmd-7", SwitchToTab7, Some("AppShell")),
        KeyBinding::new("ctrl-7", SwitchToTab7, Some("AppShell")),
        KeyBinding::new("cmd-8", SwitchToTab8, Some("AppShell")),
        KeyBinding::new("ctrl-8", SwitchToTab8, Some("AppShell")),
        KeyBinding::new("cmd-9", SwitchToTab9, Some("AppShell")),
        KeyBinding::new("ctrl-9", SwitchToTab9, Some("AppShell")),
        KeyBinding::new("cmd-d", SplitTerminalHorizontal, Some("AppShell")),
        KeyBinding::new("cmd-shift-d", SplitTerminalVertical, Some("AppShell")),
        KeyBinding::new("cmd-alt-left", FocusSplitLeft, Some("AppShell")),
        KeyBinding::new("cmd-alt-right", FocusSplitRight, Some("AppShell")),
        KeyBinding::new("cmd-alt-up", FocusSplitUp, Some("AppShell")),
        KeyBinding::new("cmd-alt-down", FocusSplitDown, Some("AppShell")),
    ]);
}

/// ponytail: 15s 足够——每次 tick 都要 fork 一次 git 子进程，5s 太密是 idle CPU 主来源之一。
const GIT_STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(15);
/// 后台 fetch 节拍：15s 轮询 × 20 = 约 5min fetch 一次，更新 behind 计数。
const GIT_FETCH_EVERY_TICKS: u64 = 20;

/// 状态栏 Git 同步操作的一次进行/错误状态；按 pane 独立记录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitSyncState {
    pub operation: GitSyncOperation,
    pub running: bool,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GitSyncOperation {
    Push,
    Pull,
}

pub struct AppShell {
    pub(crate) workspace: WorkspaceState,
    pub(crate) status: Option<String>,
    /// 侧栏搜索状态；与 Git/Note 筛选条同一编辑语义。
    pub(crate) search_query: TextEditingState,
    pub(crate) search_focus: FocusHandle,
    /// 应用外壳根焦点。
    pub(crate) shell_focus: FocusHandle,
    /// 原生项目目录选择器任务。
    _project_picker: Option<Task<()>>,
    pub(crate) language_preference: LanguagePreference,
    pub(crate) context_menu: Option<ContextMenuState<ShellMenuAction>>,
    pub(crate) terminal_settings: TerminalSettings,
    pub(crate) update_settings: UpdateSettings,
    pub(crate) updates: Entity<UpdateController>,
    update_notice: UpdateNotice,
    _updates_subscription: Subscription,
    pub(crate) workspace_settings: WorkspaceSettings,
    pub(crate) sidebar_width: Rc<Cell<f32>>,
    pub(crate) sidebar_dragging: Rc<Cell<bool>>,
    pub(crate) sidebar_scroll: gpui::ScrollHandle,
    pub(crate) tab_scroll: gpui::ScrollHandle,
    pub(crate) rename_editor: Option<RenameEditor>,
    pub(crate) default_command_editor: Option<DefaultCommandEditor>,
    pub(crate) command_palette: Option<CommandPaletteState>,
    pub(crate) compose_focus: FocusHandle,
    pub(crate) compose_scroll: gpui::ScrollHandle,
    _git_status_refresh_task: Option<Task<()>>,
    /// 最近一次状态栏 Git 同步操作的进行/错误状态，按 pane 独立记录。
    pub(crate) git_sync: BTreeMap<PaneId, GitSyncState>,
    git_fetching: HashSet<PaneId>,
    pub(crate) system_monitor: SystemMonitorState,
    system_sampler: Option<SystemSampler>,
    _system_monitor_task: Option<Task<()>>,
    pub(crate) scratch_visible: bool,
    pub(crate) scratch_terminal: Option<Entity<TerminalView>>,
    pub(crate) scratch_height: Rc<Cell<f32>>,
    pub(crate) scratch_dragging: Rc<Cell<bool>>,
    scratch_subscription: Option<Subscription>,
    pub(crate) link_preview: Option<link_preview::LinkPreview>,
    pub(crate) link_preview_visible: bool,
    shutdown_in_progress: bool,
    pub(crate) tab_close_confirmation_open: bool,
    /// 工作区订阅连接（attach + mutate）。
    pub(crate) workspace_connection: Option<Arc<WorkspaceConnection>>,
    _workspace_connection_task: Option<Task<()>>,
    /// 首次 attach 成功后才允许写 settings（旧设置迁移由 daemon 负责）。
    workspace_ready: bool,
    _activation_subscription: Option<Subscription>,
}

impl AppShell {
    /// 构造外壳：只读设置，连接 daemon 后由快照驱动布局。
    pub fn new(cx: &mut App) -> Entity<Self> {
        let snapshot = settings::load();
        let language_preference = snapshot.language;
        let terminal_settings = snapshot.terminal;
        TerminalView::apply_zed_settings(&terminal_settings, cx);
        let update_settings = snapshot.updates;
        let workspace_settings = snapshot.workspace;
        let updates = cx.new(|_| UpdateController::new(update_settings.clone()));

        let shell = cx.new(|cx| {
            let updates_subscription = cx.observe(
                &updates,
                |this: &mut Self, _updates, cx: &mut Context<Self>| {
                    this.sync_update_status(cx);
                },
            );
            Self {
                workspace: WorkspaceState::new(),
                status: None,
                search_query: TextEditingState::new(String::new()),
                search_focus: cx.focus_handle(),
                shell_focus: cx.focus_handle(),
                _project_picker: None,
                language_preference,
                context_menu: None,
                terminal_settings,
                update_settings,
                updates: updates.clone(),
                update_notice: UpdateNotice::default(),
                _updates_subscription: updates_subscription,
                workspace_settings,
                sidebar_width: Rc::new(Cell::new(theme::SIDEBAR_WIDTH)),
                sidebar_dragging: Rc::new(Cell::new(false)),
                sidebar_scroll: gpui::ScrollHandle::new(),
                tab_scroll: gpui::ScrollHandle::new(),
                rename_editor: None,
                default_command_editor: None,
                command_palette: None,
                compose_focus: cx.focus_handle(),
                compose_scroll: gpui::ScrollHandle::new(),
                _git_status_refresh_task: None,
                git_sync: BTreeMap::new(),
                git_fetching: HashSet::new(),
                system_monitor: SystemMonitorState::new(),
                system_sampler: None,
                _system_monitor_task: None,
                scratch_visible: false,
                scratch_terminal: None,
                scratch_height: Rc::new(Cell::new(0.)),
                scratch_dragging: Rc::new(Cell::new(false)),
                scratch_subscription: None,
                link_preview: None,
                link_preview_visible: false,
                shutdown_in_progress: false,
                tab_close_confirmation_open: false,
                workspace_connection: None,
                _workspace_connection_task: None,
                workspace_ready: false,
                _activation_subscription: None,
            }
        });
        updates.update(cx, |updates, cx| updates.start_startup_check(cx));
        shell.update(cx, |shell, cx| {
            shell.start_workspace_connection(cx);
            shell.ensure_git_status_refresh_task(cx);
        });
        shell
    }

    fn dispatch_shell_menu_action(
        &mut self,
        action: ShellMenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            ShellMenuAction::ChooseLocalProject => self.choose_project_directory(cx),
            ShellMenuAction::ActivateProject(project_id) => {
                self.send_workspace_op(
                    WorkspaceOp::ActivateProject {
                        project_id: ProjectId(project_id),
                    },
                    cx,
                );
            }
            ShellMenuAction::ActivateTab(tab_id) => {
                self.activate_tab(TabId(tab_id), cx);
            }
            ShellMenuAction::CreateTab(project_id) => {
                self.create_tab(Some(ProjectId(project_id)), cx);
            }
            ShellMenuAction::CloseTab(tab_id) => {
                self.request_close_tab(TabId(tab_id), window, cx);
            }
            ShellMenuAction::ClosePane(pane_id) => {
                self.request_close_pane(PaneId(pane_id), window, cx);
            }
            ShellMenuAction::CloseOtherTabs { project_id, keep } => {
                self.close_other_tabs(ProjectId(project_id), TabId(keep), cx);
            }
            ShellMenuAction::StopProject(project_id) => {
                self.send_workspace_op(
                    WorkspaceOp::StopProject {
                        project_id: Some(ProjectId(project_id)),
                    },
                    cx,
                );
            }
            ShellMenuAction::ForgetProject(project_id) => {
                self.send_workspace_op(
                    WorkspaceOp::ForgetProject {
                        project_id: Some(ProjectId(project_id)),
                    },
                    cx,
                );
            }
            ShellMenuAction::RenameTab(tab_id) => {
                self.open_rename_tab(TabId(tab_id), window, cx);
            }
            ShellMenuAction::EditDefaultCommand(tab_id) => {
                self.open_default_command_editor(TabId(tab_id), window, cx);
            }
            ShellMenuAction::ReloadDefaultCommand(tab_id) => {
                self.reload_default_command(TabId(tab_id), cx);
            }
            ShellMenuAction::ClearDefaultCommand(tab_id) => {
                self.set_tab_command(TabId(tab_id), None, cx);
            }
            ShellMenuAction::RetryPaneSession(pane_id) => {
                self.ensure_pane_session(PaneId(pane_id), cx);
            }
            ShellMenuAction::CopyText(text) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
            }
            ShellMenuAction::RevealInFinder(path) => {
                crossh_core::process::reveal_in_finder(&path);
            }
            ShellMenuAction::OpenProjectTerminal(project_id) => {
                let project_id = ProjectId(project_id);
                self.send_workspace_op(WorkspaceOp::ActivateProject { project_id }, cx);
                self.create_tab(Some(project_id), cx);
            }
        }
        self.close_context_menu(cx);
    }

    fn handle_shell_key_down(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 菜单打开时只响应 Escape（其余键被菜单模态拦截）。
        if self.context_menu.is_some() {
            if ev.keystroke.key == "escape" {
                self.close_context_menu(cx);
            }
            return;
        }
        if self.command_palette.is_some() {
            self.handle_command_palette_key(ev, window, cx);
            cx.stop_propagation();
            return;
        }
        if self.rename_editor.is_some() || self.default_command_editor.is_some() {
            return;
        }
        if self.link_preview_visible && ev.keystroke.key == "escape" {
            self.hide_link_preview(cx);
            cx.stop_propagation();
            return;
        }
        if self.scratch_visible && ev.keystroke.key == "escape" {
            self.hide_scratch_terminal(cx);
            cx.stop_propagation();
        }
    }

    fn open_query(&mut self, cx: &mut Context<Self>) {
        let query = self.search_query.value.trim().to_string();
        if query.is_empty() {
            return;
        }

        let query_lower = query.to_ascii_lowercase();
        if matches!(query_lower.as_str(), "project" | "projects" | "项目") {
            self.choose_project_directory(cx);
            return;
        }
        if matches!(query_lower.as_str(), "local" | "本地") {
            self.open_project_root(current_local_cwd(), cx);
            return;
        }

        if let Some(root) = self.workspace.snapshot.projects.iter().find_map(|project| {
            project
                .root
                .to_string_lossy()
                .to_ascii_lowercase()
                .contains(&query_lower)
                .then(|| project.root.clone())
        }) {
            self.open_project_root(root, cx);
            return;
        }

        self.search_query.clear();
        self.show_toast(
            ToastNotice::new(i18n::text("toast.search_no_match"), ToastTone::Info),
            cx,
        );
    }

    /// 通过原生目录选择器打开/激活项目；实际落点由 daemon 的 `OpenProject` 决定。
    pub(crate) fn choose_project_directory(&mut self, cx: &mut Context<Self>) {
        let paths_receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(i18n::text("project.choose_directory").into()),
        });
        let task = cx.spawn(async move |weak, cx| {
            let Ok(Ok(Some(paths))) = paths_receiver.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = weak.update(cx, |this, cx| {
                this.search_query.clear();
                this.open_project_root(path, cx);
            });
        });
        self._project_picker = Some(task);
    }

    pub(crate) fn handle_search_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &ev.keystroke;
        match ks.key.as_str() {
            "enter" | "return" => self.open_query(cx),
            "escape" => {
                self.search_query.clear();
                cx.notify();
            }
            "tab" => {
                self.search_focus.focus(window, cx);
            }
            _ => {
                let primary = ks.modifiers.control || ks.modifiers.platform;
                let paste_text = if primary && ks.key == "v" {
                    cx.read_from_clipboard()
                        .and_then(|item| item.text().map(|s| s.to_string()))
                } else {
                    None
                };
                let editing_ks = EditingKeystroke {
                    key: ks.key.clone(),
                    key_char: ks.key_char.clone(),
                    control: ks.modifiers.control,
                    platform: ks.modifiers.platform,
                    shift: ks.modifiers.shift,
                };
                let result = handle_text_editing_key(
                    &mut self.search_query,
                    &editing_ks,
                    paste_text.as_deref(),
                );
                if let Some(text) = result.copy_text {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                }
                if result.handled {
                    cx.notify();
                    cx.stop_propagation();
                }
            }
        }
    }

    /// 侧栏搜索框的鼠标选区：文本不变，只更新光标/选区，无需重跑过滤。
    pub(crate) fn handle_search_pointer(
        &mut self,
        event: &TextPointerEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let changed = match event.phase {
            TextPointerPhase::Down => {
                self.search_query
                    .pointer_down(event.byte_offset, event.extend, event.click_count)
            }
            TextPointerPhase::Drag => self.search_query.pointer_drag(event.byte_offset),
            TextPointerPhase::Up => self.search_query.pointer_up(),
        };
        if changed {
            cx.notify();
        }
    }

    pub(crate) fn open_context_menu(
        &mut self,
        position: Point<Pixels>,
        entries: Vec<MenuEntry<ShellMenuAction>>,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = Some(ContextMenuState { position, entries });
        cx.notify();
    }

    pub(crate) fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn open_project_in_editor(&mut self, directory: &Path, cx: &mut Context<Self>) {
        let path_env = editor_launcher::effective_path();
        self.open_project_in_editor_with_path_env(directory, path_env, cx);
    }

    pub(crate) fn open_project_in_editor_with_path_env(
        &mut self,
        directory: &Path,
        path_env: std::ffi::OsString,
        cx: &mut Context<Self>,
    ) {
        let editor = editor_launcher::resolve_editor(
            self.workspace_settings.editor_command.as_deref(),
            &path_env,
            editor_launcher::executable_exists,
        );
        let Some(binary) = editor else {
            self.show_toast(
                ToastNotice::new(i18n::text("toast.editor_not_found"), ToastTone::Error),
                cx,
            );
            return;
        };
        let mut process = editor_launcher::editor_process_command(&binary, directory);
        if let Err(error) = process.spawn() {
            log::warn!("failed to spawn editor {binary}: {error}");
            self.show_toast(
                ToastNotice::new(i18n::text("toast.editor_spawn_failed"), ToastTone::Error),
                cx,
            );
        }
    }

    /// 聚焦当前活动 pane 的终端；没有视图时回到外壳焦点。
    pub(crate) fn refocus_active_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(pane_id) = self.workspace.focused_pane() else {
            return;
        };
        if let Some(terminal) = self.workspace.pane_view(pane_id).cloned() {
            terminal.request_focus(cx);
        }
    }

    fn pane_cwd(&self, pane_id: PaneId) -> Option<PathBuf> {
        self.workspace.pane(pane_id).map(|pane| pane.cwd.clone())
    }

    fn refresh_git_status(&mut self, pane_id: PaneId, clear_stale: bool, cx: &mut Context<Self>) {
        let Some(cwd) = self.pane_cwd(pane_id) else {
            return;
        };
        let refresh = self.workspace.git_refresh.entry(pane_id).or_default();
        let status_cleared = clear_stale && self.workspace.git_status.remove(&pane_id).is_some();
        if !refresh.request() {
            if status_cleared {
                cx.notify();
            }
            return;
        }

        cx.spawn(async move |weak, cx| {
            let started = std::time::Instant::now();
            let cwd_for_log = cwd.clone();
            let status = cx
                .background_executor()
                .spawn(async move { inspect(&cwd) })
                .await;
            let elapsed = started.elapsed();
            if elapsed > Duration::from_secs(1) {
                log::warn!(
                    "git status inspection for {} took {}ms",
                    cwd_for_log.display(),
                    elapsed.as_millis()
                );
            }
            let _ = weak.update(cx, |this, cx| {
                let cwd_unchanged =
                    this.pane_cwd(pane_id).as_deref() == Some(cwd_for_log.as_path());
                let status_changed =
                    cwd_unchanged && this.workspace.git_status.get(&pane_id) != status.as_ref();
                if status_changed {
                    match &status {
                        Some(status) => {
                            this.workspace.git_status.insert(pane_id, status.clone());
                        }
                        None => {
                            this.workspace.git_status.remove(&pane_id);
                        }
                    }
                }
                let refresh_again = this
                    .workspace
                    .git_refresh
                    .get_mut(&pane_id)
                    .is_some_and(|refresh| refresh.finish());
                this.reconcile_git_sync_error(pane_id);
                if refresh_again {
                    this.refresh_git_status(pane_id, !cwd_unchanged, cx);
                }
                if status_changed {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn run_git_sync(
        &mut self,
        pane_id: PaneId,
        operation: GitSyncOperation,
        cx: &mut Context<Self>,
    ) {
        let Some(cwd) = self.pane_cwd(pane_id) else {
            return;
        };
        if self
            .git_sync
            .get(&pane_id)
            .is_some_and(|state| state.running)
        {
            return;
        }
        self.git_sync.insert(
            pane_id,
            GitSyncState {
                operation,
                running: true,
                error: None,
            },
        );
        cx.notify();

        cx.spawn(async move |weak, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match operation {
                        GitSyncOperation::Push => push(&cwd),
                        GitSyncOperation::Pull => pull(&cwd),
                    }
                })
                .await;
            let _ = weak.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.git_sync.remove(&pane_id);
                        this.show_toast(
                            ToastNotice::new(
                                i18n::text(match operation {
                                    GitSyncOperation::Push => "git.push_success",
                                    GitSyncOperation::Pull => "git.pull_success",
                                }),
                                ToastTone::Success,
                            ),
                            cx,
                        );
                    }
                    Err(error) => {
                        let Some(state) = this.git_sync.get_mut(&pane_id) else {
                            return;
                        };
                        state.running = false;
                        state.error = Some(error.to_string());
                        this.show_toast(
                            ToastNotice::new(
                                i18n::text(match operation {
                                    GitSyncOperation::Push => "git.push_failed",
                                    GitSyncOperation::Pull => "git.pull_failed",
                                }),
                                ToastTone::Error,
                            ),
                            cx,
                        );
                    }
                }
                this.refresh_git_status(pane_id, false, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn reconcile_git_sync_error(&mut self, pane_id: PaneId) {
        let Some(state) = self.git_sync.get(&pane_id) else {
            return;
        };
        if state.error.is_none() {
            return;
        }
        let Some(status) = self.workspace.git_status.get(&pane_id) else {
            return;
        };
        let resolved = match state.operation {
            GitSyncOperation::Pull => status.behind == 0,
            GitSyncOperation::Push => status.ahead == 0,
        };
        if resolved {
            self.git_sync.remove(&pane_id);
        }
    }

    fn fetch_then_refresh_git_status(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        if self
            .git_sync
            .get(&pane_id)
            .is_some_and(|state| state.running)
        {
            self.refresh_git_status(pane_id, false, cx);
            return;
        }
        if !self.git_fetching.insert(pane_id) {
            self.refresh_git_status(pane_id, false, cx);
            return;
        }
        let Some(cwd) = self.pane_cwd(pane_id) else {
            return;
        };
        // ponytail: 只轮询聚焦 pane，切回来再刷其余 pane，避免多项目同时 fetch 惊群。
        cx.spawn(async move |weak, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { fetch_all_remotes(&cwd) })
                .await;
            let _ = weak.update(cx, |this, cx| {
                this.git_fetching.remove(&pane_id);
                if let Err(error) = result {
                    log::debug!("background git fetch failed: {error}");
                }
                this.refresh_git_status(pane_id, false, cx);
            });
        })
        .detach();
    }

    fn ensure_git_status_refresh_task(&mut self, cx: &mut Context<Self>) {
        if self._git_status_refresh_task.is_some() {
            return;
        }

        self._git_status_refresh_task = Some(cx.spawn(async move |weak, cx| {
            let mut tick = 0u64;
            loop {
                cx.background_executor()
                    .timer(GIT_STATUS_REFRESH_INTERVAL)
                    .await;
                tick += 1;

                if weak
                    .update(cx, |this, cx| {
                        if let Some(pane_id) = this.workspace.focused_pane() {
                            if tick.is_multiple_of(GIT_FETCH_EVERY_TICKS) {
                                this.fetch_then_refresh_git_status(pane_id, cx);
                            } else {
                                this.refresh_git_status(pane_id, false, cx);
                            }
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    pub(crate) fn toggle_system_monitor(&mut self, cx: &mut Context<Self>) {
        self.system_monitor.toggle();
        if self.system_monitor.visible {
            self.system_sampler = Some(SystemSampler::new());
            self.start_system_monitor_task(cx);
        } else {
            self.system_sampler = None;
            self._system_monitor_task.take();
        }
        cx.notify();
    }

    fn start_system_monitor_task(&mut self, cx: &mut Context<Self>) {
        self._system_monitor_task.take();
        let expected_generation = self.system_monitor.generation;
        self._system_monitor_task = Some(cx.spawn(async move |weak, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(2))
                    .await;
                let now = std::time::Instant::now();
                let should_continue = weak
                    .update(cx, |this, cx| {
                        if this.system_monitor.generation != expected_generation
                            || !this.system_monitor.visible
                        {
                            return false;
                        }
                        if let Some(sampler) = this.system_sampler.as_mut() {
                            let snapshot = sampler.sample(now);
                            // ponytail: 快照不变就不 notify，避免每 2s 强制重绘整个 AppShell。
                            if this.system_monitor.snapshot.as_ref() != Some(&snapshot) {
                                this.system_monitor
                                    .apply_snapshot(snapshot, expected_generation);
                                cx.notify();
                            }
                        }
                        true
                    })
                    .unwrap_or(false);
                if !should_continue {
                    break;
                }
            }
        }));
        let now = std::time::Instant::now();
        if let Some(sampler) = self.system_sampler.as_mut() {
            let snapshot = sampler.sample(now);
            let generation = self.system_monitor.generation;
            self.system_monitor.apply_snapshot(snapshot, generation);
        }
    }

    /// 只写设置全局状态与磁盘，不重放终端设置。首次 attach 前不写，避免抢在
    /// daemon 迁移旧设置之前重写文件。
    pub(crate) fn persist_settings(&self) {
        if !self.workspace_ready {
            return;
        }
        let snapshot = SettingsSnapshot {
            language: self.language_preference,
            terminal: self.terminal_settings.clone(),
            updates: self.update_settings.clone(),
            workspace: self.workspace_settings.clone(),
        };
        if let Err(error) = settings::save(&snapshot) {
            log::warn!("failed to save settings: {error}");
        }
    }

    pub(crate) fn quit_for_update(&mut self, cx: &mut Context<Self>) {
        if self.shutdown_in_progress {
            return;
        }
        self.begin_shutdown(cx);
        cx.quit();
    }

    /// 发送布局操作；断开期间禁用并提示重连状态。
    pub(crate) fn send_workspace_op(&mut self, op: WorkspaceOp, cx: &mut Context<Self>) {
        if !self.workspace.is_connected() {
            self.show_toast(
                ToastNotice::new(i18n::text("workspace.disconnected"), ToastTone::Info),
                cx,
            );
            return;
        }
        let Some(connection) = self.workspace_connection.clone() else {
            self.show_toast(
                ToastNotice::new(i18n::text("workspace.disconnected"), ToastTone::Info),
                cx,
            );
            return;
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        crate::app::bootstrap::runtime().spawn(async move {
            let _ = tx.send(connection.mutate(op, None).await);
        });
        cx.spawn(async move |weak, cx| {
            let Some(result) = rx.recv().await else {
                return;
            };
            let _ = weak.update(cx, |this, cx| this.handle_mutate_result(result, cx));
        })
        .detach();
    }

    fn handle_mutate_result(
        &mut self,
        result: std::io::Result<crossh_session::protocol::WorkspaceMutateReply>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(reply) => {
                if let Some(error) = reply.error {
                    self.show_toast(ToastNotice::new(error.message, ToastTone::Error), cx);
                }
            }
            Err(error) => {
                self.workspace.connection = crossh_terminal::ConnState::Error(error.to_string());
                self.status = Some(
                    rust_i18n::t!("workspace.connection_failed", error = error.to_string())
                        .to_string(),
                );
                cx.notify();
            }
        }
    }
}

/// 打开主窗口。在 main.rs 中调用。
pub fn open_main_window(cx: &mut App) {
    let bounds = gpui::Bounds::centered(None, size(px(1100.), px(720.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("crossh".into()),
                ..Default::default()
            }),
            window_min_size: Some(gpui::Size {
                width: px(700.),
                height: px(420.),
            }),
            app_id: Some("me.xcrong.crossh".into()),
            ..Default::default()
        },
        |window, cx| {
            let shell = AppShell::new(cx);
            let weak = shell.downgrade();
            let notification_weak = shell.downgrade();
            let window_handle = window.window_handle();
            {
                let opener_shell = weak.clone();
                crossh_terminal_view::set_link_opener(
                    crossh_terminal_view::LinkOpener(std::rc::Rc::new(move |url, cx| {
                        window_handle
                            .update(cx, |_, window, cx| {
                                opener_shell
                                    .update(cx, |shell, cx| {
                                        shell.open_link_preview(url, window, cx)
                                    })
                                    .unwrap_or(false)
                            })
                            .unwrap_or(false)
                    })),
                    cx,
                );
            }
            cx.on_system_notification_response(move |response, cx| {
                let handled = notification_weak
                    .update(cx, |shell, cx| {
                        shell.handle_system_notification_response(response, cx)
                    })
                    .unwrap_or(false);
                if handled {
                    let _ = window_handle.update(cx, |_, window, _| window.activate_window());
                }
            });
            let activation = shell.update(cx, |_shell, cx| {
                cx.observe_window_activation(window, move |shell, _window, cx| {
                    if shell.workspace.is_connected() {
                        shell.send_workspace_op(WorkspaceOp::RefreshAvailability, cx);
                    }
                })
            });
            shell.update(cx, |shell, _| {
                shell._activation_subscription = Some(activation);
            });
            window.on_window_should_close(cx, move |_window, cx| {
                weak.update(cx, |shell, cx| shell.should_close_window(cx))
                    .unwrap_or(true)
            });
            shell
        },
    )
    .expect("Failed to open window");
    cx.activate(true);
}

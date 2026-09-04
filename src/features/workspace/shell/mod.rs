//! 应用外壳：左侧主机列表 + 顶部标签条 + 终端工作区 + 模态弹窗。
//!
//! - 连接池：同主机复用一条已认证会话（开新终端 channel），全部终端关闭才断开。
//! - 多标签：侧栏点击主机优先切换已有终端，显式新建时才追加标签；可切换/关闭。
//! - sidebar：Local 为主视图，Active/Bank 已按需求隐藏（导航数据仍保留，仅作过滤/直达用），见 sidebar.rs:188
//! - 模态：池中任一连接出现 pending_prompt（未知主机密钥/凭据）时弹覆盖层。
//!
//! 渲染与交互拆分为兄弟模块：`sidebar`（侧栏）、`workspace`（标签条+主区）、
//! `settings`（设置页）、`prompt`（模态弹窗）。本模块只保留状态与行为。

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, AppContext, Context, Entity, EntityId, FocusHandle, InteractiveElement, IntoElement,
    KeyBinding, KeyDownEvent, ParentElement, PathPromptOptions, Pixels, Point, Render, Styled,
    Subscription, Task, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, px,
    size,
};

use crate::features::editor_launcher;
use crate::features::settings::{self, SettingsSnapshot};
use crate::features::terminal::{TerminalEvent, TerminalView};
use crate::features::updates::{UpdateController, UpdateSettings};
use crate::features::workspace::command_palette::CommandPaletteState;
use crate::features::workspace::modal_editor::{DefaultCommandEditor, RenameEditor};
use crate::features::workspace::pinned::{pinned_tabs_for_project, prune_missing_pinned_tabs};
use crate::features::workspace::registry::WorkspaceState;
use crate::features::workspace::settings::WorkspaceSettings;
use crate::features::workspace::sidebar::{render_sidebar, render_sidebar_rail};
use crate::features::workspace::state::rebuild_local_dirs;
use crate::features::workspace::toaster::{ToastNotice, ToastTone};
use crate::features::workspace::view::{
    ActiveView, LocalDir, LocalSession, LocalSessionId, render_default_command_editor, render_main,
    render_rename_editor, render_workspace_status_bar,
};
use crate::shared::i18n::{self, LanguagePreference};
use crate::shared::text_editing::{EditingKeystroke, TextEditingState, handle_text_editing_key};
use crossh_core::git::{pull, push};
use crossh_core::git_status::inspect;
use crossh_core::system_stats::{SystemMonitorState, SystemSampler};
use crossh_terminal::TerminalSettings;
use crossh_ui::context_menu::ShellMenuAction;
use crossh_ui::theme;
use crossh_ui_component::context_menu::{ContextMenuState, MenuEntry, render_context_menu};

use super::local_paths::{current_local_cwd, normalize_local_cwd, normalize_recent_dirs};

mod compose;
mod notifications;
mod quit;
pub(crate) mod scratch;
mod settings_actions;
mod shell_input;
mod shell_render;
mod split;
mod tabs;

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
    ]
);

/// 注册声明式快捷键：`cmd/ctrl+数字` 直达标签、`cmd/ctrl+tab/shift+tab` 循环标签。
///
/// 消融前这些由 `handle_shell_key_down` 命令式硬编码，无法被 `Keymap` 冒泡/覆盖，
/// 也无法在 `key_context` 维度做隔离。此处迁至 `KeyBinding` 后，
/// `Terminal` 聚焦时仍通过 `context_stack` 冒泡命中 `AppShell`，且可用
/// `KeyBinding` 的 `context_predicate` 精确裁剪。
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
    ]);
}

const GIT_STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

/// 状态栏 Git 同步操作（push/pull）的一次进行/错误状态；按会话独立记录，
/// 避免一个会话的在途结果被另一个会话覆盖。
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
    /// 侧栏搜索状态；与 Git/Note 筛选条同一编辑语义（`TextEditingState` + 共享分发）。
    pub(crate) search_query: TextEditingState,
    pub(crate) search_focus: FocusHandle,
    /// 应用外壳根焦点；无任何终端/输入框聚焦时持有，保证窗口级动作
    /// （如 Cmd+Q → Quit）始终有合法的 dispatch 目标。
    pub(crate) shell_focus: FocusHandle,
    /// 原生项目目录选择器任务，持有到选择结果返回。
    _project_picker: Option<Task<()>>,
    /// 模态文本输入缓冲（密码/口令）。
    /// 模态输入框焦点。
    /// 上一帧是否有活动模态（用于在弹窗出现时自动聚焦）。
    /// 当前语言偏好；实际 locale 经 [`crate::shared::i18n::set_locale`] 切换。
    pub(crate) language_preference: LanguagePreference,
    /// 当前打开的右键上下文菜单（None = 未打开）。
    pub(crate) context_menu: Option<ContextMenuState<ShellMenuAction>>,
    pub(crate) terminal_settings: TerminalSettings,
    pub(crate) update_settings: UpdateSettings,
    pub(crate) updates: Entity<UpdateController>,
    pub(crate) workspace_settings: WorkspaceSettings,
    /// 侧栏宽度与拖动状态；只影响布局，不改变导航状态。
    pub(crate) sidebar_width: Rc<Cell<f32>>,
    pub(crate) sidebar_dragging: Rc<Cell<bool>>,
    pub(crate) sidebar_scroll: gpui::ScrollHandle,
    pub(crate) tab_scroll: gpui::ScrollHandle,
    /// Left terminal split drag state; the width cell itself lives per-owner
    /// in `WorkspaceState.split_widths` (one slot per split owner).
    pub(crate) terminal_split_dragging: Rc<Cell<bool>>,
    pub(crate) terminal_split_vertical_dragging: Rc<Cell<bool>>,
    pub(crate) terminal_split_vertical_right_dragging: Rc<Cell<bool>>,
    /// 固定标签重命名弹窗状态；与 default command 编辑器互斥（都是模态弹窗）。
    pub(crate) rename_editor: Option<RenameEditor>,
    pub(crate) default_command_editor: Option<DefaultCommandEditor>,
    pub(crate) command_palette: Option<CommandPaletteState>,
    pub(crate) compose_focus: FocusHandle,
    pub(crate) compose_scroll: gpui::ScrollHandle,
    /// 周期性刷新本地会话的 Git 状态，覆盖 shell 空闲时的外部文件变更。
    _git_status_refresh_task: Option<Task<()>>,
    /// 最近一次状态栏 Git 同步操作的进行/错误状态，按会话独立记录。
    pub(crate) git_sync: BTreeMap<LocalSessionId, GitSyncState>,
    pub(crate) system_monitor: SystemMonitorState,
    system_sampler: Option<SystemSampler>,
    _system_monitor_task: Option<Task<()>>,
    pub(crate) scratch_visible: bool,
    pub(crate) scratch_terminal: Option<Entity<TerminalView>>,
    pub(crate) scratch_height: Rc<Cell<f32>>,
    pub(crate) scratch_dragging: Rc<Cell<bool>>,
    scratch_subscription: Option<Subscription>,
    quit_confirmation_open: bool,
    shutdown_in_progress: bool,
    /// 标签页关闭确认框是否已打开，防止重复弹出。
    pub(crate) tab_close_confirmation_open: bool,
}

impl AppShell {
    /// 构造外壳（本地会话）。
    pub fn new(cx: &mut App) -> Entity<Self> {
        let mut snapshot = settings::load();
        let recent_dirs = normalize_recent_dirs(snapshot.workspace.recent_dirs.clone());
        if recent_dirs != snapshot.workspace.recent_dirs {
            snapshot.workspace.recent_dirs = recent_dirs;
            if let Err(error) = settings::save(&snapshot) {
                log::warn!("failed to save cleaned recent directories: {error}");
            }
        }
        // 启动时清理固定标签记录中已失效的目录；不存在的目录不得阻碍启动，
        // 也不得在每次启动时反复恢复（契约 6）。
        let pinned_restore =
            prune_missing_pinned_tabs(snapshot.workspace.pinned_local_tabs.clone());
        if pinned_restore != snapshot.workspace.pinned_local_tabs {
            snapshot.workspace.pinned_local_tabs = pinned_restore.clone();
            if let Err(error) = settings::save(&snapshot) {
                log::warn!("failed to save cleaned pinned tabs: {error}");
            }
        }
        let language_preference = snapshot.language;
        let terminal_settings = snapshot.terminal;
        TerminalView::apply_zed_settings(&terminal_settings, cx);
        let update_settings = snapshot.updates;
        let workspace_settings = snapshot.workspace;
        // 启动不自动打开任何会话（契约 5 Rev-2）：固定记录保留在持久化
        // 列表中，待用户打开对应项目时由契约 11 恢复。
        let updates = cx.new(|_| UpdateController::new(update_settings.clone()));
        // 启动时把最近的本地目录记录恢复到侧栏 Local 分组（无活动会话，点击即重开）。
        let mut local_dirs = BTreeMap::new();
        for cwd in &workspace_settings.recent_dirs {
            local_dirs.insert(
                cwd.clone(),
                LocalDir {
                    project_dir: cwd.clone(),
                    sessions: Vec::new(),
                    active_session: None,
                },
            );
        }

        let shell = cx.new(|cx| Self {
            workspace: WorkspaceState::new(local_dirs),
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
            workspace_settings,
            sidebar_width: Rc::new(Cell::new(theme::SIDEBAR_WIDTH)),
            sidebar_dragging: Rc::new(Cell::new(false)),
            sidebar_scroll: gpui::ScrollHandle::new(),
            tab_scroll: gpui::ScrollHandle::new(),
            terminal_split_dragging: Rc::new(Cell::new(false)),
            terminal_split_vertical_dragging: Rc::new(Cell::new(false)),
            terminal_split_vertical_right_dragging: Rc::new(Cell::new(false)),
            rename_editor: None,
            default_command_editor: None,
            command_palette: None,
            compose_focus: cx.focus_handle(),
            compose_scroll: gpui::ScrollHandle::new(),
            _git_status_refresh_task: None,
            git_sync: BTreeMap::new(),
            system_monitor: SystemMonitorState::new(),
            system_sampler: None,
            _system_monitor_task: None,
            scratch_visible: false,
            scratch_terminal: None,
            scratch_height: Rc::new(Cell::new(0.)),
            scratch_dragging: Rc::new(Cell::new(false)),
            scratch_subscription: None,
            quit_confirmation_open: false,
            shutdown_in_progress: false,
            tab_close_confirmation_open: false,
        });
        updates.update(cx, |updates, cx| updates.start_startup_check(cx));
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
            ShellMenuAction::ActivateLocalProject(path) => self.activate_local_dir(path, cx),
            ShellMenuAction::CopyText(text) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
            }
            ShellMenuAction::RevealInFinder(path) => {
                crossh_core::process::reveal_in_finder(&path);
            }
            ShellMenuAction::ForgetLocalDir(cwd) => self.forget_local_dir(cwd, cx),
            ShellMenuAction::StopLocalProject(cwd) => self.stop_local_project(cwd, window, cx),
            ShellMenuAction::OpenLocalTerminal(cwd) => {
                let _ = self.open_local_session(cwd.clone(), cwd.clone(), cx);
                // 「打开本地终端」即打开项目：同步恢复该项目尚无会话的
                // 固定记录（契约 11 Rev-1）。不能放进 open_local_session
                // 自身，否则恢复路径会递归（恢复内部也调用 open）。
                self.restore_pinned_tabs_for_project(&cwd, cx);
            }
            ShellMenuAction::SelectLocalSession(session_id) => {
                self.select_local_session(session_id, cx);
            }
            ShellMenuAction::PinLocalSession(session_id) => self.pin_local_session(session_id, cx),
            ShellMenuAction::UnpinLocalSession(session_id) => {
                self.unpin_local_session(session_id, cx)
            }
            ShellMenuAction::RenameLocalSession(session_id) => {
                self.open_rename_local_session(session_id, window, cx)
            }
            ShellMenuAction::EditDefaultCommand(session_id) => {
                self.open_default_command_editor(session_id, window, cx)
            }
            ShellMenuAction::ReloadDefaultCommand(session_id) => {
                self.reload_default_command(session_id, cx)
            }
            ShellMenuAction::ClearDefaultCommand(session_id) => {
                self.clear_default_command(session_id, cx)
            }
            ShellMenuAction::CloseLocalSession(session_id) => {
                self.request_close_local_session(session_id, window, cx);
            }
            ShellMenuAction::CloseOtherLocalSessions(session_id) => {
                self.close_other_local_sessions(session_id, cx);
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
            // 失焦兜底：焦点在输入框时内层已消费并截断（消费即截断归 handler 所有），
            // 此处只处理焦点异常；未处理键在此统一屏蔽，避免终端/全局误触。
            // 不得在此之外再注册第二个编辑分发——同一事件两次插入即 `nneeww`。
            self.handle_command_palette_key(ev, window, cx);
            cx.stop_propagation();
            return;
        }
        if self.rename_editor.is_some() || self.default_command_editor.is_some() {
            return;
        }
        // Scratch 抽屉的 Esc 隐藏：优先级高于全局快捷键，但低于模态
        if self.scratch_visible && ev.keystroke.key == "escape" {
            self.hide_scratch_terminal(cx);
            cx.stop_propagation();
        }
        // 消融完成：`cmd/ctrl+tab` 与 `cmd/ctrl+1..9` 已迁至声明式 `KeyBinding`
        //（`self::init`），此处不再做命令式分发。保留模态/抽屉的 `Escape`
        // 拦截与 `stop_propagation`，其余未命中按键交由 `Keymap` 冒泡或终端直通。
    }

    /// 在项目目录 view 中打开一个独立的 Zed terminal session。
    /// `project_dir` 决定侧栏归属，`cwd` 只决定 shell 的初始工作目录。
    ///
    /// 打开新会话不取消源 Tab 的分栏：分栏跟随其属主 Tab。
    /// 返回新会话 id；目录失效无法创建时返回 `None`（固定恢复路径据此
    /// 跳过失效记录，契约 11 Rev-4）。
    pub(crate) fn open_local_session(
        &mut self,
        project_dir: PathBuf,
        cwd: PathBuf,
        cx: &mut Context<Self>,
    ) -> Option<LocalSessionId> {
        let Some(view) = self.create_local_session(project_dir, cwd, cx) else {
            self.sync_local_dirs(cx);
            cx.notify();
            return None;
        };
        let ActiveView::LocalSession(session_id) = view;
        self.select_local_session(session_id, cx);
        self.refresh_git_status(session_id, false, cx);
        self.status = None;
        cx.notify();
        Some(session_id)
    }

    fn create_local_session(
        &mut self,
        project_dir: PathBuf,
        cwd: PathBuf,
        cx: &mut Context<Self>,
    ) -> Option<ActiveView> {
        let project_dir = normalize_local_cwd(project_dir)?;
        let cwd = normalize_local_cwd(cwd)?;
        self.remember_local_dir(&project_dir, cx);
        let cwd_text = cwd.to_string_lossy().to_string();
        let terminal =
            TerminalView::from_local_zed(cwd.clone(), self.terminal_settings.clone(), cx);
        let session_id = self.workspace.sessions.allocate_local_session_id();
        log::info!("local session {session_id} opened for {}", cwd_text);
        // Zed's local PTY process tracking reports the current cwd; keep it
        // separate from the session's project ownership when `cd` changes.
        let subscription =
            cx.subscribe(
                &terminal,
                |this, terminal, event: &TerminalEvent, cx| match event {
                    TerminalEvent::Closed => {
                        let session_id = this.workspace.sessions.local_sessions.iter().find_map(
                            |(&session_id, session)| {
                                (session.terminal.entity_id() == terminal.entity_id())
                                    .then_some(session_id)
                            },
                        );
                        if let Some(session_id) = session_id {
                            this.close_local_session(session_id, cx);
                        }
                    }
                    TerminalEvent::CwdChanged => {
                        this.sync_local_dirs(cx);
                        if let Some(session_id) =
                            this.local_session_id_for_terminal(terminal.entity_id())
                        {
                            this.refresh_git_status(session_id, true, cx);
                        }
                        cx.notify();
                    }
                    TerminalEvent::PromptReached => {
                        if let Some(session_id) =
                            this.local_session_id_for_terminal(terminal.entity_id())
                        {
                            this.refresh_git_status(session_id, false, cx);
                        }
                    }
                    TerminalEvent::CommandStarted { .. } => {}
                    TerminalEvent::CommandFinished { status } => {
                        log::debug!("local terminal command finished with status {status:?}");
                    }
                    TerminalEvent::ClipboardCopied => {
                        this.show_toast(
                            ToastNotice::new(i18n::text("toast.copied"), ToastTone::Success),
                            cx,
                        );
                    }
                    TerminalEvent::ClipboardPasted => {
                        this.show_toast(
                            ToastNotice::new(i18n::text("toast.pasted"), ToastTone::Success),
                            cx,
                        );
                    }
                    TerminalEvent::TitleChanged | TerminalEvent::Notification => cx.notify(),
                },
            );
        self.workspace
            .sessions
            .terminal_subscriptions
            .push(subscription);
        self.workspace.sessions.local_sessions.insert(
            session_id,
            LocalSession {
                project_dir,
                cwd,
                terminal,
                git_status: None,
                git_refresh: Default::default(),
                pin_id: None,
                custom_name: None,
                default_command: None,
            },
        );
        self.ensure_git_status_refresh_task(cx);
        self.sync_local_dirs(cx);
        Some(ActiveView::LocalSession(session_id))
    }

    pub(crate) fn activate_local_dir(&mut self, project_dir: PathBuf, cx: &mut Context<Self>) {
        self.sync_local_dirs(cx);
        let Some(project_dir) = normalize_local_cwd(project_dir) else {
            cx.notify();
            return;
        };
        let session_id = self
            .workspace
            .sessions
            .local_dirs
            .get(&project_dir)
            .and_then(|dir| dir.active_session.or_else(|| dir.sessions.first().copied()));
        if let Some(session_id) = session_id {
            self.select_local_session(session_id, cx);
        } else {
            // 有固定记录的项目：固定标签即代表该项目的工作会话集合，
            // 只恢复固定标签，不额外打开普通会话（契约 11 Rev-3）。
            let has_pinned_records =
                !pinned_tabs_for_project(&self.workspace_settings.pinned_local_tabs, &project_dir)
                    .is_empty();
            if !has_pinned_records {
                let _ = self.open_local_session(project_dir.clone(), project_dir.clone(), cx);
            }
        }
        // 激活项目时恢复该项目尚无会话的固定记录（契约 11）。
        self.restore_pinned_tabs_for_project(&project_dir, cx);
        // 契约 11 Rev-4：记录全部失效（已即时清理）导致项目仍无任何
        // 会话时，兜底打开一个普通会话，保证激活必有会话。
        let has_session_now = self
            .workspace
            .sessions
            .local_dirs
            .get(&project_dir)
            .is_some_and(|dir| !dir.sessions.is_empty());
        if !has_session_now {
            let _ = self.open_local_session(project_dir.clone(), project_dir.clone(), cx);
        }
    }

    pub(crate) fn select_local_session(
        &mut self,
        session_id: LocalSessionId,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.active_view == Some(ActiveView::LocalSession(session_id)) {
            return;
        }
        let cwd = self
            .workspace
            .sessions
            .local_dirs
            .iter()
            .find(|(_, dir)| dir.sessions.contains(&session_id))
            .map(|(cwd, _)| cwd.clone());
        let Some(cwd) = cwd else { return };
        if let Some(dir) = self.workspace.sessions.local_dirs.get_mut(&cwd) {
            dir.active_session = Some(session_id);
        }
        self.workspace.active_view = Some(ActiveView::LocalSession(session_id));
        self.refocus_active_terminal(cx);
        cx.notify();
    }

    pub(crate) fn close_local_session(
        &mut self,
        session_id: LocalSessionId,
        cx: &mut Context<Self>,
    ) {
        self.close_local_session_internal(session_id, false, cx);
    }

    pub(crate) fn close_local_session_internal(
        &mut self,
        session_id: LocalSessionId,
        keep_pinned: bool,
        cx: &mut Context<Self>,
    ) {
        // 关闭即取消固定（契约 8）：任何关闭路径（按钮/关闭其他/进程退出）
        // 都移除持久化记录，重启后不再恢复。stop 项目时 keep_pinned=true 保留固定记录。
        if !keep_pinned {
            let pin_id = self
                .workspace
                .sessions
                .local_sessions
                .get(&session_id)
                .and_then(|session| session.pin_id);
            if let Some(pin_id) = pin_id {
                self.workspace_settings
                    .pinned_local_tabs
                    .retain(|tab| tab.pin_id != pin_id);
                self.persist_settings();
            }
        }
        let Some(cwd) = self
            .workspace
            .sessions
            .local_dirs
            .iter()
            .find(|(_, dir)| dir.sessions.contains(&session_id))
            .map(|(cwd, _)| cwd.clone())
        else {
            return;
        };
        let split_affected =
            self.prepare_terminal_split_view_close(ActiveView::LocalSession(session_id), cx);
        let was_active = self.workspace.active_view == Some(ActiveView::LocalSession(session_id));
        let mut next_session = None;
        let remove_dir = if let Some(dir) = self.workspace.sessions.local_dirs.get_mut(&cwd) {
            dir.sessions.retain(|id| *id != session_id);
            if dir.active_session == Some(session_id) {
                dir.active_session = dir.sessions.first().copied();
            }
            next_session = dir.active_session;
            // 仍被「最近本地目录」记住的空目录保留在侧栏，等待下次点击重开。
            dir.sessions.is_empty() && !self.workspace_settings.recent_dirs.contains(&cwd)
        } else {
            false
        };
        self.workspace.sessions.local_sessions.remove(&session_id);
        self.workspace
            .remove_compose_for_view(ActiveView::LocalSession(session_id));
        self.git_sync.remove(&session_id);
        if self.workspace.sessions.local_sessions.is_empty() {
            self._git_status_refresh_task.take();
        }
        if remove_dir {
            self.workspace.sessions.local_dirs.remove(&cwd);
        }
        if was_active {
            self.workspace.active_view = next_session
                .map(ActiveView::LocalSession)
                .or_else(|| self.first_local_view());
            self.refocus_active_terminal(cx);
        }
        if split_affected && !was_active {
            self.refocus_active_terminal(cx);
        }
        cx.notify();
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
            self.activate_local_dir(current_local_cwd(), cx);
            return;
        }

        self.sync_local_dirs(cx);
        if let Some(cwd) = self.local_cwd_matching_query(&query_lower) {
            self.activate_local_dir(cwd, cx);
            return;
        }

        // 历史主机别名匹配已移除
        self.search_query.clear();
        self.show_toast(
            ToastNotice::new(i18n::text("toast.search_no_match"), ToastTone::Info),
            cx,
        );
    }

    /// 通过原生目录选择器创建或打开一个本地项目。
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
                this.activate_local_dir(path, cx);
            });
        });
        self._project_picker = Some(task);
    }

    /// 搜索框与 Git/Note 筛选条同一编辑语义：`TextEditingState` + 共享分发；
    /// 侧栏仅保留提交语义（Enter 打开匹配项目、Tab 保持焦点、Esc 清空），
    /// 其余按键走通用编辑分发（插入/删除/光标/选区/剪贴板），处理后截断冒泡。
    /// 唯一归属：仅侧栏搜索框的内层 `on_key_down` 注册，根节点不二次分发。
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

    /// 打开右键上下文菜单（替换已有菜单）。
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

    /// 执行外壳级菜单动作并关闭菜单。
    /// 解析并启动外部编辑器打开 `directory`；无可用编辑器或启动失败时弹错误 Toast。
    pub(crate) fn open_project_in_editor(&mut self, directory: &Path, cx: &mut Context<Self>) {
        let path_env = editor_launcher::effective_path();
        self.open_project_in_editor_with_path_env(directory, path_env, cx);
    }

    /// [`open_project_in_editor`](Self::open_project_in_editor) 的可注入 PATH 变体：
    /// PATH 作为参数传入，便于纯逻辑测试控制检测结果而不触碰进程环境。
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

    /// 把本地会话按创建时的项目归属目录重建目录视图（打开/关闭/`cd` 时调用）。
    pub(crate) fn sync_local_dirs(&mut self, cx: &Context<Self>) {
        self.prune_missing_recent_dirs();
        let previous = std::mem::take(&mut self.workspace.sessions.local_dirs);
        let active_local_session = match self.workspace.active_view {
            Some(ActiveView::LocalSession(session_id)) => Some(session_id),
            _ => None,
        };
        let sessions = self
            .workspace
            .sessions
            .local_sessions
            .iter_mut()
            .map(|(&session_id, session)| {
                if let Some(cwd) = session.terminal.read(cx).cwd.as_deref()
                    && let Some(cwd) = normalize_local_cwd(PathBuf::from(cwd))
                {
                    session.cwd = cwd;
                }
                (session_id, session.project_dir.clone())
            })
            .collect::<Vec<_>>();
        self.workspace.sessions.local_dirs = rebuild_local_dirs(
            &previous,
            sessions,
            self.workspace_settings.recent_dirs.iter().cloned(),
            active_local_session,
        );
    }

    fn local_session_id_for_terminal(&self, terminal_id: EntityId) -> Option<LocalSessionId> {
        self.workspace
            .sessions
            .local_sessions
            .iter()
            .find_map(|(&session_id, session)| {
                (session.terminal.entity_id() == terminal_id).then_some(session_id)
            })
    }

    fn refresh_git_status(
        &mut self,
        session_id: LocalSessionId,
        clear_stale: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.workspace.sessions.local_sessions.get_mut(&session_id) else {
            return;
        };
        let cwd = session.cwd.clone();
        let status_cleared = clear_stale && session.git_status.take().is_some();
        if !session.git_refresh.request() {
            if status_cleared {
                cx.notify();
            }
            return;
        }
        if status_cleared {
            session.git_status = None;
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
                let Some(session) = this.workspace.sessions.local_sessions.get_mut(&session_id)
                else {
                    return;
                };
                let cwd_unchanged = session.cwd == cwd_for_log;
                let status_changed = cwd_unchanged && session.git_status != status;
                if status_changed {
                    session.git_status = status;
                }
                let refresh_again = session.git_refresh.finish();
                this.reconcile_git_sync_error(session_id);
                if refresh_again {
                    this.refresh_git_status(session_id, !cwd_unchanged, cx);
                }
                if status_changed {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 在状态栏对本地会话执行 git push/pull；同一会话同时只允许一个在途操作。
    pub(crate) fn run_git_sync(
        &mut self,
        session_id: LocalSessionId,
        operation: GitSyncOperation,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.workspace.sessions.local_sessions.get(&session_id) else {
            return;
        };
        if self
            .git_sync
            .get(&session_id)
            .is_some_and(|state| state.running)
        {
            return;
        }
        let cwd = session.cwd.clone();
        self.git_sync.insert(
            session_id,
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
                        // 成功后清除状态，按钮是否保留由 ahead/behind 徽章决定。
                        this.git_sync.remove(&session_id);
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
                        let Some(state) = this.git_sync.get_mut(&session_id) else {
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
                this.refresh_git_status(session_id, false, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// 同步错误只在触发它的条件（behind/ahead 计数）消失时清除，避免残留过期错误。
    fn reconcile_git_sync_error(&mut self, session_id: LocalSessionId) {
        let Some(state) = self.git_sync.get(&session_id) else {
            return;
        };
        if state.error.is_none() {
            return;
        }
        let Some(status) = self
            .workspace
            .sessions
            .local_sessions
            .get(&session_id)
            .and_then(|session| session.git_status.as_ref())
        else {
            return;
        };
        let resolved = match state.operation {
            GitSyncOperation::Pull => status.behind == 0,
            GitSyncOperation::Push => status.ahead == 0,
        };
        if resolved {
            self.git_sync.remove(&session_id);
        }
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
                if tick.is_multiple_of(60) {
                    log::info!("git status refresh loop alive (tick {tick})");
                }

                if weak
                    .update(cx, |this, cx| {
                        if let Some(ActiveView::LocalSession(session_id)) =
                            this.workspace.focused_view()
                        {
                            this.refresh_git_status(session_id, false, cx);
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
                            this.system_monitor
                                .apply_snapshot(snapshot, expected_generation);
                        }
                        cx.notify();
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

    /// 把目录记入「最近本地目录」历史（最近优先、去重、截断到上限）并持久化。
    fn remember_local_dir(&mut self, project_dir: &Path, _cx: &mut Context<Self>) {
        let Some(project_dir) = normalize_local_cwd(project_dir.to_path_buf()) else {
            return;
        };
        self.workspace_settings
            .recent_dirs
            .retain(|existing| existing != &project_dir);
        self.workspace_settings.recent_dirs.insert(0, project_dir);
        self.workspace_settings
            .recent_dirs
            .truncate(self.workspace_settings.recent_dirs_max);
        self.persist_settings();
    }

    /// 从「最近本地目录」历史中移除一个目录并持久化。
    pub(crate) fn forget_local_dir(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        let Some(cwd) = normalize_local_cwd(cwd) else {
            return;
        };
        if !self.workspace_settings.recent_dirs.contains(&cwd) {
            return;
        }
        self.workspace_settings
            .recent_dirs
            .retain(|existing| existing != &cwd);
        self.persist_settings();
        self.sync_local_dirs(cx);
        cx.notify();
    }

    fn prune_missing_recent_dirs(&mut self) {
        let recent_dirs = normalize_recent_dirs(self.workspace_settings.recent_dirs.clone());
        if recent_dirs == self.workspace_settings.recent_dirs {
            return;
        }
        self.workspace_settings.recent_dirs = recent_dirs;
        self.persist_settings();
    }

    /// 清空「最近本地目录」历史。
    pub(crate) fn clear_recent_dirs(&mut self, cx: &mut Context<Self>) {
        if self.workspace_settings.recent_dirs.is_empty() {
            return;
        }
        self.workspace_settings.recent_dirs.clear();
        self.persist_settings();
        self.sync_local_dirs(cx);
        cx.notify();
    }

    /// 只写设置全局状态与磁盘，不重放终端设置（区别于 apply_settings）。
    fn persist_settings(&self) {
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

    /// Finish the normal shutdown sequence after a verified update has been
    /// handed to the standalone updater.
    pub(crate) fn quit_for_update(&mut self, cx: &mut Context<Self>) {
        if self.shutdown_in_progress {
            return;
        }
        self.begin_shutdown(cx);
        cx.quit();
    }

    pub(crate) fn local_dir_for_session(&self, session_id: LocalSessionId) -> Option<&LocalDir> {
        self.workspace
            .sessions
            .local_dirs
            .iter()
            .find_map(|(_, dir)| dir.sessions.contains(&session_id).then_some(dir))
    }

    fn local_cwd_matching_query(&self, query: &str) -> Option<PathBuf> {
        self.workspace
            .sessions
            .local_dirs
            .keys()
            .find(|cwd| cwd.to_string_lossy().to_ascii_lowercase().contains(query))
            .cloned()
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
            // Wayland: xdg_toplevel app_id 需与 .desktop 的 StartupWMClass / Icon 匹配，
            // 否则 GNOME/KDE dock 无法关联图标（退化为灰色齿轮）。保持与
            // main.rs:cx.set_app_identity("me.xcrong.crossh") 一致。
            app_id: Some("me.xcrong.crossh".into()),
            // X11: _NET_WM_ICON 需显式提供 RgbaImage；Wayland 忽略此字段，图标由
            // .desktop + hicolor 主题提供。此处暂不嵌入光栅图，避免额外依赖；
            // 若需 X11 托盘图标，可在此加入 `icon: load_window_icon()`。
            ..Default::default()
        },
        |window, cx| {
            let shell = AppShell::new(cx);
            let weak = shell.downgrade();
            let notification_weak = shell.downgrade();
            let window_handle = window.window_handle();
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
            window.on_window_should_close(cx, move |window, cx| {
                weak.update(cx, |shell, cx| shell.should_close_window(window, cx))
                    .unwrap_or(true)
            });
            shell
        },
    )
    .expect("Failed to open window");
    cx.activate(true);
}

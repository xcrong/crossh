//! 应用外壳：左侧主机列表 + 顶部标签条 + 终端工作区 + 模态弹窗。
//!
//! - 连接池：同主机复用一条已认证会话（开新终端 channel），全部终端关闭才断开。
//! - 多标签：侧栏点击主机优先切换已有终端，显式新建时才追加标签；可切换/关闭。
//! - sidebar：Local、Active、Bank 为同级可折叠组。
//! - 模态：池中任一连接出现 pending_prompt（未知主机密钥/凭据）时弹覆盖层。
//!
//! 渲染与交互拆分为兄弟模块：`sidebar`（侧栏）、`workspace`（标签条+主区）、
//! `settings`（设置页）、`prompt`（模态弹窗）。本模块只保留状态与行为。

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, AppContext, Context, Entity, EntityId, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, PathPromptOptions, Pixels, Point, Render, Styled, Task,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, px, size,
};

use crate::features::connections::{Connection, ConnectionManager, HostEntry, PendingPrompt};
use crate::features::connections::{PromptDisplay, render_prompt_modal};
use crate::features::forwarding::ForwardPane;
use crate::features::settings::{self, SettingsSnapshot};
use crate::features::sftp::SftpPane;
use crate::features::terminal::{TerminalEvent, TerminalView, TerminalViewEvent};
use crate::features::updates::{UpdateController, UpdateSettings};
use crate::features::workspace::empty_state::EmptyStateFilter;
use crate::features::workspace::pinned::{pinned_tabs_for_project, prune_missing_pinned_tabs};
use crate::features::workspace::quick_commands_rail::render_quick_commands_rail;
use crate::features::workspace::registry::WorkspaceState;
use crate::features::workspace::rename_editor::RenameEditor;
use crate::features::workspace::settings::WorkspaceSettings;
use crate::features::workspace::sidebar::{render_sidebar, render_sidebar_rail};
use crate::features::workspace::toaster::{ToastNotice, ToastTone};
use crate::features::workspace::view::{
    ActiveView, LocalDir, LocalSession, LocalSessionId, Tab, rebuild_local_dirs, render_main,
    render_quick_command_editor, render_quick_commands, render_rename_editor,
    render_workspace_status_bar,
};
use crate::shared::i18n::{self, LanguagePreference};
use crossh_agent::AgentSettings;
use crossh_core::commands::{
    BackgroundTaskEvent, BackgroundTaskManager, BackgroundTaskStatus, CommandHistory, local_scope,
    remote_scope,
};
use crossh_core::config::{HostConfig, SshConfig};
use crossh_core::git::{pull, push};
use crossh_core::git_status::inspect;
use crossh_ssh::{HostKeyDecision, RemoteCommandStatus};
use crossh_terminal::settings::{
    MAX_FONT_SIZE, MAX_SCROLLBACK, MIN_FONT_SIZE, MIN_SCROLLBACK, TerminalSettings,
};
use crossh_ui::context_menu::{ContextMenuState, MenuEntry, ShellMenuAction, render_context_menu};
use crossh_ui::theme;
use crossh_ui::widgets::printable_char;

use super::command_editor::QuickCommandEditor;
use super::local_paths::{current_local_cwd, normalize_local_cwd, normalize_recent_dirs};
#[cfg(test)]
use crate::shared::text_editing::{next_char_boundary, previous_char_boundary, selection_bounds};

#[path = "notifications.rs"]
mod notifications;
#[path = "quit.rs"]
mod quit;
#[path = "shell_input.rs"]
mod shell_input;
#[path = "split.rs"]
mod split;
#[path = "tabs.rs"]
mod tabs;

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

struct ActiveCommandContext {
    scope: String,
    cwd: String,
    owner: String,
    connection: Option<Entity<Connection>>,
    remote_target: Option<String>,
    remote_tab: Option<usize>,
    local_session: Option<LocalSessionId>,
}

struct PendingBackgroundRestart {
    task: crossh_core::commands::BackgroundTask,
    connection: Option<Entity<Connection>>,
}

fn local_background_owner(session_id: LocalSessionId) -> String {
    format!("local-session:{session_id}")
}

fn remote_background_owner(terminal_id: EntityId) -> String {
    format!("remote-terminal:{terminal_id}")
}

fn remote_tab_background_owner(tab: &Tab) -> Option<String> {
    tab.pane.terminal_entity_id().map(remote_background_owner)
}

pub struct AppShell {
    pub(crate) connections: ConnectionManager,
    pub(crate) workspace: WorkspaceState,
    pub(crate) status: Option<String>,
    /// 侧栏搜索文本；未命中配置别名时也作为 QuickConnect 目标。
    pub(crate) host_query: String,
    pub(crate) host_ime_marked_text: String,
    pub(crate) host_focus: FocusHandle,
    /// 应用外壳根焦点；无任何终端/输入框聚焦时持有，保证窗口级动作
    /// （如 Cmd+Q → Quit）始终有合法的 dispatch 目标。
    pub(crate) shell_focus: FocusHandle,
    /// 主机分组折叠状态；Bank 默认收起，Local/Active 默认展开。
    pub(crate) bank_collapsed: bool,
    pub(crate) active_collapsed: bool,
    pub(crate) projects_collapsed: bool,
    pub(crate) empty_state_filter: EmptyStateFilter,
    /// 原生项目目录选择器任务，持有到选择结果返回。
    _project_picker: Option<Task<()>>,
    /// 模态文本输入缓冲（密码/口令）。
    pub(crate) prompt_input: String,
    pub(crate) prompt_ime_marked_text: String,
    /// 模态输入框焦点。
    pub(crate) modal_focus: FocusHandle,
    /// 上一帧是否有活动模态（用于在弹窗出现时自动聚焦）。
    last_had_prompt: bool,
    /// 当前语言偏好；实际 locale 由 i18n 全局状态维护。
    pub(crate) language_preference: LanguagePreference,
    /// 当前打开的右键上下文菜单（None = 未打开）。
    pub(crate) context_menu: Option<ContextMenuState<ShellMenuAction>>,
    pub(crate) terminal_settings: TerminalSettings,
    pub(crate) update_settings: UpdateSettings,
    pub(crate) updates: Entity<UpdateController>,
    pub(crate) workspace_settings: WorkspaceSettings,
    pub(crate) agent_settings: AgentSettings,
    /// 侧栏宽度与拖动状态；只影响布局，不改变导航状态。
    pub(crate) sidebar_width: Rc<Cell<f32>>,
    pub(crate) sidebar_dragging: Rc<Cell<bool>>,
    pub(crate) sidebar_scroll: gpui::ScrollHandle,
    pub(crate) tab_scroll: gpui::ScrollHandle,
    /// Right-side command panel width and drag state.
    pub(crate) quick_commands_width: Rc<Cell<f32>>,
    pub(crate) quick_commands_dragging: Rc<Cell<bool>>,
    /// Left terminal split width and drag state; zero means use an even split
    /// for the current available width.
    pub(crate) terminal_split_width: Rc<Cell<f32>>,
    pub(crate) terminal_split_dragging: Rc<Cell<bool>>,
    pub(crate) command_history: CommandHistory,
    pub(crate) background_tasks: BackgroundTaskManager,
    remote_background_controls: BTreeMap<u64, (Entity<Connection>, u64)>,
    pending_background_restarts: BTreeMap<u64, PendingBackgroundRestart>,
    pub(crate) quick_command_editor: Option<QuickCommandEditor>,
    /// 固定标签重命名弹窗状态；与 quick command 编辑器互斥（都是模态弹窗）。
    pub(crate) rename_editor: Option<RenameEditor>,
    /// 周期性刷新本地会话的 Git 状态，覆盖 shell 空闲时的外部文件变更。
    _git_status_refresh_task: Option<Task<()>>,
    /// 最近一次状态栏 Git 同步操作的进行/错误状态，按会话独立记录。
    pub(crate) git_sync: BTreeMap<LocalSessionId, GitSyncState>,
    quit_confirmation_open: bool,
    shutdown_in_progress: bool,
    /// 标签页关闭确认框是否已打开，防止重复弹出。
    pub(crate) tab_close_confirmation_open: bool,
}

impl AppShell {
    /// 从 ~/.ssh/config 加载并构造外壳。
    pub fn new(cx: &mut App) -> Entity<Self> {
        let config = match SshConfig::from_default_location() {
            Ok(c) => c,
            Err(e) => {
                log::warn!("failed to read ~/.ssh/config: {e}");
                SshConfig::default()
            }
        };
        let config = Arc::new(config);
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
        let agent_settings = snapshot.agent;
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
            connections: ConnectionManager::new(config),
            workspace: WorkspaceState::new(local_dirs),
            status: None,
            host_query: String::new(),
            host_ime_marked_text: String::new(),
            host_focus: cx.focus_handle(),
            shell_focus: cx.focus_handle(),
            bank_collapsed: true,
            active_collapsed: false,
            projects_collapsed: false,
            empty_state_filter: EmptyStateFilter::default(),
            _project_picker: None,
            prompt_input: String::new(),
            prompt_ime_marked_text: String::new(),
            modal_focus: cx.focus_handle(),
            last_had_prompt: false,
            language_preference,
            context_menu: None,
            terminal_settings,
            update_settings,
            updates: updates.clone(),
            workspace_settings,
            agent_settings,
            sidebar_width: Rc::new(Cell::new(theme::SIDEBAR_WIDTH)),
            sidebar_dragging: Rc::new(Cell::new(false)),
            sidebar_scroll: gpui::ScrollHandle::new(),
            tab_scroll: gpui::ScrollHandle::new(),
            quick_commands_width: Rc::new(Cell::new(theme::QUICK_COMMANDS_WIDTH)),
            quick_commands_dragging: Rc::new(Cell::new(false)),
            terminal_split_width: Rc::new(Cell::new(0.)),
            terminal_split_dragging: Rc::new(Cell::new(false)),
            command_history: CommandHistory::load(),
            background_tasks: BackgroundTaskManager::default(),
            remote_background_controls: BTreeMap::new(),
            pending_background_restarts: BTreeMap::new(),
            quick_command_editor: None,
            rename_editor: None,
            _git_status_refresh_task: None,
            git_sync: BTreeMap::new(),
            quit_confirmation_open: false,
            shutdown_in_progress: false,
            tab_close_confirmation_open: false,
        });
        updates.update(cx, |updates, cx| updates.start_startup_check(cx));
        shell
    }

    pub(crate) fn open_host(&mut self, idx: usize, cx: &mut Context<Self>) {
        let entry = match self.connections.entries().get(idx) {
            Some(e) => e.clone(),
            None => return,
        };

        // The sidebar is navigation. Reuse the existing terminal for a live
        // connection instead of opening another channel when returning from a
        // local session.
        if let Some(tab_idx) = self.remote_terminal_to_switch(&entry.key) {
            self.switch_remote_tab(tab_idx, cx);
            return;
        }

        self.open_terminal_target(entry.alias, cx);
    }

    fn remote_terminal_to_switch(&self, host_key: &str) -> Option<usize> {
        find_remote_terminal_index(
            self.workspace
                .sessions
                .remote_tabs
                .iter()
                .enumerate()
                .map(|(idx, tab)| {
                    (
                        idx,
                        tab.host_key.as_str(),
                        tab.pane.terminal_entity_id().is_some(),
                    )
                }),
            host_key,
        )
    }

    pub(crate) fn open_sftp(&mut self, idx: usize, cx: &mut Context<Self>) {
        let entry = match self.connections.entries().get(idx) {
            Some(e) => e.clone(),
            None => return,
        };
        let resolved = self.connections.resolve(&entry.alias);
        let methods = self.connections.auth_methods(&resolved);
        let host_key = ConnectionManager::pool_key(&resolved);
        let conn = self.connections.acquire(resolved.clone(), methods, cx);
        let (cmd_tx, event_rx) = conn.read(cx).open_sftp();
        let pane = SftpPane::from_bridge(cmd_tx, event_rx, cx);
        self.workspace.sessions.remote_tabs.push(Tab {
            target: entry.alias.clone(),
            host_key,
            connection: Some(conn.clone()),
            pane: crate::features::sftp::view::workspace_pane(pane),
        });
        self.workspace.active_view = Some(ActiveView::RemoteTab(
            self.workspace.sessions.remote_tabs.len() - 1,
        ));
        cx.notify();
    }

    pub(crate) fn open_forward(&mut self, idx: usize, cx: &mut Context<Self>) {
        let entry = match self.connections.entries().get(idx) {
            Some(e) => e.clone(),
            None => return,
        };
        let resolved = self.connections.resolve(&entry.alias);
        let methods = self.connections.auth_methods(&resolved);
        let host_key = ConnectionManager::pool_key(&resolved);
        let conn = self.connections.acquire(resolved.clone(), methods, cx);
        let pane = ForwardPane::new(conn.clone(), cx, &resolved);
        self.workspace.sessions.remote_tabs.push(Tab {
            target: entry.alias.clone(),
            host_key,
            connection: Some(conn),
            pane: crate::features::forwarding::view::workspace_pane(pane),
        });
        self.workspace.active_view = Some(ActiveView::RemoteTab(
            self.workspace.sessions.remote_tabs.len() - 1,
        ));
        cx.notify();
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
        if let ActiveView::LocalSession(session_id) = view {
            self.select_local_session(session_id, cx);
            self.refresh_git_status(session_id, false, cx);
            self.status = None;
            cx.notify();
            return Some(session_id);
        }
        None
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
        let subscription = cx.subscribe(&terminal, |this, terminal, event, cx| match event {
            TerminalEvent::Closed => {
                let session_id = this.workspace.sessions.local_sessions.iter().find_map(
                    |(&session_id, session)| {
                        (session.terminal.entity_id() == terminal.entity_id()).then_some(session_id)
                    },
                );
                if let Some(session_id) = session_id {
                    this.close_local_session(session_id, cx);
                }
            }
            TerminalEvent::CwdChanged => {
                this.sync_local_dirs(cx);
                if let Some(session_id) = this.local_session_id_for_terminal(terminal.entity_id()) {
                    this.refresh_git_status(session_id, true, cx);
                }
                cx.notify();
            }
            TerminalEvent::PromptReached => {
                if let Some(session_id) = this.local_session_id_for_terminal(terminal.entity_id()) {
                    this.refresh_git_status(session_id, false, cx);
                }
            }
            TerminalEvent::CommandStarted { command, cwd } => {
                if let Some(cwd) = cwd
                    && let Some(cwd) = normalize_local_cwd(PathBuf::from(cwd))
                {
                    this.record_command(local_scope(&cwd), command.clone(), cx);
                }
            }
            TerminalEvent::CommandFinished { status } => {
                log::debug!("local terminal command finished with status {status:?}");
            }
            TerminalEvent::TitleChanged | TerminalEvent::Notification => cx.notify(),
        });
        let adjacent_subscription =
            cx.subscribe(&terminal, |this, terminal, event, cx| match event {
                TerminalViewEvent::SendSelectionToAdjacent { text } => {
                    this.send_to_adjacent_terminal(terminal.entity_id(), text, cx);
                }
            });
        self.workspace
            .sessions
            .terminal_subscriptions
            .push(subscription);
        self.workspace
            .sessions
            .terminal_subscriptions
            .push(adjacent_subscription);
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
        let owner = local_background_owner(session_id);
        self.stop_background_tasks_for_owner(&owner, cx);
        // 关闭即取消固定（契约 8）：任何关闭路径（按钮/关闭其他/进程退出）
        // 都移除持久化记录，重启后不再恢复。
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
                .or_else(|| self.first_local_view())
                .or_else(|| {
                    self.workspace.sessions.remote_tabs.last().map(|_| {
                        ActiveView::RemoteTab(self.workspace.sessions.remote_tabs.len() - 1)
                    })
                });
            self.refocus_active_terminal(cx);
        }
        if split_affected && !was_active {
            self.refocus_active_terminal(cx);
        }
        cx.notify();
    }

    fn record_command(&mut self, scope: String, command: String, cx: &mut Context<Self>) {
        if self.command_history.record(&scope, &command) {
            cx.notify();
        }
    }

    pub(crate) fn toggle_quick_command_pin(
        &mut self,
        scope: String,
        command: String,
        cx: &mut Context<Self>,
    ) {
        if self.command_history.toggle_pinned(&scope, &command) {
            cx.notify();
        }
    }

    fn active_command_context(&self, cx: &Context<Self>) -> Option<ActiveCommandContext> {
        match self.workspace.focused_view()? {
            ActiveView::LocalSession(session_id) => {
                let session = self.workspace.sessions.local_sessions.get(&session_id)?;
                let cwd = session
                    .terminal
                    .read(cx)
                    .cwd
                    .clone()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| session.cwd.clone());
                let cwd = normalize_local_cwd(cwd)?;
                Some(ActiveCommandContext {
                    scope: local_scope(&cwd),
                    cwd: cwd.to_string_lossy().to_string(),
                    owner: local_background_owner(session_id),
                    connection: None,
                    remote_target: None,
                    remote_tab: None,
                    local_session: Some(session_id),
                })
            }
            ActiveView::RemoteTab(index) => {
                let tab = self.workspace.sessions.remote_tabs.get(index)?;
                let entity_id = tab.pane.terminal_entity_id()?;
                let owner = remote_background_owner(entity_id);
                let cwd = tab.pane.cwd(cx)?;
                Some(ActiveCommandContext {
                    scope: remote_scope(&tab.host_key, &cwd),
                    cwd,
                    owner,
                    connection: tab.connection.clone(),
                    remote_target: Some(tab.target.clone()),
                    remote_tab: Some(index),
                    local_session: None,
                })
            }
        }
    }

    pub(crate) fn run_quick_command(
        &mut self,
        scope: String,
        command: String,
        background: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(context) = self.active_command_context(cx) else {
            return;
        };
        if context.scope != scope {
            return;
        }
        if background {
            let owner = context.owner.clone();
            if let Some(connection) = context.connection {
                self.start_remote_background(connection, owner, scope, context.cwd, command, cx);
            } else if let Some(target) = context.remote_target {
                let resolved = self.connections.resolve(&target);
                let methods = self.connections.auth_methods(&resolved);
                let connection = self.connections.acquire(resolved, methods, cx);
                self.start_remote_background(connection, owner, scope, context.cwd, command, cx);
            } else {
                let Some(cwd) = normalize_local_cwd(PathBuf::from(context.cwd)) else {
                    return;
                };
                let (id, event_rx) = self.background_tasks.start(scope, cwd, command, owner);
                cx.spawn(async move |weak, cx| {
                    let Ok(event) = event_rx.recv().await else {
                        return;
                    };
                    let _ = weak.update(cx, |this, cx| {
                        this.apply_background_event(event, cx);
                        cx.notify();
                    });
                })
                .detach();
                log::info!("started background command {id}");
            }
        } else {
            if let Some(index) = context.remote_tab {
                if let Some(tab) = self.workspace.sessions.remote_tabs.get(index) {
                    tab.pane.run_command(&command, cx);
                }
            } else if let Some(session_id) = context.local_session
                && let Some(session) = self.workspace.sessions.local_sessions.get(&session_id)
            {
                session.terminal.update(cx, |terminal, terminal_cx| {
                    terminal.run_command(&command, terminal_cx)
                });
            }
        }
        cx.notify();
    }

    fn start_remote_background(
        &mut self,
        connection: Entity<Connection>,
        owner: String,
        scope: String,
        cwd: String,
        command: String,
        cx: &mut Context<Self>,
    ) {
        let (remote_id, event_rx) = connection.update(cx, |connection, _cx| {
            connection.open_command(command.clone(), cwd.clone())
        });
        let task_id = self
            .background_tasks
            .start_remote(scope, PathBuf::from(cwd), command, owner);
        self.remote_background_controls
            .insert(task_id, (connection, remote_id));
        let expected_remote_id = remote_id;
        cx.spawn(async move |weak, cx| {
            let event = match event_rx.recv().await {
                Ok(event) => {
                    debug_assert_eq!(event.id, expected_remote_id);
                    BackgroundTaskEvent {
                        id: task_id,
                        status: match event.status {
                            RemoteCommandStatus::Succeeded => BackgroundTaskStatus::Succeeded,
                            RemoteCommandStatus::Failed => BackgroundTaskStatus::Failed,
                            RemoteCommandStatus::Terminated => BackgroundTaskStatus::Terminated,
                        },
                        output: event.output,
                        exit_code: event.exit_code,
                    }
                }
                Err(_) => BackgroundTaskEvent {
                    id: task_id,
                    status: BackgroundTaskStatus::Failed,
                    output: "SSH connection closed".into(),
                    exit_code: None,
                },
            };
            let _ = weak.update(cx, |this, cx| {
                this.apply_background_event(event, cx);
                cx.notify();
            });
        })
        .detach();
        log::info!("started remote background command {task_id}");
    }

    fn apply_background_event(&mut self, event: BackgroundTaskEvent, cx: &mut Context<Self>) {
        let event_id = event.id;
        log::info!(
            "background command {} finished as {:?}",
            event.id,
            event.status
        );
        self.remote_background_controls.remove(&event_id);
        self.background_tasks.apply_event(event);
        if let Some(restart) = self.pending_background_restarts.remove(&event_id) {
            self.start_background_restart(restart, cx);
        }
    }

    pub(crate) fn stop_background_task(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some((connection, remote_id)) = self.remote_background_controls.get(&id).cloned() {
            connection.read(cx).stop_command(remote_id);
        }
        self.background_tasks.mark_stopping(id);
        cx.notify();
    }

    pub(crate) fn restart_background_task(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(task) = self.background_tasks.tasks.get(&id).cloned() else {
            return;
        };
        if task.status != BackgroundTaskStatus::Running {
            return;
        }
        let connection = self
            .remote_background_controls
            .get(&id)
            .map(|(connection, _)| connection.clone());
        self.pending_background_restarts
            .insert(id, PendingBackgroundRestart { task, connection });
        self.stop_background_task(id, cx);
    }

    fn start_background_restart(
        &mut self,
        restart: PendingBackgroundRestart,
        cx: &mut Context<Self>,
    ) {
        let task = restart.task;
        if let Some(connection) = restart.connection {
            self.start_remote_background(
                connection,
                task.owner,
                task.scope,
                task.cwd.to_string_lossy().into_owned(),
                task.command,
                cx,
            );
        } else {
            let (id, event_rx) =
                self.background_tasks
                    .start(task.scope, task.cwd, task.command, task.owner);
            cx.spawn(async move |weak, cx| {
                let Ok(event) = event_rx.recv().await else {
                    return;
                };
                let _ = weak.update(cx, |this, cx| {
                    this.apply_background_event(event, cx);
                    cx.notify();
                });
            })
            .detach();
            log::info!("restarted background command {id}");
        }
    }

    fn stop_background_tasks_for_owner(&mut self, owner: &str, cx: &mut Context<Self>) {
        let ids = self.background_tasks.active_for_owner(owner);
        for id in ids {
            self.pending_background_restarts.remove(&id);
            self.stop_background_task(id, cx);
        }
    }

    fn open_query(&mut self, cx: &mut Context<Self>) {
        let query = self.host_query.trim().to_string();
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

        let matching_idx = self
            .connections
            .entries()
            .iter()
            .position(|entry| entry.alias.eq_ignore_ascii_case(&query))
            .or_else(|| {
                self.connections
                    .entries()
                    .iter()
                    .position(|entry| host_entry_matches(entry, &query_lower))
            });

        if let Some(idx) = matching_idx {
            self.open_host(idx, cx);
        } else {
            self.open_terminal_target(query, cx);
        }
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
                this.host_query.clear();
                this.host_ime_marked_text.clear();
                this.activate_local_dir(path, cx);
            });
        });
        self._project_picker = Some(task);
    }

    pub(crate) fn handle_host_search_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev.keystroke.key.as_str() {
            "enter" | "return" => self.open_query(cx),
            "escape" => {
                self.host_query.clear();
                self.host_ime_marked_text.clear();
                cx.notify();
            }
            "backspace" => {
                self.host_query.pop();
                self.host_ime_marked_text.clear();
                cx.notify();
            }
            _ => {
                if let Some(ch) = printable_char(&ev.keystroke) {
                    self.host_query.push(ch);
                    self.host_ime_marked_text.clear();
                    cx.notify();
                } else if ev.keystroke.key == "tab" {
                    self.host_focus.focus(window, cx);
                }
            }
        }
    }

    fn handle_shell_key_down(
        &mut self,
        ev: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 菜单打开时只响应 Escape（其余键被菜单模态拦截）。
        if self.context_menu.is_some() {
            if ev.keystroke.key == "escape" {
                self.close_context_menu(cx);
            }
            return;
        }
        if !matches!(self.current_prompt(cx), PromptDisplay::None) {
            return;
        }
        if self.quick_command_editor.is_some() {
            return;
        }
        let ks = &ev.keystroke;
        let primary = ks.modifiers.platform || ks.modifiers.control;
        if !primary {
            return;
        }

        match ks.key.as_str() {
            "tab" => self.cycle_tab(if ks.modifiers.shift { -1 } else { 1 }, cx),
            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" => {
                if let Ok(n) = ks.key.parse::<usize>() {
                    self.switch_tab(n - 1, cx);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn toggle_bank_group(&mut self, cx: &mut Context<Self>) {
        self.bank_collapsed = !self.bank_collapsed;
        cx.notify();
    }

    pub(crate) fn toggle_active_group(&mut self, cx: &mut Context<Self>) {
        self.active_collapsed = !self.active_collapsed;
        cx.notify();
    }

    pub(crate) fn toggle_projects_group(&mut self, cx: &mut Context<Self>) {
        self.projects_collapsed = !self.projects_collapsed;
        cx.notify();
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
    fn dispatch_shell_menu_action(
        &mut self,
        action: ShellMenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            ShellMenuAction::ChooseLocalProject => self.choose_project_directory(cx),
            ShellMenuAction::ActivateLocalProject(path) => self.activate_local_dir(path, cx),
            ShellMenuAction::OpenHost(idx) => self.open_host(idx, cx),
            ShellMenuAction::OpenSftp(idx) => self.open_sftp(idx, cx),
            ShellMenuAction::OpenForward(idx) => self.open_forward(idx, cx),
            ShellMenuAction::CopyText(text) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
            }
            ShellMenuAction::RevealInFinder(path) => {
                crossh_core::process::reveal_in_finder(&path);
            }
            ShellMenuAction::ForgetLocalDir(cwd) => self.forget_local_dir(cwd, cx),
            ShellMenuAction::OpenLocalTerminal(cwd) => {
                let _ = self.open_local_session(cwd.clone(), cwd.clone(), cx);
                // 「打开本地终端」即打开项目：同步恢复该项目尚无会话的
                // 固定记录（契约 11 Rev-1）。不能放进 open_local_session
                // 自身，否则恢复路径会递归（恢复内部也调用 open）。
                self.restore_pinned_tabs_for_project(&cwd, cx);
            }
            ShellMenuAction::SelectRemoteTab(idx) => self.switch_remote_tab(idx, cx),
            ShellMenuAction::ToggleLowLatencyShellInput(idx) => {
                self.toggle_low_latency_shell_input(idx, cx)
            }
            ShellMenuAction::CloseRemoteTab(idx) => self.request_close_remote_tab(idx, window, cx),
            ShellMenuAction::CloseOtherRemoteTabs(idx) => self.close_other_remote_tabs(idx, cx),
            ShellMenuAction::CloseAllRemoteTabs => self.close_all_remote_tabs(cx),
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
            ShellMenuAction::CloseLocalSession(session_id) => {
                self.request_close_local_session(session_id, window, cx);
            }
            ShellMenuAction::CloseOtherLocalSessions(session_id) => {
                self.close_other_local_sessions(session_id, cx);
            }
            ShellMenuAction::RunQuickCommand {
                scope,
                command,
                background,
            } => self.run_quick_command(scope, command, background, cx),
            ShellMenuAction::EditQuickCommand { scope, command } => {
                self.open_quick_command_editor(scope, command, window, cx)
            }
            ShellMenuAction::ToggleQuickCommandPin { scope, command } => {
                self.toggle_quick_command_pin(scope, command, cx)
            }
            ShellMenuAction::DeleteQuickCommand { scope, command } => {
                self.command_history.remove(&scope, &command);
                cx.notify();
            }
            ShellMenuAction::IgnoreQuickCommand { scope, command } => {
                self.command_history.ignore(&scope, &command);
                cx.notify();
            }
            ShellMenuAction::StopBackgroundTask(id) => self.stop_background_task(id, cx),
            ShellMenuAction::RestartBackgroundTask(id) => self.restart_background_task(id, cx),
        }
        self.close_context_menu(cx);
    }

    fn toggle_low_latency_shell_input(&mut self, idx: usize, cx: &mut Context<Self>) {
        let terminal = self
            .workspace
            .sessions
            .remote_tabs
            .get(idx)
            .map(|tab| &tab.pane);
        if let Some(pane) = terminal {
            pane.toggle_low_latency(cx);
            cx.notify();
        }
    }

    pub(crate) fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        crate::features::settings::toggle_settings(cx.weak_entity(), cx);
        cx.notify();
    }

    pub(crate) fn set_language(&mut self, preference: LanguagePreference, cx: &mut Context<Self>) {
        if self.language_preference == preference {
            cx.notify();
            return;
        }
        i18n::set_language(cx, preference);
        crate::infrastructure::app_menu::refresh(cx);
        self.language_preference = preference;
        for tab in &self.workspace.sessions.remote_tabs {
            tab.pane.notify_language(cx);
        }
        self.persist_settings();
        cx.notify();
    }

    pub(crate) fn toggle_timestamps(&mut self, cx: &mut Context<Self>) {
        let mut terminal = self.terminal_settings.clone();
        terminal.show_timestamps = !terminal.show_timestamps;
        self.apply_terminal_settings(terminal, cx);
    }

    pub(crate) fn toggle_host_sidebar(&mut self, cx: &mut Context<Self>) {
        self.workspace_settings.show_host_sidebar = !self.workspace_settings.show_host_sidebar;
        self.persist_settings();
        cx.notify();
    }

    pub(crate) fn set_empty_state_filter(
        &mut self,
        filter: EmptyStateFilter,
        cx: &mut Context<Self>,
    ) {
        if self.empty_state_filter != filter {
            self.empty_state_filter = filter;
            cx.notify();
        }
    }

    pub(crate) fn toggle_quick_commands(&mut self, cx: &mut Context<Self>) {
        self.workspace_settings.show_quick_commands = !self.workspace_settings.show_quick_commands;
        self.persist_settings();
        cx.notify();
    }

    pub(crate) fn toggle_terminal_notifications(&mut self, cx: &mut Context<Self>) {
        let mut terminal = self.terminal_settings.clone();
        terminal.notifications_enabled = !terminal.notifications_enabled;
        self.apply_terminal_settings(terminal, cx);
    }

    pub(crate) fn set_update_check_on_startup(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.update_settings.check_on_startup == enabled {
            return;
        }
        self.update_settings.check_on_startup = enabled;
        self.persist_settings();
        cx.notify();
    }

    pub(crate) fn set_agent_settings(&mut self, settings: AgentSettings, cx: &mut Context<Self>) {
        let settings = settings.normalized();
        if settings.validate().is_err() || self.agent_settings == settings {
            return;
        }
        self.agent_settings = settings;
        self.persist_settings();
        cx.notify();
    }

    pub(crate) fn adjust_font_size(&mut self, delta: f32, cx: &mut Context<Self>) {
        let mut terminal = self.terminal_settings.clone();
        terminal.font_size = (terminal.font_size + delta)
            .round()
            .clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        self.apply_terminal_settings(terminal, cx);
    }

    pub(crate) fn set_scrollback(&mut self, scrollback: usize, cx: &mut Context<Self>) {
        let mut terminal = self.terminal_settings.clone();
        terminal.scrollback = scrollback.clamp(MIN_SCROLLBACK, MAX_SCROLLBACK);
        self.apply_terminal_settings(terminal, cx);
    }

    pub(crate) fn set_recent_dirs_max(&mut self, max: usize, cx: &mut Context<Self>) {
        let mut workspace = self.workspace_settings.clone();
        workspace.recent_dirs_max = max;
        self.workspace_settings = workspace.normalized();
        self.persist_settings();
        self.sync_local_dirs(cx);
        cx.notify();
    }

    fn apply_terminal_settings(&mut self, settings: TerminalSettings, cx: &mut Context<Self>) {
        let settings = settings.normalized();
        if self.terminal_settings == settings {
            return;
        }

        TerminalView::apply_zed_settings(&settings, cx);

        for tab in &self.workspace.sessions.remote_tabs {
            tab.pane.apply_terminal_settings(settings.clone(), cx);
        }
        for session in self.workspace.sessions.local_sessions.values() {
            session.terminal.update(cx, |terminal, cx| {
                terminal.apply_settings(settings.clone(), cx)
            });
        }

        self.terminal_settings = settings;
        self.persist_settings();
        cx.notify();
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
            agent: self.agent_settings.clone(),
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

    /// 当前有待处理弹窗的连接（若有）。
    fn pending_connection(&self, cx: &Context<Self>) -> Option<Entity<Connection>> {
        self.connections.pending_prompt_connection(cx)
    }

    /// 回填凭据（None = 取消）。
    pub(crate) fn resolve_credential(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        if let Some(c) = self.pending_connection(cx) {
            c.update(cx, |conn, _| conn.resolve_credential(value));
        }
        self.prompt_input.clear();
        self.prompt_ime_marked_text.clear();
        self.refocus_active_terminal(cx);
        cx.notify();
    }

    /// 回填主机密钥决定。
    pub(crate) fn resolve_host_key(&mut self, decision: HostKeyDecision, cx: &mut Context<Self>) {
        if let Some(c) = self.pending_connection(cx) {
            c.update(cx, |conn, _| conn.resolve_host_key(decision));
        }
        self.refocus_active_terminal(cx);
        cx.notify();
    }

    /// 当前活动模态的显示快照。
    fn current_prompt(&self, cx: &Context<Self>) -> PromptDisplay {
        let Some(conn) = self.pending_connection(cx) else {
            return PromptDisplay::None;
        };
        match conn.read(cx).pending_prompt.as_ref() {
            None => PromptDisplay::None,
            Some(PendingPrompt::HostKey {
                host,
                port,
                key_type,
                fingerprint,
                changed,
                ..
            }) => PromptDisplay::HostKey {
                host: host.clone(),
                port: *port,
                key_type: key_type.clone(),
                fingerprint: fingerprint.clone(),
                changed: *changed,
            },
            Some(PendingPrompt::Credential { kind, prompt, .. }) => PromptDisplay::Credential {
                kind: *kind,
                prompt: prompt.clone(),
            },
        }
    }
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prompt = self.current_prompt(cx);
        let has_prompt = !matches!(prompt, PromptDisplay::None);

        if has_prompt && !self.last_had_prompt {
            self.modal_focus.focus(window, cx);
        }
        if !has_prompt {
            self.prompt_input.clear();
            self.prompt_ime_marked_text.clear();
        }
        self.last_had_prompt = has_prompt;

        // 保持根焦点：没有终端或输入框聚焦时，按键/动作仍能沿 dispatch
        // 路径到达 #app-shell 的动作处理器（否则 Cmd+Q 等会被静默丢弃）。
        if window.focused(cx).is_none() {
            self.shell_focus.focus(window, cx);
        }

        // 快捷命令是 workspace 级面板，使用当前活动视图的上下文；没有活动视图时不显示。
        let quick_context = match self.workspace.focused_view() {
            Some(ActiveView::LocalSession(session_id)) => self
                .workspace
                .sessions
                .local_sessions
                .get(&session_id)
                .map(|session| {
                    let cwd = session
                        .terminal
                        .read(cx)
                        .cwd
                        .clone()
                        .unwrap_or_else(|| session.cwd.to_string_lossy().to_string());
                    let cwd = PathBuf::from(cwd);
                    (local_scope(&cwd), cwd.to_string_lossy().to_string())
                }),
            Some(ActiveView::RemoteTab(index)) => self
                .workspace
                .sessions
                .remote_tabs
                .get(index)
                .and_then(|tab| {
                    tab.pane
                        .cwd(cx)
                        .map(|cwd| (remote_scope(&tab.host_key, &cwd), cwd))
                }),
            None => None,
        };
        let quick_commands_panel_mode = quick_commands_panel_mode(
            quick_context.is_some(),
            self.workspace_settings.show_quick_commands,
        );
        let quick_commands = match quick_commands_panel_mode {
            Some(QuickCommandsPanelMode::Expanded) => {
                let (scope, cwd) =
                    quick_context.expect("expanded panel requires a command context");
                Some(render_quick_commands(self, scope, cwd, cx))
            }
            Some(QuickCommandsPanelMode::Rail) => {
                let (scope, _) = quick_context.expect("rail requires a command context");
                Some(render_quick_commands_rail(self, &scope, cx))
            }
            None => None,
        };

        // Materialize opaque elements before attaching the root listener so Rust 2024 does not
        // keep `cx` borrowed through the render helpers.
        let sidebar_width = if self.workspace_settings.show_host_sidebar {
            self.sidebar_width
                .get()
                .clamp(theme::SIDEBAR_MIN_WIDTH, theme::SIDEBAR_MAX_WIDTH)
        } else {
            theme::SIDEBAR_RAIL_WIDTH
        };
        let quick_commands_width = match quick_commands_panel_mode {
            Some(QuickCommandsPanelMode::Expanded) => self.quick_commands_width.get().clamp(
                theme::QUICK_COMMANDS_MIN_WIDTH,
                theme::QUICK_COMMANDS_MAX_WIDTH,
            ),
            Some(QuickCommandsPanelMode::Rail) => theme::QUICK_COMMANDS_RAIL_WIDTH,
            None => 0.,
        };
        let available_main_width = available_main_width(
            window.viewport_size().width,
            sidebar_width,
            quick_commands_width,
        );
        let main = render_main(self, available_main_width, cx);
        let sidebar = if self.workspace_settings.show_host_sidebar {
            render_sidebar(self, window, cx)
        } else {
            render_sidebar_rail(self, cx)
        };
        let workspace = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_row()
            .child(sidebar)
            .child(main)
            .children(quick_commands);
        let status_bar = render_workspace_status_bar(self, available_main_width, cx);

        let mut root =
            div()
                .id("app-shell")
                .key_context("AppShell")
                .track_focus(&self.shell_focus)
                .flex()
                .flex_col()
                .relative()
                .size_full()
                .bg(theme::canvas())
                .text_color(theme::text())
                .on_action(cx.listener(AppShell::handle_new_terminal))
                .on_action(cx.listener(AppShell::handle_close_active_tab))
                .on_action(cx.listener(|this, _: &crate::OpenProject, _, cx| {
                    this.choose_project_directory(cx)
                }))
                .on_action(cx.listener(|this, _: &crate::ToggleHostSidebar, _, cx| {
                    this.toggle_host_sidebar(cx)
                }))
                .on_action(cx.listener(|this, _: &crate::ToggleQuickCommands, _, cx| {
                    this.toggle_quick_commands(cx)
                }))
                .on_action(cx.listener(|this, _: &crate::ToggleTimestamps, _, cx| {
                    this.toggle_timestamps(cx)
                }))
                .on_action(cx.listener(AppShell::handle_quit))
                .on_key_down(cx.listener(AppShell::handle_shell_key_down))
                .child(workspace)
                .child(status_bar);

        root = root.children(self.render_toaster());

        if matches!(
            prompt,
            PromptDisplay::HostKey { .. } | PromptDisplay::Credential { .. }
        ) {
            root = root.child(render_prompt_modal(self, prompt, window, cx));
        }
        if self.quick_command_editor.is_some() {
            root = root.child(render_quick_command_editor(self, window, cx));
        }
        if self.rename_editor.is_some() {
            root = root.child(render_rename_editor(self, window, cx));
        }
        if let Some(menu) = self.context_menu.clone() {
            root = root.child(render_context_menu(
                &menu,
                Point::new(px(0.), px(0.)),
                window,
                cx,
                |this, action, window, cx| {
                    this.dispatch_shell_menu_action(action, window, cx);
                },
                |this, cx| this.close_context_menu(cx),
            ));
        }
        root
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QuickCommandsPanelMode {
    Expanded,
    Rail,
}

fn quick_commands_panel_mode(
    has_command_context: bool,
    show_quick_commands: bool,
) -> Option<QuickCommandsPanelMode> {
    has_command_context.then_some(if show_quick_commands {
        QuickCommandsPanelMode::Expanded
    } else {
        QuickCommandsPanelMode::Rail
    })
}

fn available_main_width(
    viewport_width: Pixels,
    sidebar_width: f32,
    quick_commands_width: f32,
) -> Pixels {
    px((viewport_width.as_f32() - sidebar_width - quick_commands_width).max(0.))
}

fn host_entry_matches(entry: &HostEntry, query: &str) -> bool {
    entry.alias.to_ascii_lowercase().contains(query)
        || entry.detail.to_ascii_lowercase().contains(query)
}

fn find_remote_terminal_index<'a>(
    tabs: impl DoubleEndedIterator<Item = (usize, &'a str, bool)>,
    host_key: &str,
) -> Option<usize> {
    tabs.rev().find_map(|(idx, tab_host_key, is_terminal)| {
        (tab_host_key == host_key && is_terminal).then_some(idx)
    })
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

#[cfg(test)]
mod tests {
    use super::{
        QuickCommandsPanelMode, available_main_width, find_remote_terminal_index,
        next_char_boundary, previous_char_boundary, quick_commands_panel_mode, selection_bounds,
    };
    use gpui::px;

    #[test]
    fn sidebar_host_reuse_selects_latest_matching_terminal() {
        let tabs = vec![
            (0, "vps", true),
            (1, "vps", false),
            (2, "other", true),
            (3, "vps", true),
        ];

        assert_eq!(find_remote_terminal_index(tabs.into_iter(), "vps"), Some(3));
    }

    #[test]
    fn quick_command_editing_respects_utf8_boundaries_and_selection_direction() {
        let text = "run 你好";
        assert_eq!(previous_char_boundary(text, text.len()), "run 你".len());
        assert_eq!(next_char_boundary(text, "run ".len()), "run 你".len());
        assert_eq!(
            selection_bounds(Some(text.len()), "run ".len()),
            Some((4, text.len()))
        );
        assert_eq!(selection_bounds(Some(4), 4), None);
    }

    #[test]
    fn quick_commands_panel_requires_an_active_command_context() {
        assert_eq!(
            quick_commands_panel_mode(true, true),
            Some(QuickCommandsPanelMode::Expanded)
        );
        assert_eq!(
            quick_commands_panel_mode(true, false),
            Some(QuickCommandsPanelMode::Rail)
        );
        assert_eq!(quick_commands_panel_mode(false, true), None);
        assert_eq!(quick_commands_panel_mode(false, false), None);
    }

    #[test]
    fn main_width_excludes_both_workspace_side_panels() {
        assert_eq!(available_main_width(px(700.), 216., 240.), px(244.));
        assert_eq!(available_main_width(px(700.), 44., 40.), px(616.));
        assert_eq!(available_main_width(px(700.), 216., 0.), px(484.));
        assert_eq!(available_main_width(px(400.), 216., 240.), px(0.));
    }
}

#[cfg(test)]
#[path = "git_sync_toast_tests.rs"]
mod git_sync_toast_tests;
#[cfg(test)]
#[path = "pinned_tab_tests.rs"]
mod pinned_tab_tests;
#[cfg(test)]
#[path = "shell_notification_tests.rs"]
mod shell_notification_tests;

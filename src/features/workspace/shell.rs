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
    KeyDownEvent, ParentElement, PathPromptOptions, Pixels, Point, PromptButton, PromptLevel,
    Render, Styled, SystemNotificationResponse, Task, TitlebarOptions, Window, WindowBounds,
    WindowOptions, div, px, size,
};

use crate::features::commands::{
    BackgroundTaskEvent, BackgroundTaskManager, BackgroundTaskStatus, CommandHistory, local_scope,
    remote_scope,
};
use crate::features::connections::{ConnectionManager, HostEntry};
use crate::features::connections::{PromptDisplay, render_prompt_modal};
use crate::features::forwarding::ForwardPane;
use crate::features::projects::inspect;
use crate::features::sftp::SftpPane;
use crate::features::terminal::{ConnState, TerminalEvent, TerminalView};
use crate::features::workspace::registry::WorkspaceState;
use crate::features::workspace::sidebar::render_sidebar;
use crate::features::workspace::view::{
    ActiveView, LocalDir, LocalSession, LocalSessionId, Pane, Tab, rebuild_local_dirs, render_main,
    render_quick_command_editor,
};
use crate::infrastructure::config::SshConfig;
use crate::infrastructure::local;
use crate::infrastructure::ssh::{Connection, HostKeyDecision, PendingPrompt, RemoteCommandStatus};
use crate::shared::i18n::{self, AppSettings, LanguagePreference};
use crate::shared::ui::context_menu::{
    ContextMenuState, MenuEntry, ShellMenuAction, render_context_menu,
};
use crate::shared::ui::theme;
use crate::shared::ui::widgets::printable_char;

const GIT_STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy)]
enum ExitIntent {
    QuitApp,
    CloseWindow,
}

pub(crate) struct QuickCommandEditor {
    pub(crate) scope: String,
    pub(crate) original: String,
    pub(crate) value: String,
    pub(crate) focus: FocusHandle,
}

struct ActiveCommandContext {
    terminal: Entity<TerminalView>,
    scope: String,
    cwd: String,
    connection: Option<Entity<Connection>>,
}

#[derive(Default)]
struct QuitRiskSummary {
    running_commands: usize,
    sftp_writes: usize,
    unsaved_editors: usize,
    active_forwards: usize,
}

impl QuitRiskSummary {
    fn needs_confirmation(&self) -> bool {
        self.running_commands > 0
            || self.sftp_writes > 0
            || self.unsaved_editors > 0
            || self.active_forwards > 0
    }

    fn detail(&self) -> String {
        let mut lines = vec![i18n::text("quit.warning")];
        if self.running_commands > 0 {
            lines.push(rust_i18n::t!("quit.commands", count = self.running_commands).to_string());
        }
        if self.sftp_writes > 0 {
            lines.push(rust_i18n::t!("quit.transfers", count = self.sftp_writes).to_string());
        }
        if self.unsaved_editors > 0 {
            lines.push(
                rust_i18n::t!("quit.unsaved_editors", count = self.unsaved_editors).to_string(),
            );
        }
        if self.active_forwards > 0 {
            lines.push(rust_i18n::t!("quit.forwards", count = self.active_forwards).to_string());
        }
        lines.push(String::new());
        lines.push(i18n::text("quit.cleanup"));
        lines.join("\n")
    }
}

pub struct AppShell {
    pub(crate) connections: ConnectionManager,
    pub(crate) workspace: WorkspaceState,
    pub(crate) status: Option<String>,
    /// 侧栏搜索文本；未命中配置别名时也作为 QuickConnect 目标。
    pub(crate) host_query: String,
    pub(crate) host_focus: FocusHandle,
    /// 主机分组折叠状态；Bank 默认收起，Local/Active 默认展开。
    pub(crate) bank_collapsed: bool,
    pub(crate) active_collapsed: bool,
    pub(crate) projects_collapsed: bool,
    /// 原生项目目录选择器任务，持有到选择结果返回。
    _project_picker: Option<Task<()>>,
    /// 模态文本输入缓冲（密码/口令）。
    pub(crate) prompt_input: String,
    /// 模态输入框焦点。
    pub(crate) modal_focus: FocusHandle,
    /// 上一帧是否有活动模态（用于在弹窗出现时自动聚焦）。
    last_had_prompt: bool,
    /// 当前语言偏好；实际 locale 由 i18n 全局状态维护。
    pub(crate) language_preference: LanguagePreference,
    pub(crate) language_menu_open: bool,
    /// 当前打开的右键上下文菜单（None = 未打开）。
    pub(crate) context_menu: Option<ContextMenuState<ShellMenuAction>>,
    pub(crate) settings: AppSettings,
    pub(crate) settings_open: bool,
    pub(crate) settings_section: crate::features::settings::SettingsSection,
    pub(crate) settings_scroll: gpui::ScrollHandle,
    /// 侧栏宽度与拖动状态；只影响布局，不改变导航状态。
    pub(crate) sidebar_width: Rc<Cell<f32>>,
    pub(crate) sidebar_dragging: Rc<Cell<bool>>,
    pub(crate) sidebar_scroll: gpui::ScrollHandle,
    pub(crate) tab_scroll: gpui::ScrollHandle,
    /// Right-side command panel width and drag state.
    pub(crate) quick_commands_width: Rc<Cell<f32>>,
    pub(crate) quick_commands_dragging: Rc<Cell<bool>>,
    pub(crate) command_history: CommandHistory,
    pub(crate) background_tasks: BackgroundTaskManager,
    remote_background_controls: BTreeMap<u64, (Entity<Connection>, u64)>,
    background_event_tasks: Vec<Task<()>>,
    pub(crate) quick_command_editor: Option<QuickCommandEditor>,
    /// 周期性刷新本地会话的 Git 状态，覆盖 shell 空闲时的外部文件变更。
    _git_status_refresh_task: Option<Task<()>>,
    quit_confirmation_open: bool,
    shutdown_in_progress: bool,
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
        let settings = i18n::settings(cx);
        let language_preference = settings.language;
        // 启动时把最近的本地目录记录恢复到侧栏 Local 分组（无活动会话，点击即重开）。
        let mut local_dirs = BTreeMap::new();
        for cwd in &settings.recent_local_dirs {
            if cwd.is_dir() {
                local_dirs.insert(
                    normalize_local_cwd(cwd.clone()),
                    LocalDir {
                        project_dir: normalize_local_cwd(cwd.clone()),
                        sessions: Vec::new(),
                        active_session: None,
                    },
                );
            }
        }

        cx.new(|cx| Self {
            connections: ConnectionManager::new(config),
            workspace: WorkspaceState::new(local_dirs),
            status: None,
            host_query: String::new(),
            host_focus: cx.focus_handle(),
            bank_collapsed: true,
            active_collapsed: false,
            projects_collapsed: false,
            _project_picker: None,
            prompt_input: String::new(),
            modal_focus: cx.focus_handle(),
            last_had_prompt: false,
            language_preference,
            language_menu_open: false,
            context_menu: None,
            settings,
            settings_open: false,
            settings_section: crate::features::settings::SettingsSection::General,
            settings_scroll: gpui::ScrollHandle::new(),
            sidebar_width: Rc::new(Cell::new(theme::SIDEBAR_WIDTH)),
            sidebar_dragging: Rc::new(Cell::new(false)),
            sidebar_scroll: gpui::ScrollHandle::new(),
            tab_scroll: gpui::ScrollHandle::new(),
            quick_commands_width: Rc::new(Cell::new(theme::QUICK_COMMANDS_WIDTH)),
            quick_commands_dragging: Rc::new(Cell::new(false)),
            command_history: CommandHistory::load(),
            background_tasks: BackgroundTaskManager::default(),
            remote_background_controls: BTreeMap::new(),
            background_event_tasks: Vec::new(),
            quick_command_editor: None,
            _git_status_refresh_task: None,
            quit_confirmation_open: false,
            shutdown_in_progress: false,
        })
    }

    pub(crate) fn open_host(&mut self, idx: usize, cx: &mut Context<Self>) {
        let entry = match self.connections.entries().get(idx) {
            Some(e) => e.clone(),
            None => return,
        };

        // The sidebar is navigation. Reuse the existing terminal for a live
        // connection instead of opening another channel when returning from a
        // local session.
        if let Some(tab_idx) = self.remote_terminal_to_switch(&entry.key, cx) {
            self.switch_remote_tab(tab_idx, cx);
            return;
        }

        self.open_terminal_target(entry.alias, cx);
    }

    fn remote_terminal_to_switch(&self, host_key: &str, cx: &Context<Self>) -> Option<usize> {
        let state = self.connections.state_for_key(host_key, cx);
        if !is_reusable_connection_state(&state) {
            return None;
        }

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
                        matches!(&tab.pane, Pane::Terminal(_)),
                    )
                }),
            host_key,
        )
    }

    /// 按别名或 `user@host[:port]` 打开一个终端标签。
    ///
    /// 空认证候选也允许继续：Connection 会在认证失败前向 UI 请求密码，
    /// 这样密码登录主机不会被侧栏提前拦截。
    fn open_terminal_target(&mut self, target: String, cx: &mut Context<Self>) {
        self.settings_open = false;
        let resolved = self.connections.resolve(&target);
        let methods = self.connections.auth_methods(&resolved);
        let host_key = ConnectionManager::pool_key(&resolved);

        // 复用或新建连接，开一个终端 channel。
        let conn = self.connections.acquire(resolved, methods, cx);
        let (input_tx, event_rx) = conn.read(cx).open_terminal(100, 30);
        let terminal = TerminalView::from_bridge(input_tx, event_rx, 100, 30, cx);
        let event_host_key = host_key.clone();
        let subscription = cx.subscribe(&terminal, move |this, terminal, event, cx| match event {
            TerminalEvent::Closed => {
                this.close_remote_terminal(terminal.entity_id(), cx);
            }
            TerminalEvent::TitleChanged | TerminalEvent::Notification => cx.notify(),
            TerminalEvent::CommandStarted { command, cwd } => {
                if !terminal.read(cx).is_local()
                    && let Some(cwd) = cwd.as_deref()
                {
                    this.record_command(remote_scope(&event_host_key, cwd), command.clone(), cx);
                }
            }
            TerminalEvent::CwdChanged => cx.notify(),
            TerminalEvent::PromptReached => {}
        });
        self.workspace
            .sessions
            .terminal_subscriptions
            .push(subscription);
        self.workspace.sessions.remote_tabs.push(Tab {
            target,
            host_key,
            connection: conn,
            pane: Pane::Terminal(terminal),
        });
        self.workspace.active_view = Some(ActiveView::RemoteTab(
            self.workspace.sessions.remote_tabs.len() - 1,
        ));
        self.status = None;
        cx.notify();
    }

    pub(crate) fn open_sftp(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.settings_open = false;
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
            connection: conn.clone(),
            pane: Pane::Sftp(pane),
        });
        self.workspace.active_view = Some(ActiveView::RemoteTab(
            self.workspace.sessions.remote_tabs.len() - 1,
        ));
        cx.notify();
    }

    pub(crate) fn open_forward(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.settings_open = false;
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
            connection: conn,
            pane: Pane::Forward(pane),
        });
        self.workspace.active_view = Some(ActiveView::RemoteTab(
            self.workspace.sessions.remote_tabs.len() - 1,
        ));
        cx.notify();
    }

    /// 在项目目录 view 中打开一个独立的本地 PTY session。
    /// `project_dir` 决定侧栏归属，`cwd` 只决定 shell 的初始工作目录。
    pub(crate) fn open_local_session(
        &mut self,
        project_dir: PathBuf,
        cwd: PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.settings_open = false;
        let project_dir = normalize_local_cwd(project_dir);
        let cwd = normalize_local_cwd(cwd);
        self.remember_local_dir(&project_dir, cx);
        let cwd_text = cwd.to_string_lossy().to_string();
        let (input_tx, event_rx) = local::open_terminal(cwd.clone(), 100, 30);
        let terminal =
            TerminalView::from_local_bridge(input_tx, event_rx, 100, 30, cwd_text.clone(), cx);
        let session_id = self.workspace.sessions.allocate_local_session_id();
        log::info!("local session {session_id} opened for {}", cwd_text);
        // shell 内 `cd` 会经 OSC 7 上报新目录；订阅后更新 cwd 和 Git 状态，
        // 但不会改变 session 的项目归属。
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
                if let Some(cwd) = cwd {
                    let cwd = normalize_local_cwd(PathBuf::from(cwd));
                    this.record_command(local_scope(&cwd), command.clone(), cx);
                }
            }
            TerminalEvent::TitleChanged | TerminalEvent::Notification => cx.notify(),
        });
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
                git_refresh_generation: 0,
            },
        );
        self.ensure_git_status_refresh_task(cx);
        self.sync_local_dirs(cx);
        self.select_local_session(session_id, cx);
        self.refresh_git_status(session_id, false, cx);
        self.status = None;
        cx.notify();
    }

    pub(crate) fn activate_local_dir(&mut self, project_dir: PathBuf, cx: &mut Context<Self>) {
        let project_dir = normalize_local_cwd(project_dir);
        self.sync_local_dirs(cx);
        let session_id = self
            .workspace
            .sessions
            .local_dirs
            .get(&project_dir)
            .and_then(|dir| dir.active_session.or_else(|| dir.sessions.first().copied()));
        if let Some(session_id) = session_id {
            self.select_local_session(session_id, cx);
        } else {
            self.open_local_session(project_dir.clone(), project_dir, cx);
        }
    }

    pub(crate) fn select_local_session(
        &mut self,
        session_id: LocalSessionId,
        cx: &mut Context<Self>,
    ) {
        self.settings_open = false;
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
        let was_active = self.workspace.active_view == Some(ActiveView::LocalSession(session_id));
        let mut next_session = None;
        let remove_dir = if let Some(dir) = self.workspace.sessions.local_dirs.get_mut(&cwd) {
            dir.sessions.retain(|id| *id != session_id);
            if dir.active_session == Some(session_id) {
                dir.active_session = dir.sessions.first().copied();
            }
            next_session = dir.active_session;
            // 仍被「最近本地目录」记住的空目录保留在侧栏，等待下次点击重开。
            dir.sessions.is_empty() && !self.settings.recent_local_dirs.contains(&cwd)
        } else {
            false
        };
        self.workspace.sessions.local_sessions.remove(&session_id);
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
        cx.notify();
    }

    fn switch_tab(&mut self, idx: usize, cx: &mut Context<Self>) {
        match self.workspace.active_view {
            Some(ActiveView::RemoteTab(_)) => self.switch_remote_tab(idx, cx),
            Some(ActiveView::LocalSession(session_id)) => {
                let next_session = self
                    .local_dir_for_session(session_id)
                    .and_then(|dir| dir.sessions.get(idx).copied());
                if let Some(next_session) = next_session {
                    self.select_local_session(next_session, cx);
                }
            }
            None => {}
        }
    }

    pub(crate) fn switch_remote_tab(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.workspace.sessions.remote_tabs.len() {
            return;
        }
        self.settings_open = false;
        self.workspace.active_view = Some(ActiveView::RemoteTab(idx));
        self.refocus_active_terminal(cx);
        cx.notify();
    }

    pub(crate) fn close_remote_tab(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.workspace.sessions.remote_tabs.len() {
            return;
        }
        self.workspace.sessions.remote_tabs.remove(idx);
        // 移除 Tab → Entity<TerminalView> 释放 → input_tx 断 → relay 结束 →
        // Connection channel 计数减；归 0 则连接自行 disconnect。
        self.workspace.active_view = match self.workspace.active_view {
            Some(ActiveView::RemoteTab(a)) if a == idx => {
                if self.workspace.sessions.remote_tabs.is_empty() {
                    self.first_local_view()
                } else if a >= self.workspace.sessions.remote_tabs.len() {
                    Some(ActiveView::RemoteTab(
                        self.workspace.sessions.remote_tabs.len() - 1,
                    ))
                } else {
                    Some(ActiveView::RemoteTab(a))
                }
            }
            Some(ActiveView::RemoteTab(a)) if a > idx => Some(ActiveView::RemoteTab(a - 1)),
            other => other,
        };
        cx.notify();
    }

    fn close_remote_terminal(&mut self, terminal_id: EntityId, cx: &mut Context<Self>) {
        let Some(idx) = self.workspace.sessions.remote_tabs.iter().position(|tab| {
            matches!(&tab.pane, Pane::Terminal(terminal) if terminal.entity_id() == terminal_id)
        }) else {
            return;
        };
        self.close_remote_tab(idx, cx);
    }

    fn handle_system_notification_response(
        &mut self,
        response: SystemNotificationResponse,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut terminals = Vec::new();
        for (index, tab) in self.workspace.sessions.remote_tabs.iter().enumerate() {
            if let Pane::Terminal(terminal) = &tab.pane {
                terminals.push((ActiveView::RemoteTab(index), terminal.clone()));
            }
        }
        for (&session_id, session) in &self.workspace.sessions.local_sessions {
            terminals.push((
                ActiveView::LocalSession(session_id),
                session.terminal.clone(),
            ));
        }

        for (view, terminal) in terminals {
            let handled = terminal.update(cx, |terminal, cx| {
                terminal.handle_system_notification_response(&response, cx)
            });
            let Some(focus) = handled else {
                continue;
            };
            if focus {
                self.workspace.active_view = Some(view);
                terminal.update(cx, |terminal, _cx| terminal.request_focus());
                cx.notify();
            }
            return true;
        }
        false
    }

    fn close_active_tab(&mut self, cx: &mut Context<Self>) {
        match self.workspace.active_view {
            Some(ActiveView::RemoteTab(idx)) => self.close_remote_tab(idx, cx),
            Some(ActiveView::LocalSession(session_id)) => self.close_local_session(session_id, cx),
            None => {}
        }
    }

    fn cycle_tab(&mut self, direction: isize, cx: &mut Context<Self>) {
        match self.workspace.active_view {
            Some(ActiveView::RemoteTab(current)) => {
                let len = self.workspace.sessions.remote_tabs.len();
                if len == 0 {
                    return;
                }
                let next = (current as isize + direction).rem_euclid(len as isize) as usize;
                self.switch_remote_tab(next, cx);
            }
            Some(ActiveView::LocalSession(session_id)) => {
                let Some(dir) = self.local_dir_for_session(session_id) else {
                    return;
                };
                let Some(current) = dir.sessions.iter().position(|id| *id == session_id) else {
                    return;
                };
                let next =
                    (current as isize + direction).rem_euclid(dir.sessions.len() as isize) as usize;
                if let Some(next_session) = dir.sessions.get(next).copied() {
                    self.select_local_session(next_session, cx);
                }
            }
            None => {}
        }
    }

    /// 从当前标签复制一个终端标签；没有活动标签时把焦点放到快速连接框。
    pub(crate) fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.workspace.active_view {
            Some(ActiveView::LocalSession(session_id)) => {
                let project_dir = self.local_session_project_dir(session_id);
                let cwd = self.local_session_cwd(session_id, cx);
                self.open_local_session(project_dir, cwd, cx);
                return;
            }
            Some(ActiveView::RemoteTab(idx)) => {
                if let Some(tab) = self.workspace.sessions.remote_tabs.get(idx) {
                    let target = tab.target.clone();
                    self.open_terminal_target(target, cx);
                    return;
                }
            }
            None => {}
        }
        self.host_query.clear();
        self.host_focus.focus(window, cx);
        cx.notify();
    }

    fn record_command(&mut self, scope: String, command: String, cx: &mut Context<Self>) {
        if self.command_history.record(&scope, &command) {
            cx.notify();
        }
    }

    fn active_command_context(&self, cx: &Context<Self>) -> Option<ActiveCommandContext> {
        match self.workspace.active_view? {
            ActiveView::LocalSession(session_id) => {
                let session = self.workspace.sessions.local_sessions.get(&session_id)?;
                let terminal = session.terminal.clone();
                let cwd = terminal
                    .read(cx)
                    .cwd
                    .clone()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| session.cwd.clone());
                let cwd = normalize_local_cwd(cwd);
                Some(ActiveCommandContext {
                    terminal,
                    scope: local_scope(&cwd),
                    cwd: cwd.to_string_lossy().to_string(),
                    connection: None,
                })
            }
            ActiveView::RemoteTab(index) => {
                let tab = self.workspace.sessions.remote_tabs.get(index)?;
                let Pane::Terminal(terminal) = &tab.pane else {
                    return None;
                };
                let terminal = terminal.clone();
                let cwd = terminal.read(cx).cwd.clone()?;
                Some(ActiveCommandContext {
                    terminal,
                    scope: remote_scope(&tab.host_key, &cwd),
                    cwd,
                    connection: Some(tab.connection.clone()),
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
            if let Some(connection) = context.connection {
                self.start_remote_background(connection, scope, context.cwd, command, cx);
            } else {
                let cwd = normalize_local_cwd(PathBuf::from(context.cwd));
                let (id, event_rx) = self.background_tasks.start(scope, cwd, command);
                let task = cx.spawn(async move |weak, cx| {
                    let Ok(event) = event_rx.recv().await else {
                        return;
                    };
                    let _ = weak.update(cx, |this, cx| {
                        this.apply_background_event(event);
                        cx.notify();
                    });
                });
                self.background_event_tasks.push(task);
                log::info!("started background command {id}");
            }
        } else {
            context
                .terminal
                .update(cx, |terminal, _cx| terminal.run_command(&command));
        }
        cx.notify();
    }

    fn start_remote_background(
        &mut self,
        connection: Entity<Connection>,
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
            .start_remote(scope, PathBuf::from(cwd), command);
        self.remote_background_controls
            .insert(task_id, (connection, remote_id));
        let expected_remote_id = remote_id;
        let task = cx.spawn(async move |weak, cx| {
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
                this.apply_background_event(event);
                cx.notify();
            });
        });
        self.background_event_tasks.push(task);
        log::info!("started remote background command {task_id}");
    }

    fn apply_background_event(&mut self, event: BackgroundTaskEvent) {
        log::info!(
            "background command {} finished as {:?}",
            event.id,
            event.status
        );
        self.remote_background_controls.remove(&event.id);
        self.background_tasks.apply_event(event);
    }

    pub(crate) fn stop_background_task(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some((connection, remote_id)) = self.remote_background_controls.get(&id).cloned() {
            connection.read(cx).stop_command(remote_id);
        }
        self.background_tasks.mark_stopping(id);
        cx.notify();
    }

    pub(crate) fn open_quick_command_editor(
        &mut self,
        scope: String,
        command: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = cx.focus_handle();
        self.quick_command_editor = Some(QuickCommandEditor {
            scope,
            original: command.clone(),
            value: command,
            focus: focus.clone(),
        });
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(crate) fn submit_quick_command_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.quick_command_editor.take() else {
            return;
        };
        self.command_history
            .edit(&editor.scope, &editor.original, &editor.value);
        cx.notify();
    }

    pub(crate) fn cancel_quick_command_editor(&mut self, cx: &mut Context<Self>) {
        if self.quick_command_editor.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn handle_quick_command_editor_key(
        &mut self,
        ev: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev.keystroke.key.as_str() {
            "enter" | "return" => self.submit_quick_command_editor(cx),
            "escape" => self.cancel_quick_command_editor(cx),
            "backspace" => {
                if let Some(editor) = &mut self.quick_command_editor {
                    editor.value.pop();
                }
                cx.notify();
            }
            _ => {
                if let Some(ch) = printable_char(&ev.keystroke)
                    && let Some(editor) = &mut self.quick_command_editor
                {
                    editor.value.push(ch);
                    cx.notify();
                }
            }
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
                cx.notify();
            }
            "backspace" => {
                self.host_query.pop();
                cx.notify();
            }
            _ => {
                if let Some(ch) = printable_char(&ev.keystroke) {
                    self.host_query.push(ch);
                    cx.notify();
                } else if ev.keystroke.key == "tab" {
                    self.host_focus.focus(window, cx);
                }
            }
        }
    }

    fn handle_quit(&mut self, _: &crate::Quit, window: &mut Window, cx: &mut Context<Self>) {
        self.request_exit(ExitIntent::QuitApp, window, cx);
    }

    fn should_close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.shutdown_in_progress {
            return true;
        }
        if !self.quit_risks(cx).needs_confirmation() {
            self.begin_shutdown(cx);
            return true;
        }
        self.request_exit(ExitIntent::CloseWindow, window, cx);
        false
    }

    fn request_exit(&mut self, intent: ExitIntent, window: &mut Window, cx: &mut Context<Self>) {
        if self.shutdown_in_progress || self.quit_confirmation_open {
            return;
        }

        let risks = self.quit_risks(cx);
        if !risks.needs_confirmation() {
            self.begin_shutdown(cx);
            match intent {
                ExitIntent::QuitApp => cx.quit(),
                ExitIntent::CloseWindow => window.remove_window(),
            }
            return;
        }

        self.quit_confirmation_open = true;
        let answers = [
            PromptButton::ok(i18n::text("quit.confirm")),
            PromptButton::cancel(i18n::text("quit.cancel")),
        ];
        let answer = window.prompt(
            PromptLevel::Warning,
            &i18n::text("quit.title"),
            Some(&risks.detail()),
            &answers,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let confirmed = answer.await == Ok(0);
            let _ = this.update(cx, |this, cx| {
                this.quit_confirmation_open = false;
                if confirmed {
                    this.begin_shutdown(cx);
                }
            });
            if !confirmed {
                return;
            }

            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let _ = cx.update(|window, cx| match intent {
                ExitIntent::QuitApp => cx.quit(),
                ExitIntent::CloseWindow => window.remove_window(),
            });
        })
        .detach();
    }

    fn quit_risks(&self, cx: &Context<Self>) -> QuitRiskSummary {
        let mut risks = QuitRiskSummary {
            running_commands: self.background_tasks.running_count(),
            ..QuitRiskSummary::default()
        };
        for session in self.workspace.sessions.local_sessions.values() {
            if session.terminal.read(cx).is_command_running() {
                risks.running_commands += 1;
            }
        }
        for tab in &self.workspace.sessions.remote_tabs {
            match &tab.pane {
                Pane::Terminal(terminal) => {
                    if terminal.read(cx).is_command_running() {
                        risks.running_commands += 1;
                    }
                }
                Pane::Sftp(sftp) => {
                    let sftp = sftp.read(cx);
                    risks.sftp_writes += usize::from(sftp.has_active_write());
                    risks.unsaved_editors += usize::from(sftp.has_unsaved_changes());
                }
                Pane::Forward(forward) => {
                    risks.active_forwards += forward.read(cx).active_count();
                }
            }
        }
        risks
    }

    fn begin_shutdown(&mut self, cx: &mut Context<Self>) {
        if self.shutdown_in_progress {
            return;
        }
        self.shutdown_in_progress = true;
        self.status = Some(i18n::text("quit.closing"));
        let running_background = self
            .background_tasks
            .tasks
            .values()
            .filter(|task| {
                matches!(
                    task.status,
                    crate::features::commands::BackgroundTaskStatus::Running
                        | crate::features::commands::BackgroundTaskStatus::Stopping
                )
            })
            .map(|task| task.id)
            .collect::<Vec<_>>();
        for id in running_background {
            self.stop_background_task(id, cx);
        }

        let mut terminals = self
            .workspace
            .sessions
            .local_sessions
            .values()
            .map(|session| session.terminal.clone())
            .collect::<Vec<_>>();
        let mut forwards = Vec::new();
        for tab in &self.workspace.sessions.remote_tabs {
            match &tab.pane {
                Pane::Terminal(terminal) => terminals.push(terminal.clone()),
                Pane::Forward(forward) => forwards.push(forward.clone()),
                Pane::Sftp(_) => {}
            }
        }
        for terminal in terminals {
            terminal.update(cx, |terminal, _cx| terminal.request_close());
        }
        for forward in forwards {
            forward.update(cx, |forward, cx| forward.stop_all(cx));
        }
        cx.notify();
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
        if !matches!(self.current_prompt(cx), PromptDisplay::None) {
            return;
        }
        let ks = &ev.keystroke;
        let primary = ks.modifiers.platform || ks.modifiers.control;
        if !primary {
            return;
        }

        match ks.key.as_str() {
            "w" => self.close_active_tab(cx),
            "t" => self.new_tab(window, cx),
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

    pub(crate) fn toggle_language_menu(&mut self, cx: &mut Context<Self>) {
        self.language_menu_open = !self.language_menu_open;
        self.settings_open = false;
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
        self.language_menu_open = false;
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
            ShellMenuAction::OpenHost(idx) => self.open_host(idx, cx),
            ShellMenuAction::OpenSftp(idx) => self.open_sftp(idx, cx),
            ShellMenuAction::OpenForward(idx) => self.open_forward(idx, cx),
            ShellMenuAction::CopyText(text) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
            }
            ShellMenuAction::RevealInFinder(path) => {
                std::process::Command::new("open").arg(&path).spawn().ok();
            }
            ShellMenuAction::ForgetLocalDir(cwd) => self.forget_local_dir(cwd, cx),
            ShellMenuAction::OpenLocalTerminal(cwd) => {
                self.open_local_session(cwd.clone(), cwd, cx)
            }
            ShellMenuAction::SelectRemoteTab(idx) => self.switch_remote_tab(idx, cx),
            ShellMenuAction::CloseRemoteTab(idx) => self.close_remote_tab(idx, cx),
            ShellMenuAction::CloseOtherRemoteTabs(idx) => self.close_other_remote_tabs(idx, cx),
            ShellMenuAction::CloseAllRemoteTabs => self.close_all_remote_tabs(cx),
            ShellMenuAction::SelectLocalSession(session_id) => {
                self.select_local_session(session_id, cx);
            }
            ShellMenuAction::CloseLocalSession(session_id) => {
                self.close_local_session(session_id, cx);
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
            ShellMenuAction::DeleteQuickCommand { scope, command } => {
                self.command_history.remove(&scope, &command);
                cx.notify();
            }
            ShellMenuAction::StopBackgroundTask(id) => self.stop_background_task(id, cx),
        }
        self.close_context_menu(cx);
    }

    /// 关闭除 `keep` 外的全部远程标签。
    fn close_other_remote_tabs(&mut self, keep: usize, cx: &mut Context<Self>) {
        if keep >= self.workspace.sessions.remote_tabs.len() {
            return;
        }
        self.workspace.sessions.remote_tabs =
            vec![self.workspace.sessions.remote_tabs.swap_remove(keep)];
        self.workspace.active_view = Some(ActiveView::RemoteTab(0));
        cx.notify();
    }

    fn close_all_remote_tabs(&mut self, cx: &mut Context<Self>) {
        if self.workspace.sessions.remote_tabs.is_empty() {
            return;
        }
        self.workspace.sessions.remote_tabs.clear();
        self.workspace.active_view = self.first_local_view();
        cx.notify();
    }

    /// 关闭同一目录下的其他本地会话（保留 `keep`）。
    fn close_other_local_sessions(&mut self, keep: LocalSessionId, cx: &mut Context<Self>) {
        let Some(others) = self.local_dir_for_session(keep).map(|dir| {
            dir.sessions
                .iter()
                .copied()
                .filter(|id| *id != keep)
                .collect::<Vec<_>>()
        }) else {
            return;
        };
        for session_id in others {
            self.close_local_session(session_id, cx);
        }
        self.select_local_session(keep, cx);
    }

    pub(crate) fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_open = !self.settings_open;
        self.language_menu_open = false;
        cx.notify();
    }

    pub(crate) fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_open = false;
        cx.notify();
    }

    pub(crate) fn select_settings_section(
        &mut self,
        section: crate::features::settings::SettingsSection,
        cx: &mut Context<Self>,
    ) {
        self.settings_section = section;
        self.settings_scroll
            .set_offset(gpui::Point::new(px(0.), px(0.)));
        cx.notify();
    }

    pub(crate) fn apply_settings(&mut self, settings: AppSettings, cx: &mut Context<Self>) {
        let settings = settings.normalized();
        if self.settings == settings {
            return;
        }
        let language_changed = self.language_preference != settings.language;
        let language = settings.language;

        for tab in &self.workspace.sessions.remote_tabs {
            match &tab.pane {
                Pane::Terminal(terminal) => {
                    let terminal_settings = settings.clone();
                    terminal.update(cx, |terminal, cx| {
                        terminal.apply_settings(terminal_settings, cx)
                    });
                }
                Pane::Sftp(pane) if language_changed => {
                    pane.update(cx, |_, cx| cx.notify());
                }
                Pane::Forward(pane) if language_changed => {
                    pane.update(cx, |_, cx| cx.notify());
                }
                Pane::Sftp(_) | Pane::Forward(_) => {}
            }
        }
        for session in self.workspace.sessions.local_sessions.values() {
            let terminal_settings = settings.clone();
            session.terminal.update(cx, |terminal, cx| {
                terminal.apply_settings(terminal_settings, cx)
            });
        }

        i18n::set_settings(cx, settings.clone());
        self.settings = settings;
        self.language_preference = language;
        cx.notify();
    }

    pub(crate) fn set_language(&mut self, preference: LanguagePreference, cx: &mut Context<Self>) {
        if self.language_preference == preference {
            self.language_menu_open = false;
            cx.notify();
            return;
        }
        let mut settings = self.settings.clone();
        settings.language = preference;
        self.apply_settings(settings, cx);
        self.language_menu_open = false;
        cx.notify();
    }

    pub(crate) fn toggle_timestamps(&mut self, cx: &mut Context<Self>) {
        let mut settings = self.settings.clone();
        settings.show_timestamps = !settings.show_timestamps;
        self.apply_settings(settings, cx);
    }

    pub(crate) fn toggle_terminal_notifications(&mut self, cx: &mut Context<Self>) {
        let mut settings = self.settings.clone();
        settings.terminal_notifications = !settings.terminal_notifications;
        self.apply_settings(settings, cx);
    }

    pub(crate) fn adjust_font_size(&mut self, delta: f32, cx: &mut Context<Self>) {
        let mut settings = self.settings.clone();
        settings.terminal_font_size = (settings.terminal_font_size + delta)
            .round()
            .clamp(i18n::MIN_TERMINAL_FONT_SIZE, i18n::MAX_TERMINAL_FONT_SIZE);
        self.apply_settings(settings, cx);
    }

    pub(crate) fn set_scrollback(&mut self, scrollback: usize, cx: &mut Context<Self>) {
        let mut settings = self.settings.clone();
        settings.terminal_scrollback = scrollback;
        self.apply_settings(settings, cx);
    }

    pub(crate) fn set_recent_dirs_max(&mut self, max: usize, cx: &mut Context<Self>) {
        let mut settings = self.settings.clone();
        settings.recent_local_dirs_max = max;
        self.apply_settings(settings, cx);
        self.sync_local_dirs(cx);
    }

    /// 把本地会话按创建时的项目归属目录重建目录视图（打开/关闭/`cd` 时调用）。
    pub(crate) fn sync_local_dirs(&mut self, cx: &Context<Self>) {
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
                if let Some(cwd) = session.terminal.read(cx).cwd.as_deref() {
                    session.cwd = normalize_local_cwd(PathBuf::from(cwd));
                }
                (session_id, session.project_dir.clone())
            })
            .collect::<Vec<_>>();
        self.workspace.sessions.local_dirs = rebuild_local_dirs(
            &previous,
            sessions,
            self.settings.recent_local_dirs.iter().cloned(),
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
        session.git_refresh_generation = session.git_refresh_generation.wrapping_add(1);
        let generation = session.git_refresh_generation;
        let cwd = session.cwd.clone();
        if clear_stale {
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
                if session.git_refresh_generation != generation {
                    return;
                }
                session.git_status = status;
                cx.notify();
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
                if tick.is_multiple_of(60) {
                    log::info!("git status refresh loop alive (tick {tick})");
                }

                if weak
                    .update(cx, |this, cx| {
                        let session_ids = this
                            .workspace
                            .sessions
                            .local_sessions
                            .keys()
                            .copied()
                            .collect::<Vec<_>>();
                        for session_id in session_ids {
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
    fn remember_local_dir(&mut self, project_dir: &Path, cx: &mut Context<Self>) {
        let project_dir = normalize_local_cwd(project_dir.to_path_buf());
        self.settings
            .recent_local_dirs
            .retain(|existing| existing != &project_dir);
        self.settings.recent_local_dirs.insert(0, project_dir);
        self.settings
            .recent_local_dirs
            .truncate(self.settings.recent_local_dirs_max);
        self.persist_settings(cx);
    }

    /// 从「最近本地目录」历史中移除一个目录并持久化。
    pub(crate) fn forget_local_dir(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        let cwd = normalize_local_cwd(cwd);
        if !self.settings.recent_local_dirs.contains(&cwd) {
            return;
        }
        self.settings
            .recent_local_dirs
            .retain(|existing| existing != &cwd);
        self.persist_settings(cx);
        self.sync_local_dirs(cx);
        cx.notify();
    }

    /// 清空「最近本地目录」历史。
    pub(crate) fn clear_recent_dirs(&mut self, cx: &mut Context<Self>) {
        if self.settings.recent_local_dirs.is_empty() {
            return;
        }
        self.settings.recent_local_dirs.clear();
        self.persist_settings(cx);
        self.sync_local_dirs(cx);
        cx.notify();
    }

    /// 只写设置全局状态与磁盘，不重放终端设置（区别于 apply_settings）。
    fn persist_settings(&mut self, cx: &mut Context<Self>) {
        i18n::set_settings(cx, self.settings.clone());
    }

    pub(crate) fn local_dir_for_session(&self, session_id: LocalSessionId) -> Option<&LocalDir> {
        self.workspace
            .sessions
            .local_dirs
            .iter()
            .find_map(|(_, dir)| dir.sessions.contains(&session_id).then_some(dir))
    }

    fn first_local_view(&self) -> Option<ActiveView> {
        self.workspace
            .sessions
            .local_dirs
            .values()
            .find_map(|dir| dir.active_session.map(ActiveView::LocalSession))
    }

    fn local_session_cwd(&self, session_id: LocalSessionId, cx: &Context<Self>) -> PathBuf {
        self.workspace
            .sessions
            .local_sessions
            .get(&session_id)
            .map(|session| {
                session
                    .terminal
                    .read(cx)
                    .cwd
                    .as_deref()
                    .map(|cwd| normalize_local_cwd(PathBuf::from(cwd)))
                    .unwrap_or_else(|| session.cwd.clone())
            })
            .unwrap_or_else(current_local_cwd)
    }

    fn local_session_project_dir(&self, session_id: LocalSessionId) -> PathBuf {
        self.workspace
            .sessions
            .local_sessions
            .get(&session_id)
            .map(|session| session.project_dir.clone())
            .unwrap_or_else(current_local_cwd)
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

    /// 把焦点交还给当前活动终端 tab（切换 tab / 关闭模态后调用）。
    fn refocus_active_terminal(&self, cx: &mut Context<Self>) {
        match self.workspace.active_view {
            Some(ActiveView::RemoteTab(idx)) => {
                if let Some(Tab {
                    pane: Pane::Terminal(terminal),
                    ..
                }) = self.workspace.sessions.remote_tabs.get(idx)
                {
                    terminal.update(cx, |terminal, _| terminal.request_focus());
                }
            }
            Some(ActiveView::LocalSession(session_id)) => {
                if let Some(session) = self.workspace.sessions.local_sessions.get(&session_id) {
                    session
                        .terminal
                        .update(cx, |terminal, _| terminal.request_focus());
                }
            }
            None => {}
        }
    }

    /// 回填凭据（None = 取消）。
    pub(crate) fn resolve_credential(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        if let Some(c) = self.pending_connection(cx) {
            c.update(cx, |conn, _| conn.resolve_credential(value));
        }
        self.prompt_input.clear();
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
        }
        self.last_had_prompt = has_prompt;

        let sidebar = render_sidebar(self, cx);
        // Materialize the opaque element before attaching the root listener so
        // Rust 2024 does not keep `cx` borrowed through `render_main`.
        let main = render_main(self, cx);

        let mut root = div()
            .id("app-shell")
            .flex()
            .flex_row()
            .size_full()
            .bg(theme::canvas())
            .text_color(theme::text())
            .on_action(cx.listener(AppShell::handle_quit))
            .on_key_down(cx.listener(AppShell::handle_shell_key_down))
            .child(sidebar)
            .child(main);

        if matches!(
            prompt,
            PromptDisplay::HostKey { .. } | PromptDisplay::Credential { .. }
        ) {
            root = root.child(render_prompt_modal(self, prompt, cx));
        }
        if self.quick_command_editor.is_some() {
            root = root.child(render_quick_command_editor(self, cx));
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

fn current_local_cwd() -> PathBuf {
    normalize_local_cwd(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")))
}

fn normalize_local_cwd(path: PathBuf) -> PathBuf {
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map(|base| base.join(&path))
            .unwrap_or(path)
    };
    if path.is_dir() {
        path.canonicalize().unwrap_or(path)
    } else {
        PathBuf::from("/")
    }
}

fn host_entry_matches(entry: &HostEntry, query: &str) -> bool {
    entry.alias.to_ascii_lowercase().contains(query)
        || entry.detail.to_ascii_lowercase().contains(query)
}

fn is_reusable_connection_state(state: &Option<ConnState>) -> bool {
    matches!(
        state,
        Some(ConnState::Connecting) | Some(ConnState::Connected)
    )
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
        ConnState, QuitRiskSummary, find_remote_terminal_index, is_reusable_connection_state,
    };

    #[test]
    fn quit_confirmation_is_only_required_for_material_activity() {
        assert!(!QuitRiskSummary::default().needs_confirmation());

        for risks in [
            QuitRiskSummary {
                running_commands: 1,
                ..Default::default()
            },
            QuitRiskSummary {
                sftp_writes: 1,
                ..Default::default()
            },
            QuitRiskSummary {
                unsaved_editors: 1,
                ..Default::default()
            },
            QuitRiskSummary {
                active_forwards: 1,
                ..Default::default()
            },
        ] {
            assert!(risks.needs_confirmation());
        }
    }

    #[test]
    fn sidebar_host_reuse_only_accepts_live_connection_states() {
        assert!(is_reusable_connection_state(&Some(ConnState::Connecting)));
        assert!(is_reusable_connection_state(&Some(ConnState::Connected)));
        assert!(!is_reusable_connection_state(&None));
        assert!(!is_reusable_connection_state(&Some(ConnState::Closed)));
        assert!(!is_reusable_connection_state(&Some(ConnState::Error(
            "connection failed".into(),
        ))));
    }

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
}

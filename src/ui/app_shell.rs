//! 应用外壳：左侧主机列表 + 顶部标签条 + 终端工作区 + 模态弹窗。
//!
//! - 连接池：同主机复用一条已认证会话（开新终端 channel），全部终端关闭才断开。
//! - 多标签：每点击主机新开一个终端标签；可切换/关闭。
//! - sidebar：Local、Active、Bank 为同级可折叠组。
//! - 模态：池中任一连接出现 pending_prompt（未知主机密钥/凭据）时弹覆盖层。

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Entity, FocusHandle, FontWeight,
    InteractiveElement, IntoElement, KeyDownEvent, Keystroke, MouseButton, MouseMoveEvent,
    MouseUpEvent, ParentElement, PathPromptOptions, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Task, TitlebarOptions, Window, WindowBounds, WindowOptions,
    canvas, div, hsla, px, rgb, size,
};

use crate::config::SshConfig;
use crate::i18n::{self, AppSettings, LanguagePreference};
use crate::local;
use crate::ssh::{
    Connection, ConnectionPool, CredentialKind, HostKeyDecision, PendingPrompt, default_auth_for,
};
use crate::ui::terminal_view::ConnState;
use crate::ui::{ForwardPane, SftpPane, TerminalView, icons, theme};

/// 主机列表条目：别名 + 详情 + 池键（用于查连接状态）。
#[derive(Clone)]
struct HostEntry {
    alias: String,
    detail: String,
    key: String,
}

/// 一个标签内承载的面板。
enum Pane {
    Terminal(Entity<TerminalView>),
    Sftp(Entity<SftpPane>),
    Forward(Entity<ForwardPane>),
}

/// 一个远程终端/SFTP 标签。
struct Tab {
    /// 重新打开终端时使用的原始目标（别名或 user@host:port）。
    target: String,
    alias: String,
    host_key: String,
    pane: Pane,
}

type LocalSessionId = u64;

/// 当前主区正在展示的工作区。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActiveView {
    RemoteTab(usize),
    LocalSession(LocalSessionId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsSection {
    General,
    Terminal,
}

struct LocalSession {
    cwd: PathBuf,
    terminal: Entity<TerminalView>,
}

struct LocalDir {
    cwd: PathBuf,
    sessions: Vec<LocalSessionId>,
    active_session: Option<LocalSessionId>,
}

pub struct AppShell {
    config: Arc<SshConfig>,
    entries: Vec<HostEntry>,
    pool: ConnectionPool,
    remote_tabs: Vec<Tab>,
    local_sessions: BTreeMap<LocalSessionId, LocalSession>,
    local_dirs: BTreeMap<PathBuf, LocalDir>,
    next_local_session_id: LocalSessionId,
    active_view: Option<ActiveView>,
    status: Option<String>,
    /// 侧栏搜索文本；未命中配置别名时也作为 QuickConnect 目标。
    host_query: String,
    host_focus: FocusHandle,
    /// 主机分组折叠状态；Bank 默认收起，Local/Active 默认展开。
    bank_collapsed: bool,
    active_collapsed: bool,
    projects_collapsed: bool,
    /// 原生项目目录选择器任务，持有到选择结果返回。
    _project_picker: Option<Task<()>>,
    /// 模态文本输入缓冲（密码/口令）。
    prompt_input: String,
    /// 模态输入框焦点。
    modal_focus: FocusHandle,
    /// 上一帧是否有活动模态（用于在弹窗出现时自动聚焦）。
    last_had_prompt: bool,
    /// 当前语言偏好；实际 locale 由 i18n 全局状态维护。
    language_preference: LanguagePreference,
    language_menu_open: bool,
    settings: AppSettings,
    settings_open: bool,
    settings_section: SettingsSection,
    settings_scroll: gpui::ScrollHandle,
    /// 侧栏宽度与拖动状态；只影响布局，不改变导航状态。
    sidebar_width: Rc<Cell<f32>>,
    sidebar_dragging: Rc<Cell<bool>>,
    sidebar_scroll: gpui::ScrollHandle,
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
        let entries = build_entries(&config);
        let settings = i18n::settings(cx);
        let language_preference = settings.language;

        cx.new(|cx| Self {
            config,
            entries,
            pool: ConnectionPool::new(),
            remote_tabs: Vec::new(),
            local_sessions: BTreeMap::new(),
            local_dirs: BTreeMap::new(),
            next_local_session_id: 1,
            active_view: None,
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
            settings,
            settings_open: false,
            settings_section: SettingsSection::General,
            settings_scroll: gpui::ScrollHandle::new(),
            sidebar_width: Rc::new(Cell::new(theme::SIDEBAR_WIDTH)),
            sidebar_dragging: Rc::new(Cell::new(false)),
            sidebar_scroll: gpui::ScrollHandle::new(),
        })
    }

    fn open_host(&mut self, idx: usize, cx: &mut Context<Self>) {
        let entry = match self.entries.get(idx) {
            Some(e) => e.clone(),
            None => return,
        };
        self.open_terminal_target(entry.alias.clone(), entry.alias, cx);
    }

    /// 按别名或 `user@host[:port]` 打开一个终端标签。
    ///
    /// 空认证候选也允许继续：Connection 会在认证失败前向 UI 请求密码，
    /// 这样密码登录主机不会被侧栏提前拦截。
    fn open_terminal_target(&mut self, target: String, alias: String, cx: &mut Context<Self>) {
        self.settings_open = false;
        let resolved = self.config.resolve(&target);
        let methods = default_auth_for(&resolved);
        let host_key = ConnectionPool::key_for(&resolved);

        // 复用或新建连接，开一个终端 channel。
        let conn = self
            .pool
            .acquire(resolved, methods, self.config.clone(), cx);
        let (input_tx, event_rx) = conn.read(cx).open_terminal(100, 30);
        let terminal = TerminalView::from_bridge(input_tx, event_rx, 100, 30, cx);
        self.remote_tabs.push(Tab {
            target,
            alias,
            host_key,
            pane: Pane::Terminal(terminal),
        });
        self.active_view = Some(ActiveView::RemoteTab(self.remote_tabs.len() - 1));
        self.status = None;
        cx.notify();
    }

    fn open_sftp(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.settings_open = false;
        let entry = match self.entries.get(idx) {
            Some(e) => e.clone(),
            None => return,
        };
        let resolved = self.config.resolve(&entry.alias);
        let methods = default_auth_for(&resolved);
        let host_key = ConnectionPool::key_for(&resolved);
        let conn = self
            .pool
            .acquire(resolved.clone(), methods, self.config.clone(), cx);
        let (cmd_tx, event_rx) = conn.read(cx).open_sftp();
        let pane = SftpPane::from_bridge(cmd_tx, event_rx, cx);
        self.remote_tabs.push(Tab {
            target: entry.alias.clone(),
            alias: entry.alias,
            host_key,
            pane: Pane::Sftp(pane),
        });
        self.active_view = Some(ActiveView::RemoteTab(self.remote_tabs.len() - 1));
        cx.notify();
    }

    fn open_forward(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.settings_open = false;
        let entry = match self.entries.get(idx) {
            Some(e) => e.clone(),
            None => return,
        };
        let resolved = self.config.resolve(&entry.alias);
        let methods = default_auth_for(&resolved);
        let host_key = ConnectionPool::key_for(&resolved);
        let conn = self
            .pool
            .acquire(resolved.clone(), methods, self.config.clone(), cx);
        let pane = ForwardPane::new(conn, cx, &resolved);
        self.remote_tabs.push(Tab {
            target: entry.alias.clone(),
            alias: entry.alias,
            host_key,
            pane: Pane::Forward(pane),
        });
        self.active_view = Some(ActiveView::RemoteTab(self.remote_tabs.len() - 1));
        cx.notify();
    }

    /// 在目录 view 中打开一个独立的本地 PTY session。
    fn open_local_session(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        self.settings_open = false;
        let cwd = normalize_local_cwd(cwd);
        let cwd_text = cwd.to_string_lossy().to_string();
        let (input_tx, event_rx) = local::open_terminal(cwd.clone(), 100, 30);
        let terminal =
            TerminalView::from_local_bridge(input_tx, event_rx, 100, 30, cwd_text.clone(), cx);
        let session_id = self.next_local_session_id;
        self.next_local_session_id += 1;
        self.local_sessions
            .insert(session_id, LocalSession { cwd, terminal });
        self.sync_local_dirs(cx);
        self.select_local_session(session_id, cx);
        self.status = None;
        cx.notify();
    }

    fn activate_local_dir(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        let cwd = normalize_local_cwd(cwd);
        self.sync_local_dirs(cx);
        let session_id = self
            .local_dirs
            .get(&cwd)
            .and_then(|dir| dir.active_session.or_else(|| dir.sessions.first().copied()));
        if let Some(session_id) = session_id {
            self.select_local_session(session_id, cx);
        } else {
            self.open_local_session(cwd, cx);
        }
    }

    fn select_local_session(&mut self, session_id: LocalSessionId, cx: &mut Context<Self>) {
        self.settings_open = false;
        let cwd = self
            .local_dirs
            .iter()
            .find(|(_, dir)| dir.sessions.contains(&session_id))
            .map(|(cwd, _)| cwd.clone());
        let Some(cwd) = cwd else { return };
        if let Some(dir) = self.local_dirs.get_mut(&cwd) {
            dir.active_session = Some(session_id);
        }
        self.active_view = Some(ActiveView::LocalSession(session_id));
        self.refocus_active_terminal(cx);
        cx.notify();
    }

    fn close_local_session(&mut self, session_id: LocalSessionId, cx: &mut Context<Self>) {
        let Some(cwd) = self
            .local_dirs
            .iter()
            .find(|(_, dir)| dir.sessions.contains(&session_id))
            .map(|(cwd, _)| cwd.clone())
        else {
            return;
        };
        let was_active = self.active_view == Some(ActiveView::LocalSession(session_id));
        let mut next_session = None;
        let remove_dir = if let Some(dir) = self.local_dirs.get_mut(&cwd) {
            dir.sessions.retain(|id| *id != session_id);
            if dir.active_session == Some(session_id) {
                dir.active_session = dir.sessions.first().copied();
            }
            next_session = dir.active_session;
            dir.sessions.is_empty()
        } else {
            false
        };
        self.local_sessions.remove(&session_id);
        if remove_dir {
            self.local_dirs.remove(&cwd);
        }
        if was_active {
            self.active_view = next_session
                .map(ActiveView::LocalSession)
                .or_else(|| self.first_local_view())
                .or_else(|| {
                    self.remote_tabs
                        .last()
                        .map(|_| ActiveView::RemoteTab(self.remote_tabs.len() - 1))
                });
            self.refocus_active_terminal(cx);
        }
        cx.notify();
    }

    fn switch_tab(&mut self, idx: usize, cx: &mut Context<Self>) {
        match self.active_view {
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

    fn switch_remote_tab(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.remote_tabs.len() {
            return;
        }
        self.settings_open = false;
        self.active_view = Some(ActiveView::RemoteTab(idx));
        self.refocus_active_terminal(cx);
        cx.notify();
    }

    fn close_remote_tab(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.remote_tabs.len() {
            return;
        }
        self.remote_tabs.remove(idx);
        // 移除 Tab → Entity<TerminalView> 释放 → input_tx 断 → relay 结束 →
        // Connection channel 计数减；归 0 则连接自行 disconnect。
        self.active_view = match self.active_view {
            Some(ActiveView::RemoteTab(a)) if a == idx => {
                if self.remote_tabs.is_empty() {
                    self.first_local_view()
                } else if a >= self.remote_tabs.len() {
                    Some(ActiveView::RemoteTab(self.remote_tabs.len() - 1))
                } else {
                    Some(ActiveView::RemoteTab(a))
                }
            }
            Some(ActiveView::RemoteTab(a)) if a > idx => Some(ActiveView::RemoteTab(a - 1)),
            other => other,
        };
        cx.notify();
    }

    fn close_active_tab(&mut self, cx: &mut Context<Self>) {
        match self.active_view {
            Some(ActiveView::RemoteTab(idx)) => self.close_remote_tab(idx, cx),
            Some(ActiveView::LocalSession(session_id)) => self.close_local_session(session_id, cx),
            None => {}
        }
    }

    fn cycle_tab(&mut self, direction: isize, cx: &mut Context<Self>) {
        match self.active_view {
            Some(ActiveView::RemoteTab(current)) => {
                let len = self.remote_tabs.len();
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
    fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_view {
            Some(ActiveView::LocalSession(session_id)) => {
                let cwd = self.local_session_cwd(session_id, cx);
                self.open_local_session(cwd, cx);
                return;
            }
            Some(ActiveView::RemoteTab(idx)) => {
                if let Some(tab) = self.remote_tabs.get(idx) {
                    let target = tab.target.clone();
                    self.open_terminal_target(target.clone(), target, cx);
                    return;
                }
            }
            None => {}
        }
        self.host_query.clear();
        self.host_focus.focus(window, cx);
        cx.notify();
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
            .entries
            .iter()
            .position(|entry| entry.alias.eq_ignore_ascii_case(&query))
            .or_else(|| {
                self.entries
                    .iter()
                    .position(|entry| host_entry_matches(entry, &query_lower))
            });

        if let Some(idx) = matching_idx {
            self.open_host(idx, cx);
        } else {
            self.open_terminal_target(query.clone(), query, cx);
        }
    }

    /// 通过原生目录选择器创建或打开一个本地项目。
    fn choose_project_directory(&mut self, cx: &mut Context<Self>) {
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

    fn handle_host_search_key(
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

    fn handle_shell_key_down(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
            key if matches!(key, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9") => {
                if let Ok(n) = ks.key.parse::<usize>() {
                    self.switch_tab(n - 1, cx);
                }
            }
            _ => {}
        }
    }

    fn toggle_bank_group(&mut self, cx: &mut Context<Self>) {
        self.bank_collapsed = !self.bank_collapsed;
        cx.notify();
    }

    fn toggle_active_group(&mut self, cx: &mut Context<Self>) {
        self.active_collapsed = !self.active_collapsed;
        cx.notify();
    }

    fn toggle_projects_group(&mut self, cx: &mut Context<Self>) {
        self.projects_collapsed = !self.projects_collapsed;
        cx.notify();
    }

    fn toggle_language_menu(&mut self, cx: &mut Context<Self>) {
        self.language_menu_open = !self.language_menu_open;
        self.settings_open = false;
        cx.notify();
    }

    fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_open = !self.settings_open;
        self.language_menu_open = false;
        cx.notify();
    }

    fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_open = false;
        cx.notify();
    }

    fn select_settings_section(&mut self, section: SettingsSection, cx: &mut Context<Self>) {
        self.settings_section = section;
        self.settings_scroll
            .set_offset(gpui::Point::new(px(0.), px(0.)));
        cx.notify();
    }

    fn apply_settings(&mut self, settings: AppSettings, cx: &mut Context<Self>) {
        let settings = settings.normalized();
        if self.settings == settings {
            return;
        }
        let language_changed = self.language_preference != settings.language;
        i18n::set_settings(cx, settings);
        self.settings = settings;
        self.language_preference = settings.language;

        for tab in &self.remote_tabs {
            match &tab.pane {
                Pane::Terminal(terminal) => {
                    terminal.update(cx, |terminal, cx| terminal.apply_settings(settings, cx));
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
        for session in self.local_sessions.values() {
            session
                .terminal
                .update(cx, |terminal, cx| terminal.apply_settings(settings, cx));
        }
        cx.notify();
    }

    fn set_language(&mut self, preference: LanguagePreference, cx: &mut Context<Self>) {
        if self.language_preference == preference {
            self.language_menu_open = false;
            cx.notify();
            return;
        }
        let mut settings = self.settings;
        settings.language = preference;
        self.apply_settings(settings, cx);
        self.language_menu_open = false;
        cx.notify();
    }

    fn toggle_timestamps(&mut self, cx: &mut Context<Self>) {
        let mut settings = self.settings;
        settings.show_timestamps = !settings.show_timestamps;
        self.apply_settings(settings, cx);
    }

    fn adjust_font_size(&mut self, delta: f32, cx: &mut Context<Self>) {
        let mut settings = self.settings;
        settings.terminal_font_size = (settings.terminal_font_size + delta)
            .round()
            .clamp(i18n::MIN_TERMINAL_FONT_SIZE, i18n::MAX_TERMINAL_FONT_SIZE);
        self.apply_settings(settings, cx);
    }

    fn set_scrollback(&mut self, scrollback: usize, cx: &mut Context<Self>) {
        let mut settings = self.settings;
        settings.terminal_scrollback = scrollback;
        self.apply_settings(settings, cx);
    }

    fn sync_local_dirs(&mut self, cx: &Context<Self>) {
        let previous = std::mem::take(&mut self.local_dirs);
        let active_local_session = match self.active_view {
            Some(ActiveView::LocalSession(session_id)) => Some(session_id),
            _ => None,
        };
        let sessions = self
            .local_sessions
            .iter_mut()
            .map(|(&session_id, session)| {
                if let Some(cwd) = session.terminal.read(cx).cwd.as_deref() {
                    session.cwd = normalize_local_cwd(PathBuf::from(cwd));
                }
                (session_id, session.cwd.clone())
            })
            .collect::<Vec<_>>();
        self.local_dirs = rebuild_local_dirs(&previous, sessions, active_local_session);
    }

    fn local_dir_for_session(&self, session_id: LocalSessionId) -> Option<&LocalDir> {
        self.local_dirs
            .iter()
            .find_map(|(_, dir)| dir.sessions.contains(&session_id).then_some(dir))
    }

    fn first_local_view(&self) -> Option<ActiveView> {
        self.local_dirs
            .values()
            .find_map(|dir| dir.active_session.map(ActiveView::LocalSession))
    }

    fn local_session_cwd(&self, session_id: LocalSessionId, cx: &Context<Self>) -> PathBuf {
        self.local_sessions
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

    fn local_cwd_matching_query(&self, query: &str) -> Option<PathBuf> {
        self.local_dirs
            .keys()
            .find(|cwd| cwd.to_string_lossy().to_ascii_lowercase().contains(query))
            .cloned()
    }

    /// 当前有待处理弹窗的连接（若有）。
    fn pending_connection(&self, cx: &Context<Self>) -> Option<Entity<Connection>> {
        self.pool.pending_prompt_connection(cx)
    }

    /// 把焦点交还给当前活动终端 tab（切换 tab / 关闭模态后调用）。
    fn refocus_active_terminal(&self, cx: &mut Context<Self>) {
        match self.active_view {
            Some(ActiveView::RemoteTab(idx)) => {
                if let Some(Tab {
                    pane: Pane::Terminal(terminal),
                    ..
                }) = self.remote_tabs.get(idx)
                {
                    terminal.update(cx, |terminal, _| terminal.request_focus());
                }
            }
            Some(ActiveView::LocalSession(session_id)) => {
                if let Some(session) = self.local_sessions.get(&session_id) {
                    session
                        .terminal
                        .update(cx, |terminal, _| terminal.request_focus());
                }
            }
            None => {}
        }
    }

    /// 回填凭据（None = 取消）。
    fn resolve_credential(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        if let Some(c) = self.pending_connection(cx) {
            c.update(cx, |conn, _| conn.resolve_credential(value));
        }
        self.prompt_input.clear();
        self.refocus_active_terminal(cx);
        cx.notify();
    }

    /// 回填主机密钥决定。
    fn resolve_host_key(&mut self, decision: HostKeyDecision, cx: &mut Context<Self>) {
        if let Some(c) = self.pending_connection(cx) {
            c.update(cx, |conn, _| conn.resolve_host_key(decision));
        }
        self.refocus_active_terminal(cx);
        cx.notify();
    }
}

/// 当前活动模态的显示快照。
enum PromptDisplay {
    None,
    HostKey {
        host: String,
        port: u16,
        key_type: String,
        fingerprint: String,
        changed: bool,
    },
    Credential {
        kind: CredentialKind,
        prompt: String,
    },
}

impl AppShell {
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
        // PTY 的 cwd 会随着 shell 中的 `cd` 改变；每帧同步，保证 session
        // 自动移动到新的 Local 目录 view，同时保留自己的 session id。
        self.sync_local_dirs(cx);
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
            .on_key_down(cx.listener(AppShell::handle_shell_key_down))
            .child(sidebar)
            .child(main);

        if matches!(
            prompt,
            PromptDisplay::HostKey { .. } | PromptDisplay::Credential { .. }
        ) {
            root = root.child(render_prompt_modal(self, prompt, cx));
        }
        root
    }
}

fn render_sidebar(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let query = shell.host_query.trim().to_ascii_lowercase();
    let search_focus = shell.host_focus.clone();
    let search_value = shell.host_query.clone();
    let active_remote_key = match shell.active_view {
        Some(ActiveView::RemoteTab(idx)) => {
            shell.remote_tabs.get(idx).map(|tab| tab.host_key.clone())
        }
        _ => None,
    };
    let project_dirs: Vec<&LocalDir> = shell
        .local_dirs
        .values()
        .filter(|dir| local_dir_matches_query(dir, &query))
        .collect();
    let mut project_name_counts = BTreeMap::new();
    for dir in shell.local_dirs.values() {
        *project_name_counts
            .entry(local_dir_name_key(&dir.cwd))
            .or_insert(0usize) += 1;
    }
    let project_query = matches!(
        query.as_str(),
        "local" | "project" | "projects" | "本地" | "项目"
    );
    let show_projects = query.is_empty() || project_query || !project_dirs.is_empty();

    let mut active_entries = Vec::new();
    let mut bank_entries = Vec::new();
    for (idx, entry) in shell.entries.iter().enumerate() {
        if !query.is_empty() && !host_entry_matches(entry, &query) {
            continue;
        }
        let state = shell.pool.state_for_key(&entry.key, cx);
        let row = (idx, entry.clone(), state);
        if is_active_connection(&row.2) {
            active_entries.push(row);
        } else {
            bank_entries.push(row);
        }
    }

    let active_count = active_entries.len();
    let bank_count = bank_entries.len();
    let project_count = if show_projects { project_dirs.len() } else { 0 };
    let visible_count = active_count + bank_count + project_count;

    let mut active_list = div().id("active-host-list").flex().flex_col().gap_1();
    if active_entries.is_empty() {
        active_list = active_list.child(render_host_group_empty(i18n::text(
            "sidebar.no_active_connections",
        )));
    } else {
        for (idx, entry, state) in active_entries {
            let selected = active_remote_key.as_deref() == Some(entry.key.as_str());
            active_list = active_list.child(render_host_entry(idx, &entry, state, selected, cx));
        }
    }

    let mut bank_list = div().id("bank-host-list").flex().flex_col().gap_1();
    if bank_entries.is_empty() {
        bank_list = bank_list.child(render_host_group_empty(i18n::text(
            "sidebar.no_hosts_in_bank",
        )));
    } else {
        for (idx, entry, state) in bank_entries {
            let selected = active_remote_key.as_deref() == Some(entry.key.as_str());
            bank_list = bank_list.child(render_host_entry(idx, &entry, state, selected, cx));
        }
    }

    let mut project_list = div().id("project-list").flex().flex_col().gap_1();
    if project_dirs.is_empty() {
        project_list =
            project_list.child(render_host_group_empty(i18n::text("sidebar.no_projects")));
    } else {
        for (idx, dir) in project_dirs.iter().enumerate() {
            let selected = is_active_local_dir(shell, dir);
            let duplicate_name = project_name_counts
                .get(&local_dir_name_key(&dir.cwd))
                .is_some_and(|count| *count > 1);
            project_list = project_list.child(render_local_dir(
                idx,
                dir,
                selected,
                duplicate_name,
                shell,
                cx,
            ));
        }
    }

    // Searching should reveal matching hosts even when their group is collapsed.
    let active_collapsed = shell.active_collapsed && query.is_empty();
    let bank_collapsed = shell.bank_collapsed && query.is_empty();
    let active_group = render_host_group(
        "active",
        i18n::text("sidebar.active"),
        active_count,
        active_collapsed,
        active_list.into_any_element(),
        AppShell::toggle_active_group,
        None,
        cx,
    );
    let bank_group = render_host_group(
        "bank",
        i18n::text("sidebar.bank"),
        bank_count,
        bank_collapsed,
        bank_list.into_any_element(),
        AppShell::toggle_bank_group,
        None,
        cx,
    );
    let projects_group = if show_projects {
        Some(render_host_group(
            "projects",
            i18n::text("sidebar.local"),
            project_count,
            shell.projects_collapsed && query.is_empty(),
            project_list.into_any_element(),
            AppShell::toggle_projects_group,
            Some(AppShell::choose_project_directory),
            cx,
        ))
    } else {
        None
    };

    let mut list = div()
        .id("host-list")
        .track_scroll(&shell.sidebar_scroll)
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap_2()
        .px_2()
        .py_2()
        .overflow_y_scroll();
    if let Some(projects_group) = projects_group {
        list = list.child(projects_group);
    }
    list = list.child(active_group).child(bank_group);

    let search_placeholder = if search_value.is_empty() {
        i18n::text("sidebar.search_placeholder")
    } else {
        search_value
    };
    let search = div()
        .id("host-search")
        .mx_2()
        .mb_2()
        .h(px(32.))
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .bg(theme::canvas())
        .border_1()
        .border_color(theme::border())
        .rounded(px(theme::RADIUS_SM))
        .text_xs()
        .text_color(if shell.host_query.is_empty() {
            theme::faint_text()
        } else {
            theme::text()
        })
        .track_focus(&search_focus)
        .focus(|style| style.border_color(theme::focus_ring()))
        .on_click({
            let search_focus = search_focus.clone();
            move |_ev, window, cx| window.focus(&search_focus, cx)
        })
        .on_key_down(cx.listener(AppShell::handle_host_search_key))
        .child(icons::icon(icons::IconName::Search, 14.).text_color(theme::muted_text()))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .child(SharedString::from(search_placeholder)),
        );

    let list_footer = if visible_count == 0 && shell.entries.is_empty() && !show_projects {
        i18n::text("sidebar.no_ssh_hosts")
    } else if visible_count == 0 {
        i18n::text("sidebar.no_matches")
    } else {
        rust_i18n::t!("sidebar.entry_count", count = visible_count).to_string()
    };

    let width = shell
        .sidebar_width
        .get()
        .clamp(theme::SIDEBAR_MIN_WIDTH, theme::SIDEBAR_MAX_WIDTH);
    let container: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
    let backing = canvas(
        {
            let container = container.clone();
            move |bounds, _window, _cx| container.set(Some(bounds))
        },
        {
            let container = container.clone();
            let width_cell = shell.sidebar_width.clone();
            let dragging = shell.sidebar_dragging.clone();
            move |_bounds, _state, window, _cx| {
                window.on_mouse_event({
                    let container = container.clone();
                    let width_cell = width_cell.clone();
                    let dragging = dragging.clone();
                    move |ev: &MouseMoveEvent, _phase, window, _cx| {
                        if !dragging.get() {
                            return;
                        }
                        let Some(bounds) = container.get() else {
                            return;
                        };
                        let width = (ev.position.x - bounds.origin.x)
                            .as_f32()
                            .clamp(theme::SIDEBAR_MIN_WIDTH, theme::SIDEBAR_MAX_WIDTH);
                        width_cell.set(width);
                        window.refresh();
                    }
                });
                window.on_mouse_event({
                    let dragging = dragging.clone();
                    move |_ev: &MouseUpEvent, _phase, window, _cx| {
                        if dragging.replace(false) {
                            window.refresh();
                        }
                    }
                });
            }
        },
    )
    .absolute()
    .size_full();

    let resizing = shell.sidebar_dragging.get();
    let resize_handle = div()
        .id("sidebar-resize")
        .absolute()
        .top_0()
        .right(px(-4.))
        .w(px(8.))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_col_resize()
        .child(
            div()
                .w(px(1.))
                .h_full()
                .bg(if resizing {
                    theme::accent()
                } else {
                    theme::border()
                })
                .hover(|style| style.bg(theme::accent())),
        )
        .on_mouse_down(MouseButton::Left, {
            let dragging = shell.sidebar_dragging.clone();
            move |_ev, window, _cx| {
                dragging.set(true);
                window.refresh();
            }
        });

    let mut titlebar = div()
        .relative()
        .h(px(theme::TITLEBAR_HEIGHT))
        .flex_shrink_0()
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .child(icons::icon(icons::IconName::Terminal, 15.).text_color(theme::accent()))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::text())
                .child(SharedString::from("crossh")),
        )
        .child(div().flex_1())
        .child(
            div()
                .id("language-toggle")
                .h(px(24.))
                .px_2()
                .flex()
                .items_center()
                .gap_1()
                .rounded(px(theme::RADIUS_SM))
                .cursor_pointer()
                .text_xs()
                .text_color(theme::muted_text())
                .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
                .child(SharedString::from(i18n::language_short_label(
                    shell.language_preference.resolve(),
                )))
                .child(icons::icon(icons::IconName::ChevronDown, 11.))
                .on_click(cx.listener(|this, _ev, _window, cx| {
                    this.toggle_language_menu(cx);
                })),
        )
        .child(
            div()
                .id("settings-toggle")
                .w(px(24.))
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::RADIUS_SM))
                .cursor_pointer()
                .text_color(if shell.settings_open {
                    theme::accent()
                } else {
                    theme::muted_text()
                })
                .bg(if shell.settings_open {
                    theme::accent_soft()
                } else {
                    theme::sidebar()
                })
                .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
                .tooltip(|_window, cx| {
                    cx.new(|_| LocalPathTooltip {
                        path: SharedString::from(i18n::text("tooltip.settings")),
                    })
                    .into()
                })
                .child(icons::icon(icons::IconName::Settings, 14.))
                .on_click(cx.listener(|this, _ev, _window, cx| {
                    this.toggle_settings(cx);
                })),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme::faint_text())
                .child(SharedString::from(format!("{visible_count}"))),
        );
    if shell.language_menu_open {
        titlebar = titlebar.child(render_language_menu(shell, cx));
    }

    div()
        .relative()
        .flex_shrink_0()
        .w(px(width))
        .h_full()
        .flex()
        .flex_col()
        .bg(theme::sidebar())
        .border_r_1()
        .border_color(theme::border())
        .child(backing)
        .child(
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(titlebar)
                .child(search)
                .child(list)
                .child(
                    div()
                        .flex_shrink_0()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(theme::border())
                        .text_xs()
                        .text_color(theme::faint_text())
                        .child(SharedString::from(list_footer)),
                ),
        )
        .child(resize_handle)
        .into_any_element()
}

fn render_language_menu(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let current = shell.language_preference;
    let mut menu = div()
        .id("language-menu")
        .absolute()
        .top(px(theme::TITLEBAR_HEIGHT - 2.))
        .right(px(8.))
        .w(px(168.))
        .p_1()
        .flex()
        .flex_col()
        .gap_1()
        .bg(theme::raised())
        .border_1()
        .border_color(theme::border_strong())
        .rounded(px(theme::RADIUS_SM))
        .shadow_md();

    for preference in LanguagePreference::ALL {
        let selected = preference == current;
        let option_id = format!("language-option-{:?}", preference);
        let option = div()
            .id(option_id)
            .h(px(28.))
            .px_2()
            .flex()
            .items_center()
            .rounded(px(theme::RADIUS_SM))
            .cursor_pointer()
            .text_xs()
            .text_color(if selected {
                theme::accent()
            } else {
                theme::text()
            })
            .bg(if selected {
                theme::accent_soft()
            } else {
                theme::raised()
            })
            .hover(|s| s.bg(theme::surface()).text_color(theme::text()))
            .child(SharedString::from(i18n::preference_label(preference)))
            .on_click(cx.listener(move |this, _ev, _window, cx| {
                this.set_language(preference, cx);
            }));
        menu = menu.child(option);
    }
    menu.into_any_element()
}

fn local_dir_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

fn local_dir_name_key(path: &Path) -> String {
    local_dir_name(path).to_ascii_lowercase()
}

fn local_dir_label(path: &Path, duplicate_name: bool) -> String {
    let name = local_dir_name(path);
    if !duplicate_name {
        return name;
    }

    path.parent()
        .and_then(Path::file_name)
        .and_then(|parent| parent.to_str())
        .filter(|parent| !parent.is_empty())
        .map(|parent| format!("{name} · {parent}"))
        .unwrap_or(name)
}

fn local_dir_matches_query(dir: &LocalDir, query: &str) -> bool {
    query.is_empty()
        || matches!(query, "local" | "project" | "projects" | "本地" | "项目")
        || dir
            .cwd
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains(query)
}

fn is_active_local_dir(shell: &AppShell, dir: &LocalDir) -> bool {
    matches!(shell.active_view, Some(ActiveView::LocalSession(session_id)) if dir.sessions.contains(&session_id))
}

fn local_dir_state(shell: &AppShell, dir: &LocalDir, cx: &Context<AppShell>) -> Option<ConnState> {
    dir.sessions
        .iter()
        .filter_map(|id| shell.local_sessions.get(id))
        .map(|session| session.terminal.read(cx).state.clone())
        .reduce(preferred_state)
}

fn render_local_dir(
    idx: usize,
    dir: &LocalDir,
    selected: bool,
    duplicate_name: bool,
    shell: &AppShell,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let cwd = dir.cwd.clone();
    let cwd_for_new = cwd.clone();
    let label = local_dir_label(&cwd, duplicate_name);
    let tooltip_path = SharedString::from(cwd.to_string_lossy().to_string());
    let count = dir.sessions.len();
    let state = local_dir_state(shell, dir, cx);
    let folder_color = if selected {
        theme::accent()
    } else {
        match state {
            Some(ConnState::Connected) => theme::accent(),
            Some(ConnState::Connecting) => theme::warning(),
            Some(ConnState::Error(_)) => theme::danger(),
            _ => theme::muted_text(),
        }
    };
    let mut row = div()
        .id(("local-group", idx))
        .flex_shrink_0()
        .h(px(theme::ROW_HEIGHT))
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(theme::RADIUS_SM))
        .text_sm()
        .cursor_pointer();
    if selected {
        row = row
            .bg(theme::accent_soft())
            .border_l_2()
            .border_color(theme::accent());
    }
    row.hover(|s| s.bg(theme::surface()))
        .tooltip(move |_window, cx| {
            let path = tooltip_path.clone();
            cx.new(|_| LocalPathTooltip { path }).into()
        })
        .on_click(cx.listener(move |this, _ev, _window, cx| {
            this.activate_local_dir(cwd.clone(), cx);
        }))
        .child(icons::icon(icons::IconName::FolderOpen, 15.).text_color(folder_color))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_color(theme::text())
                .child(SharedString::from(label)),
        )
        .child(
            div()
                .min_w(px(18.))
                .text_xs()
                .text_color(theme::muted_text())
                .child(SharedString::from(format!("{count}"))),
        )
        .child(
            div()
                .id(("local-new", idx))
                .w(px(24.))
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::RADIUS_SM))
                .cursor_pointer()
                .text_color(theme::muted_text())
                .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
                .tooltip(|_window, cx| {
                    cx.new(|_| LocalPathTooltip {
                        path: SharedString::from(i18n::text("tooltip.new_terminal")),
                    })
                    .into()
                })
                .child(icons::icon(icons::IconName::Plus, 14.))
                .on_click(cx.listener(move |this, _ev, _window, cx| {
                    cx.stop_propagation();
                    this.open_local_session(cwd_for_new.clone(), cx);
                })),
        )
        .into_any_element()
}

struct LocalPathTooltip {
    path: SharedString,
}

impl Render for LocalPathTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(480.))
            .px_2()
            .py_1()
            .bg(theme::raised())
            .border_1()
            .border_color(theme::border_strong())
            .text_xs()
            .text_color(theme::text())
            .whitespace_normal()
            .child(self.path.clone())
    }
}

fn render_host_entry(
    idx: usize,
    entry: &HostEntry,
    state: Option<ConnState>,
    selected: bool,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let alias = entry.alias.clone();
    let detail = entry.detail.clone();
    let badge = state_badget(&state);

    let mut entry_div = div()
        .id(("host-entry", idx))
        .flex_shrink_0()
        .min_h(px(theme::ROW_HEIGHT))
        .px_2()
        .py_1()
        .rounded(px(theme::RADIUS_SM))
        .text_sm()
        .cursor_pointer();
    if selected {
        entry_div = entry_div
            .bg(theme::accent_soft())
            .border_l_2()
            .border_color(theme::accent());
    }
    entry_div = entry_div
        .hover(|s| s.bg(theme::surface()))
        .on_click(cx.listener(move |this, _ev, _window, cx| {
            this.open_host(idx, cx);
        }))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    icons::icon(icons::IconName::Server, 15.).text_color(if selected {
                        theme::accent()
                    } else {
                        theme::muted_text()
                    }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(theme::text())
                        .child(SharedString::from(alias)),
                )
                .child(
                    div()
                        .id(("sftp-btn", idx))
                        .w(px(24.))
                        .h(px(24.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(theme::RADIUS_SM))
                        .cursor_pointer()
                        .text_color(theme::muted_text())
                        .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
                        .tooltip(|_window, cx| {
                            cx.new(|_| LocalPathTooltip {
                                path: SharedString::from(i18n::text("tooltip.open_sftp")),
                            })
                            .into()
                        })
                        .child(icons::icon(icons::IconName::Folder, 14.))
                        .on_click(cx.listener(move |this, _ev, _w, cx| {
                            cx.stop_propagation();
                            this.open_sftp(idx, cx);
                        })),
                )
                .child(
                    div()
                        .id(("fwd-btn", idx))
                        .w(px(24.))
                        .h(px(24.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(theme::RADIUS_SM))
                        .cursor_pointer()
                        .text_color(theme::muted_text())
                        .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
                        .tooltip(|_window, cx| {
                            cx.new(|_| LocalPathTooltip {
                                path: SharedString::from(i18n::text("tooltip.port_forwarding")),
                            })
                            .into()
                        })
                        .child(icons::icon(icons::IconName::ArrowLeftRight, 14.))
                        .on_click(cx.listener(move |this, _ev, _w, cx| {
                            cx.stop_propagation();
                            this.open_forward(idx, cx);
                        })),
                ),
        )
        .child(
            div()
                .ml(px(23.))
                .text_xs()
                .text_color(match state {
                    Some(ConnState::Connected) => theme::accent(),
                    Some(ConnState::Connecting) => theme::warning(),
                    Some(ConnState::Error(_)) => theme::danger(),
                    _ => theme::faint_text(),
                })
                .child(SharedString::from(format!("{badge}{detail}"))),
        );
    entry_div.into_any_element()
}

fn render_host_group(
    id: &'static str,
    title: String,
    count: usize,
    collapsed: bool,
    children: AnyElement,
    toggle: fn(&mut AppShell, &mut Context<AppShell>),
    action: Option<fn(&mut AppShell, &mut Context<AppShell>)>,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let caret = if collapsed {
        icons::IconName::ChevronRight
    } else {
        icons::IconName::ChevronDown
    };
    let mut header = div()
        .id(format!("host-group-header-{id}"))
        .h(px(28.))
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .text_xs()
        .text_color(theme::muted_text())
        .hover(|s| s.bg(theme::surface()).text_color(theme::text()))
        .on_click(cx.listener(move |this, _ev, _window, cx| toggle(this, cx)))
        .child(icons::icon(caret, 13.).text_color(theme::faint_text()))
        .child(
            div()
                .flex_1()
                .font_weight(FontWeight::MEDIUM)
                .child(SharedString::from(title)),
        )
        .child(
            div()
                .text_color(theme::faint_text())
                .child(SharedString::from(count.to_string())),
        );

    if let Some(action) = action {
        header = header.child(
            div()
                .id(format!("host-group-action-{id}"))
                .w(px(24.))
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::RADIUS_SM))
                .cursor_pointer()
                .text_color(theme::muted_text())
                .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
                .tooltip(|_window, cx| {
                    cx.new(|_| LocalPathTooltip {
                        path: SharedString::from(i18n::text("tooltip.new_project")),
                    })
                    .into()
                })
                .child(icons::icon(icons::IconName::Plus, 14.))
                .on_click(cx.listener(move |this, _ev, _window, cx| {
                    cx.stop_propagation();
                    action(this, cx);
                })),
        );
    }

    let mut group = div()
        .id(format!("host-group-{id}"))
        .flex()
        .flex_col()
        .flex_shrink_0()
        .child(header);
    if !collapsed {
        group = group.child(children);
    }
    group.into_any_element()
}

fn render_host_group_empty(label: String) -> AnyElement {
    div()
        .px_2()
        .py_2()
        .rounded(px(theme::RADIUS_SM))
        .text_xs()
        .text_color(theme::faint_text())
        .child(SharedString::from(label))
        .into_any_element()
}

fn is_active_connection(state: &Option<ConnState>) -> bool {
    matches!(state, Some(ConnState::Connected))
}

fn rebuild_local_dirs(
    previous: &BTreeMap<PathBuf, LocalDir>,
    sessions: impl IntoIterator<Item = (LocalSessionId, PathBuf)>,
    active_local_session: Option<LocalSessionId>,
) -> BTreeMap<PathBuf, LocalDir> {
    let mut next = BTreeMap::new();
    for (session_id, cwd) in sessions {
        next.entry(cwd.clone())
            .or_insert_with(|| LocalDir {
                cwd,
                sessions: Vec::new(),
                active_session: None,
            })
            .sessions
            .push(session_id);
    }

    for (cwd, dir) in &mut next {
        let previous_active = previous.get(cwd).and_then(|old| old.active_session);
        dir.active_session = active_local_session
            .filter(|id| dir.sessions.contains(id))
            .or_else(|| previous_active.filter(|id| dir.sessions.contains(id)))
            .or_else(|| dir.sessions.first().copied());
    }
    next
}

fn preferred_state(left: ConnState, right: ConnState) -> ConnState {
    if state_priority(&right) > state_priority(&left) {
        right
    } else {
        left
    }
}

fn state_priority(state: &ConnState) -> u8 {
    match state {
        ConnState::Connected => 4,
        ConnState::Connecting => 3,
        ConnState::Error(_) => 2,
        ConnState::Closed => 1,
    }
}

/// 连接状态徽标文字。
fn state_badget(state: &Option<ConnState>) -> String {
    match state {
        None => String::new(),
        Some(ConnState::Connecting) => i18n::text("connection.connecting_with_separator"),
        Some(ConnState::Connected) => i18n::text("connection.connected_with_separator"),
        Some(ConnState::Error(_)) => i18n::text("connection.error_with_separator"),
        Some(ConnState::Closed) => i18n::text("connection.closed_with_separator"),
    }
}

fn tab_badge_color(state: &Option<ConnState>) -> gpui::Hsla {
    match state {
        Some(ConnState::Connecting) => hsla(0.13, 0.8, 0.6, 1.),
        Some(ConnState::Connected) => hsla(0.33, 0.7, 0.5, 1.),
        Some(ConnState::Error(_)) => hsla(0., 0.8, 0.55, 1.),
        Some(ConnState::Closed) => hsla(0., 0., 0.35, 1.),
        None => hsla(0., 0., 0.35, 1.),
    }
}

fn tab_label(tab: &Tab) -> String {
    match &tab.pane {
        Pane::Terminal(_) => tab.alias.clone(),
        Pane::Sftp(_) => format!("{} ({})", tab.alias, i18n::text("tab.sftp")),
        Pane::Forward(_) => format!("{} ({})", tab.alias, i18n::text("tab.forward")),
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

fn render_main(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    if shell.settings_open {
        return render_settings_page(shell, cx);
    }

    let mut pane = div()
        .flex_1()
        .min_h_0()
        .h_full()
        .bg(theme::canvas())
        .flex()
        .flex_col();

    // 标签条。
    pane = pane.child(render_tab_strip(shell, cx));

    // 终端/SFTP 区。
    let mut content = div().flex_1().min_h_0().relative();
    let active_pane = match shell.active_view {
        Some(ActiveView::RemoteTab(idx)) => shell.remote_tabs.get(idx).map(|tab| match &tab.pane {
            Pane::Terminal(t) => t.clone().into_any_element(),
            Pane::Sftp(s) => s.clone().into_any_element(),
            Pane::Forward(f) => f.clone().into_any_element(),
        }),
        Some(ActiveView::LocalSession(session_id)) => shell
            .local_sessions
            .get(&session_id)
            .map(|session| session.terminal.clone().into_any_element()),
        None => None,
    };
    if let Some(active_pane) = active_pane {
        content = content.child(active_pane);
    } else {
        content = content.child(render_empty_state(cx));
    }

    if let Some(status) = &shell.status {
        content = content.child(
            div()
                .absolute()
                .bottom_2()
                .left_2()
                .px_3()
                .py_1()
                .rounded(px(theme::RADIUS_SM))
                .border_1()
                .border_color(theme::border_strong())
                .bg(theme::raised())
                .text_xs()
                .text_color(theme::text())
                .child(SharedString::from(status.clone())),
        );
    }
    pane = pane.child(content);
    pane.into_any_element()
}

fn render_empty_state(cx: &mut Context<AppShell>) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(340.))
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .w(px(56.))
                        .h(px(56.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(14.))
                        .bg(theme::accent_soft())
                        .child(
                            icons::icon(icons::IconName::Terminal, 28.).text_color(theme::accent()),
                        ),
                )
                .child(
                    div()
                        .mt_2()
                        .text_lg()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::text())
                        .child(SharedString::from(i18n::text("empty_state.title"))),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme::muted_text())
                        .child(SharedString::from(i18n::text("empty_state.description"))),
                )
                .child(
                    div()
                        .id("empty-new-project")
                        .mt_3()
                        .px_3()
                        .h(px(32.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(theme::RADIUS_SM))
                        .cursor_pointer()
                        .bg(theme::accent())
                        .text_color(theme::canvas())
                        .hover(|style| style.bg(rgb(0x82e3bf)))
                        .child(icons::icon(icons::IconName::FolderOpen, 14.))
                        .child(SharedString::from(i18n::text("project.open")))
                        .on_click(cx.listener(|this, _ev, _window, cx| {
                            this.choose_project_directory(cx);
                        })),
                ),
        )
        .into_any_element()
}

fn render_settings_page(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let section = shell.settings_section;
    let general = div()
        .id("settings-section-general")
        .h(px(32.))
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .text_xs()
        .text_color(if section == SettingsSection::General {
            theme::text()
        } else {
            theme::muted_text()
        })
        .bg(if section == SettingsSection::General {
            theme::accent_soft()
        } else {
            theme::canvas()
        })
        .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
        .child(icons::icon(icons::IconName::Settings, 14.))
        .child(SharedString::from(i18n::text("settings.general")))
        .on_click(cx.listener(|this, _ev, _window, cx| {
            this.select_settings_section(SettingsSection::General, cx);
        }));
    let terminal = div()
        .id("settings-section-terminal")
        .h(px(32.))
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .text_xs()
        .text_color(if section == SettingsSection::Terminal {
            theme::text()
        } else {
            theme::muted_text()
        })
        .bg(if section == SettingsSection::Terminal {
            theme::accent_soft()
        } else {
            theme::canvas()
        })
        .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
        .child(icons::icon(icons::IconName::Terminal, 14.))
        .child(SharedString::from(i18n::text("settings.terminal")))
        .on_click(cx.listener(|this, _ev, _window, cx| {
            this.select_settings_section(SettingsSection::Terminal, cx);
        }));

    let content = match section {
        SettingsSection::General => render_general_settings(shell, cx),
        SettingsSection::Terminal => render_terminal_settings(shell, cx),
    };

    div()
        .id("settings-page")
        .size_full()
        .flex()
        .flex_col()
        .bg(theme::canvas())
        .child(
            div()
                .h(px(theme::TAB_HEIGHT))
                .flex_shrink_0()
                .px_5()
                .flex()
                .items_center()
                .gap_2()
                .bg(theme::surface())
                .border_b_1()
                .border_color(theme::border())
                .child(icons::icon(icons::IconName::Settings, 16.).text_color(theme::accent()))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(SharedString::from(i18n::text("settings.title"))),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .id("settings-close")
                        .w(px(28.))
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(theme::RADIUS_SM))
                        .cursor_pointer()
                        .text_color(theme::muted_text())
                        .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
                        .tooltip(|_window, cx| {
                            cx.new(|_| LocalPathTooltip {
                                path: SharedString::from(i18n::text("settings.close")),
                            })
                            .into()
                        })
                        .child(icons::icon(icons::IconName::X, 14.))
                        .on_click(cx.listener(|this, _ev, _window, cx| {
                            this.close_settings(cx);
                        })),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(
                    div()
                        .w(px(180.))
                        .flex_shrink_0()
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .border_r_1()
                        .border_color(theme::border())
                        .child(general)
                        .child(terminal),
                )
                .child(
                    div()
                        .id("settings-content")
                        .track_scroll(&shell.settings_scroll)
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px_5()
                        .py_4()
                        .child(content),
                ),
        )
        .into_any_element()
}

fn render_general_settings(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let mut languages = div().flex().flex_row().gap_1().flex_wrap().justify_end();
    for preference in LanguagePreference::ALL {
        let selected = preference == shell.language_preference;
        let option = div()
            .id(format!("settings-language-{preference:?}"))
            .h(px(30.))
            .px_2()
            .flex()
            .items_center()
            .rounded(px(theme::RADIUS_SM))
            .cursor_pointer()
            .text_xs()
            .text_color(if selected {
                theme::canvas()
            } else {
                theme::muted_text()
            })
            .bg(if selected {
                theme::accent()
            } else {
                theme::raised()
            })
            .hover(|s| s.bg(theme::accent()).text_color(theme::canvas()))
            .child(SharedString::from(i18n::preference_label(preference)))
            .on_click(cx.listener(move |this, _ev, _window, cx| {
                this.set_language(preference, cx);
            }));
        languages = languages.child(option);
    }

    div()
        .id("settings-general")
        .max_w(px(760.))
        .flex()
        .flex_col()
        .child(settings_heading("settings.general"))
        .child(settings_row(
            i18n::text("settings.language"),
            i18n::text("settings.language_description"),
            languages.into_any_element(),
        ))
        .into_any_element()
}

fn render_terminal_settings(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let mut timestamps = div()
        .id("settings-timestamps-toggle")
        .w(px(42.))
        .h(px(24.))
        .p_1()
        .flex()
        .items_center()
        .rounded_full()
        .cursor_pointer()
        .bg(if shell.settings.show_timestamps {
            theme::accent()
        } else {
            theme::border_strong()
        });
    timestamps = if shell.settings.show_timestamps {
        timestamps.justify_end()
    } else {
        timestamps.justify_start()
    };
    timestamps = timestamps.child(
        div()
            .w(px(18.))
            .h(px(18.))
            .rounded_full()
            .bg(theme::canvas()),
    );
    timestamps = timestamps.on_click(cx.listener(|this, _ev, _window, cx| {
        this.toggle_timestamps(cx);
    }));

    let font_size = shell.settings.terminal_font_size.round() as u32;
    let font_control = div()
        .flex()
        .items_center()
        .gap_1()
        .child(settings_icon_button(
            "settings-font-decrease",
            icons::IconName::Minus,
            i18n::text("settings.font_size"),
            cx.listener(|this, _ev, _window, cx| this.adjust_font_size(-1.0, cx)),
        ))
        .child(
            div()
                .w(px(64.))
                .h(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::RADIUS_SM))
                .bg(theme::raised())
                .text_xs()
                .text_color(theme::text())
                .child(SharedString::from(
                    rust_i18n::t!("settings.pixels", value = font_size).to_string(),
                )),
        )
        .child(settings_icon_button(
            "settings-font-increase",
            icons::IconName::Plus,
            i18n::text("settings.font_size"),
            cx.listener(|this, _ev, _window, cx| this.adjust_font_size(1.0, cx)),
        ));

    let scrollback_values = [500usize, 1000, 5000, 10000];
    let mut scrollback = div().flex().flex_row().gap_1().flex_wrap().justify_end();
    for value in scrollback_values {
        let selected = value == shell.settings.terminal_scrollback;
        let option = div()
            .id(format!("settings-scrollback-{value}"))
            .h(px(30.))
            .px_2()
            .flex()
            .items_center()
            .rounded(px(theme::RADIUS_SM))
            .cursor_pointer()
            .text_xs()
            .text_color(if selected {
                theme::canvas()
            } else {
                theme::muted_text()
            })
            .bg(if selected {
                theme::accent()
            } else {
                theme::raised()
            })
            .hover(|s| s.bg(theme::accent()).text_color(theme::canvas()))
            .child(SharedString::from(
                rust_i18n::t!("settings.lines", value = value).to_string(),
            ))
            .on_click(cx.listener(move |this, _ev, _window, cx| {
                this.set_scrollback(value, cx);
            }));
        scrollback = scrollback.child(option);
    }

    div()
        .id("settings-terminal")
        .max_w(px(760.))
        .flex()
        .flex_col()
        .child(settings_heading("settings.terminal"))
        .child(settings_row(
            i18n::text("settings.timestamps"),
            i18n::text("settings.timestamps_description"),
            timestamps.into_any_element(),
        ))
        .child(settings_row(
            i18n::text("settings.font_size"),
            i18n::text("settings.font_size_description"),
            font_control.into_any_element(),
        ))
        .child(settings_row(
            i18n::text("settings.scrollback"),
            i18n::text("settings.scrollback_description"),
            scrollback.into_any_element(),
        ))
        .into_any_element()
}

fn settings_heading(key: &str) -> AnyElement {
    div()
        .pb_2()
        .text_lg()
        .font_weight(FontWeight::MEDIUM)
        .child(SharedString::from(i18n::text(key)))
        .into_any_element()
}

fn settings_row(label: String, description: String, control: AnyElement) -> AnyElement {
    div()
        .w_full()
        .py_4()
        .flex()
        .items_center()
        .gap_4()
        .border_b_1()
        .border_color(theme::border())
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .text_color(theme::text())
                        .child(SharedString::from(label)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::muted_text())
                        .child(SharedString::from(description)),
                ),
        )
        .child(control)
        .into_any_element()
}

fn settings_icon_button(
    id: &'static str,
    icon: icons::IconName,
    tooltip: String,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .w(px(30.))
        .h(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .bg(theme::raised())
        .text_color(theme::muted_text())
        .hover(|s| s.bg(theme::accent()).text_color(theme::canvas()))
        .tooltip(move |_window, cx| {
            cx.new(|_| LocalPathTooltip {
                path: SharedString::from(tooltip.clone()),
            })
            .into()
        })
        .child(icons::icon(icon, 14.))
        .on_click(on_click)
        .into_any_element()
}

fn render_tab_strip(shell: &AppShell, cx: &mut Context<AppShell>) -> impl IntoElement {
    let mut strip = div()
        .flex()
        .flex_row()
        .h(px(theme::TAB_HEIGHT))
        .px_2()
        .gap_1()
        .items_center()
        .bg(theme::surface())
        .border_b_1()
        .border_color(theme::border());

    match shell.active_view {
        Some(ActiveView::RemoteTab(active_idx)) => {
            for idx in 0..shell.remote_tabs.len() {
                let tab = &shell.remote_tabs[idx];
                let is_active = active_idx == idx;
                let state = shell.pool.state_for_key(&tab.host_key, cx);
                let alias = tab_label(tab);
                // 容器不绑定 click；标签名与关闭按钮分别绑定，避免事件叠加。
                let mut container = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h(px(28.))
                    .px_1()
                    .rounded(px(theme::RADIUS_SM));
                if is_active {
                    container = container.bg(theme::accent_soft());
                } else {
                    container = container.hover(|style| style.bg(theme::raised()));
                }
                let container = container
                    .child(
                        div()
                            .id(("remote-tab", idx))
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .py_1()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(theme::text())
                            .hover(|s| s.text_color(theme::accent()))
                            .child(
                                div()
                                    .w(px(6.))
                                    .h(px(6.))
                                    .rounded_full()
                                    .bg(tab_badge_color(&state)),
                            )
                            .child(SharedString::from(alias))
                            .on_click(cx.listener(move |this, _ev, _w, cx| {
                                this.switch_remote_tab(idx, cx);
                            })),
                    )
                    .child(
                        div()
                            .id(("remote-tab-close", idx))
                            .w(px(24.))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(theme::RADIUS_SM))
                            .cursor_pointer()
                            .text_color(theme::muted_text())
                            .hover(|s| s.bg(theme::raised()).text_color(theme::danger()))
                            .tooltip(|_window, cx| {
                                cx.new(|_| LocalPathTooltip {
                                    path: SharedString::from(i18n::text("tooltip.close_tab")),
                                })
                                .into()
                            })
                            .child(icons::icon(icons::IconName::X, 13.))
                            .on_click(cx.listener(move |this, _ev, _w, cx| {
                                this.close_remote_tab(idx, cx);
                            })),
                    );
                strip = strip.child(container);
            }
        }
        Some(ActiveView::LocalSession(active_session_id)) => {
            let session_ids = shell
                .local_dir_for_session(active_session_id)
                .map(|dir| dir.sessions.clone())
                .unwrap_or_default();
            for (idx, session_id) in session_ids.iter().copied().enumerate() {
                let is_active = active_session_id == session_id;
                let state = shell
                    .local_sessions
                    .get(&session_id)
                    .map(|session| session.terminal.read(cx).state.clone());
                let label = format!("ses{}", idx + 1);
                let mut container = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h(px(28.))
                    .px_1()
                    .rounded(px(theme::RADIUS_SM));
                if is_active {
                    container = container.bg(theme::accent_soft());
                } else {
                    container = container.hover(|style| style.bg(theme::raised()));
                }
                let container = container
                    .child(
                        div()
                            .id(("local-tab", session_id))
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .py_1()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(theme::text())
                            .hover(|s| s.text_color(theme::accent()))
                            .child(
                                div()
                                    .w(px(6.))
                                    .h(px(6.))
                                    .rounded_full()
                                    .bg(tab_badge_color(&state)),
                            )
                            .child(SharedString::from(label))
                            .on_click(cx.listener(move |this, _ev, _w, cx| {
                                this.select_local_session(session_id, cx);
                            })),
                    )
                    .child(
                        div()
                            .id(("local-tab-close", session_id))
                            .w(px(24.))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(theme::RADIUS_SM))
                            .cursor_pointer()
                            .text_color(theme::muted_text())
                            .hover(|s| s.bg(theme::raised()).text_color(theme::danger()))
                            .tooltip(|_window, cx| {
                                cx.new(|_| LocalPathTooltip {
                                    path: SharedString::from(i18n::text("tooltip.close_tab")),
                                })
                                .into()
                            })
                            .child(icons::icon(icons::IconName::X, 13.))
                            .on_click(cx.listener(move |this, _ev, _w, cx| {
                                this.close_local_session(session_id, cx);
                            })),
                    );
                strip = strip.child(container);
            }
        }
        None => {}
    }

    strip.child(div().flex_1()).child(
        div()
            .id("new-tab")
            .flex_shrink_0()
            .w(px(28.))
            .h(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(theme::RADIUS_SM))
            .cursor_pointer()
            .bg(theme::accent_soft())
            .border_1()
            .border_color(theme::border_strong())
            .text_color(theme::accent())
            .hover(|s| {
                s.bg(theme::accent())
                    .border_color(theme::accent())
                    .text_color(theme::canvas())
            })
            .tooltip(|_window, cx| {
                cx.new(|_| LocalPathTooltip {
                    path: SharedString::from(i18n::text("tooltip.new_terminal")),
                })
                .into()
            })
            .child(icons::icon(icons::IconName::Plus, 15.))
            .on_click(cx.listener(|this, _ev, window, cx| {
                this.new_tab(window, cx);
            })),
    )
}

/// 渲染模态覆盖层。
fn render_prompt_modal(
    shell: &mut AppShell,
    prompt: PromptDisplay,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let modal_focus = shell.modal_focus.clone();
    let modal_icon = match &prompt {
        PromptDisplay::HostKey { .. } => icons::IconName::ShieldAlert,
        PromptDisplay::Credential { .. } => icons::IconName::KeyRound,
        PromptDisplay::None => return div().into_any_element(),
    };

    let (title, body, is_credential): (String, String, bool) = match &prompt {
        PromptDisplay::HostKey {
            host,
            port,
            key_type,
            fingerprint,
            changed,
        } => {
            let warning = if *changed {
                i18n::text("prompt.changed_host_key_warning")
            } else {
                i18n::text("prompt.unknown_host_warning")
            };
            (
                i18n::text("prompt.host_key_title"),
                rust_i18n::t!(
                    "prompt.host_key_body",
                    warning = warning,
                    host = host,
                    port = port,
                    key_type = key_type,
                    fingerprint = fingerprint
                )
                .to_string(),
                false,
            )
        }
        PromptDisplay::Credential { kind, prompt } => {
            let title = match kind {
                CredentialKind::Passphrase => i18n::text("prompt.passphrase_title"),
                CredentialKind::Password => i18n::text("prompt.password_title"),
            };
            (title, prompt.clone(), true)
        }
        PromptDisplay::None => unreachable!(),
    };

    let mut buttons = div().flex().flex_row().gap_2().mt_4();
    match prompt {
        PromptDisplay::HostKey { changed, .. } => {
            if !changed {
                buttons = buttons
                    .child(host_key_button(
                        shell,
                        cx,
                        i18n::text("prompt.accept_once"),
                        HostKeyDecision::AcceptOnce,
                    ))
                    .child(host_key_button(
                        shell,
                        cx,
                        i18n::text("prompt.accept_always"),
                        HostKeyDecision::AcceptAlways,
                    ));
            }
            buttons = buttons.child(host_key_button(
                shell,
                cx,
                i18n::text("prompt.reject"),
                HostKeyDecision::Reject,
            ));
        }
        PromptDisplay::Credential { .. } => {
            buttons = buttons
                .child(cred_button(shell, cx, i18n::text("prompt.confirm"), true))
                .child(cred_button(shell, cx, i18n::text("prompt.cancel"), false));
        }
        PromptDisplay::None => {}
    }

    let mut card = div()
        .w(px(440.))
        .p_5()
        .bg(theme::surface())
        .border_1()
        .border_color(theme::border_strong())
        .rounded(px(theme::RADIUS_MD))
        .shadow_md()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::text())
                .child(icons::icon(modal_icon, 17.).text_color(if is_credential {
                    theme::info()
                } else {
                    theme::warning()
                }))
                .child(SharedString::from(title)),
        )
        .child(
            div()
                .mt_2()
                .text_xs()
                .text_color(theme::muted_text())
                .child(SharedString::from(body)),
        );

    if is_credential {
        let masked = "•".repeat(shell.prompt_input.chars().count());
        let input = div()
            .id("prompt-input")
            .w_full()
            .h(px(34.))
            .px_3()
            .flex()
            .items_center()
            .mt_2()
            .bg(theme::canvas())
            .border_1()
            .border_color(theme::border_strong())
            .rounded(px(theme::RADIUS_SM))
            .text_sm()
            .text_color(theme::text())
            .track_focus(&modal_focus)
            .on_key_down(cx.listener(handle_credential_key))
            .child(SharedString::from(masked));
        card = card.child(input);
    }
    card = card.child(buttons);

    div()
        .absolute()
        .size_full()
        .top_0()
        .left_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme::scrim())
        .child(card)
        .into_any_element()
}

fn handle_credential_key(
    this: &mut AppShell,
    ev: &KeyDownEvent,
    _: &mut Window,
    cx: &mut Context<AppShell>,
) {
    let ks = &ev.keystroke;
    match ks.key.as_str() {
        "enter" | "return" => {
            let val = std::mem::take(&mut this.prompt_input);
            this.resolve_credential(Some(val), cx);
        }
        "escape" => {
            this.resolve_credential(None, cx);
        }
        "backspace" => {
            this.prompt_input.pop();
            cx.notify();
        }
        _ => {
            if let Some(ch) = printable_char(ks) {
                this.prompt_input.push(ch);
                cx.notify();
            }
        }
    }
}

fn printable_char(ks: &Keystroke) -> Option<char> {
    if ks.modifiers.control || ks.modifiers.platform {
        return None;
    }
    ks.key_char.as_ref().and_then(|s| s.chars().next())
}

fn host_key_button(
    shell: &mut AppShell,
    cx: &mut Context<AppShell>,
    label: String,
    decision: HostKeyDecision,
) -> impl IntoElement {
    let id = SharedString::from(label.clone());
    let icon = match decision {
        HostKeyDecision::Reject => icons::IconName::CircleX,
        HostKeyDecision::AcceptOnce | HostKeyDecision::AcceptAlways => icons::IconName::Check,
    };
    let _ = shell;
    div()
        .id(id)
        .h(px(30.))
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(theme::RADIUS_SM))
        .text_xs()
        .cursor_pointer()
        .bg(if matches!(decision, HostKeyDecision::Reject) {
            theme::raised()
        } else {
            theme::accent()
        })
        .hover(|s| s.bg(theme::border_strong()))
        .text_color(if matches!(decision, HostKeyDecision::Reject) {
            theme::text()
        } else {
            theme::canvas()
        })
        .child(icons::icon(icon, 14.))
        .child(SharedString::from(label))
        .on_click(cx.listener(move |this, _ev, _w, cx| {
            this.resolve_host_key(decision, cx);
        }))
}

fn cred_button(
    shell: &mut AppShell,
    cx: &mut Context<AppShell>,
    label: String,
    submit: bool,
) -> impl IntoElement {
    let id = SharedString::from(label.clone());
    let _ = shell;
    div()
        .id(id)
        .h(px(30.))
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(theme::RADIUS_SM))
        .text_xs()
        .cursor_pointer()
        .bg(if submit {
            theme::accent()
        } else {
            theme::raised()
        })
        .hover(|s| s.bg(theme::border_strong()))
        .text_color(if submit {
            theme::canvas()
        } else {
            theme::text()
        })
        .child(icons::icon(
            if submit {
                icons::IconName::Check
            } else {
                icons::IconName::X
            },
            14.,
        ))
        .child(SharedString::from(label))
        .on_click(cx.listener(move |this, _ev, _w, cx| {
            if submit {
                let val = std::mem::take(&mut this.prompt_input);
                this.resolve_credential(Some(val), cx);
            } else {
                this.resolve_credential(None, cx);
            }
        }))
}

/// 构建主机列表条目，过滤掉纯通配的默认块（如 `Host *`），并解析出池键。
fn build_entries(config: &SshConfig) -> Vec<HostEntry> {
    let mut out = Vec::new();
    for h in config.hosts() {
        let alias = h.alias().to_string();
        if alias == "*" || alias.starts_with('!') {
            continue;
        }
        // resolve 以合并默认块（User/Port 等），得到准确池键与详情。
        let resolved = config.resolve(&alias);
        let detail = format!(
            "{}@{}:{}",
            resolved.user.as_deref().unwrap_or(""),
            resolved.effective_host(),
            resolved.effective_port()
        );
        let key = ConnectionPool::key_for(&resolved);
        out.push(HostEntry { alias, detail, key });
    }
    out
}

fn host_entry_matches(entry: &HostEntry, query: &str) -> bool {
    entry.alias.to_ascii_lowercase().contains(query)
        || entry.detail.to_ascii_lowercase().contains(query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_connected_hosts_are_active() {
        assert!(is_active_connection(&Some(ConnState::Connected)));
        assert!(!is_active_connection(&Some(ConnState::Connecting)));
        assert!(!is_active_connection(&Some(ConnState::Closed)));
        assert!(!is_active_connection(&Some(ConnState::Error(
            "failed".to_string()
        ))));
        assert!(!is_active_connection(&None));
    }

    #[test]
    fn project_search_matches_directory_view() {
        let dir = LocalDir {
            cwd: PathBuf::from("/Users/me/projects/crossh"),
            sessions: vec![1, 2],
            active_session: Some(1),
        };
        assert!(local_dir_matches_query(&dir, ""));
        assert!(local_dir_matches_query(&dir, "local"));
        assert!(local_dir_matches_query(&dir, "project"));
        assert!(local_dir_matches_query(&dir, "projects"));
        assert!(local_dir_matches_query(&dir, "projects/crossh"));
        assert!(!local_dir_matches_query(&dir, "unrelated"));
    }

    #[test]
    fn project_labels_prefer_directory_name_and_disambiguate_duplicates() {
        let path = Path::new("/Users/me/Code/crossh");
        assert_eq!(local_dir_name(path), "crossh");
        assert_eq!(
            local_dir_name_key(Path::new("/Users/me/Code/Crossh")),
            "crossh"
        );
        assert_eq!(local_dir_label(path, false), "crossh");
        assert_eq!(local_dir_label(path, true), "crossh · Code");
        assert_eq!(local_dir_label(Path::new("/"), true), "/");
    }

    #[test]
    fn project_directories_keep_sessions_isolated() {
        let previous = BTreeMap::from([
            (
                PathBuf::from("/Users/me/one"),
                LocalDir {
                    cwd: PathBuf::from("/Users/me/one"),
                    sessions: vec![1, 2],
                    active_session: Some(2),
                },
            ),
            (
                PathBuf::from("/Users/me/two"),
                LocalDir {
                    cwd: PathBuf::from("/Users/me/two"),
                    sessions: vec![3],
                    active_session: Some(3),
                },
            ),
        ]);
        let current = vec![
            (1, PathBuf::from("/Users/me/one")),
            (2, PathBuf::from("/Users/me/two")),
            (3, PathBuf::from("/Users/me/two")),
        ];

        let dirs = rebuild_local_dirs(&previous, current, Some(2));
        assert_eq!(dirs[&PathBuf::from("/Users/me/one")].sessions, vec![1]);
        assert_eq!(
            dirs[&PathBuf::from("/Users/me/one")].active_session,
            Some(1)
        );
        assert_eq!(dirs[&PathBuf::from("/Users/me/two")].sessions, vec![2, 3]);
        assert_eq!(
            dirs[&PathBuf::from("/Users/me/two")].active_session,
            Some(2)
        );
    }

    #[test]
    fn project_group_prefers_a_live_session_state() {
        assert_eq!(
            preferred_state(ConnState::Closed, ConnState::Connecting),
            ConnState::Connecting
        );
        assert_eq!(
            preferred_state(ConnState::Connecting, ConnState::Connected),
            ConnState::Connected
        );
        assert_eq!(
            preferred_state(ConnState::Connected, ConnState::Error("failed".into())),
            ConnState::Connected
        );
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
            ..Default::default()
        },
        |_window, cx| AppShell::new(cx),
    )
    .expect("Failed to open window");
    cx.activate(true);
}

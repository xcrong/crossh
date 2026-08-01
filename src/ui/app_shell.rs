//! 应用外壳：左侧主机列表 + 顶部标签条 + 终端工作区 + 模态弹窗。
//!
//! - 连接池：同主机复用一条已认证会话（开新终端 channel），全部终端关闭才断开。
//! - 多标签：每点击主机新开一个终端标签；可切换/关闭。
//! - sidebar：按连接状态分为可折叠的 Active 与 Bank 主机组。
//! - 模态：池中任一连接出现 pending_prompt（未知主机密钥/凭据）时弹覆盖层。

use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext, Context, Entity, FocusHandle, FontWeight, InteractiveElement,
    IntoElement, Keystroke, KeyDownEvent, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, TitlebarOptions, Window, WindowBounds, WindowOptions,
    div, hsla, px, rgb, size,
};

use crate::config::SshConfig;
use crate::ssh::{
    default_auth_for, Connection, ConnectionPool, CredentialKind, HostKeyDecision, PendingPrompt,
};
use crate::ui::terminal_view::ConnState;
use crate::ui::{ForwardPane, SftpPane, TerminalView};

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

/// 一个终端/SFTP 标签。
struct Tab {
    /// 重新打开终端时使用的原始目标（别名或 user@host:port）。
    target: String,
    alias: String,
    host_key: String,
    pane: Pane,
}

pub struct AppShell {
    config: Arc<SshConfig>,
    entries: Vec<HostEntry>,
    pool: ConnectionPool,
    tabs: Vec<Tab>,
    active_tab: Option<usize>,
    status: Option<String>,
    /// 侧栏搜索文本；未命中配置别名时也作为 QuickConnect 目标。
    host_query: String,
    host_focus: FocusHandle,
    /// 主机分组折叠状态；Bank 默认收起，Active 默认展开。
    bank_collapsed: bool,
    active_collapsed: bool,
    /// 模态文本输入缓冲（密码/口令）。
    prompt_input: String,
    /// 模态输入框焦点。
    modal_focus: FocusHandle,
    /// 上一帧是否有活动模态（用于在弹窗出现时自动聚焦）。
    last_had_prompt: bool,
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

        cx.new(|cx| Self {
            config,
            entries,
            pool: ConnectionPool::new(),
            tabs: Vec::new(),
            active_tab: None,
            status: None,
            host_query: String::new(),
            host_focus: cx.focus_handle(),
            bank_collapsed: true,
            active_collapsed: false,
            prompt_input: String::new(),
            modal_focus: cx.focus_handle(),
            last_had_prompt: false,
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
        let resolved = self.config.resolve(&target);
        let methods = default_auth_for(&resolved);
        let host_key = ConnectionPool::key_for(&resolved);

        // 复用或新建连接，开一个终端 channel。
        let conn = self.pool.acquire(resolved, methods, self.config.clone(), cx);
        let (input_tx, event_rx) = conn.read(cx).open_terminal(100, 30);
        let terminal = TerminalView::from_bridge(input_tx, event_rx, 100, 30, cx);
        self.tabs.push(Tab {
            target,
            alias,
            host_key,
            pane: Pane::Terminal(terminal),
        });
        self.active_tab = Some(self.tabs.len() - 1);
        self.status = None;
        cx.notify();
    }

    fn open_sftp(&mut self, idx: usize, cx: &mut Context<Self>) {
        let entry = match self.entries.get(idx) {
            Some(e) => e.clone(),
            None => return,
        };
        let resolved = self.config.resolve(&entry.alias);
        let methods = default_auth_for(&resolved);
        let host_key = ConnectionPool::key_for(&resolved);
        let conn = self.pool.acquire(resolved.clone(), methods, self.config.clone(), cx);
        let (cmd_tx, event_rx) = conn.read(cx).open_sftp();
        let pane = SftpPane::from_bridge(cmd_tx, event_rx, cx);
        self.tabs.push(Tab {
            target: entry.alias.clone(),
            alias: format!("{} (SFTP)", entry.alias),
            host_key,
            pane: Pane::Sftp(pane),
        });
        self.active_tab = Some(self.tabs.len() - 1);
        cx.notify();
    }

    fn open_forward(&mut self, idx: usize, cx: &mut Context<Self>) {
        let entry = match self.entries.get(idx) {
            Some(e) => e.clone(),
            None => return,
        };
        let resolved = self.config.resolve(&entry.alias);
        let methods = default_auth_for(&resolved);
        let host_key = ConnectionPool::key_for(&resolved);
        let conn = self.pool.acquire(resolved.clone(), methods, self.config.clone(), cx);
        let pane = ForwardPane::new(conn, cx, &resolved);
        self.tabs.push(Tab {
            target: entry.alias.clone(),
            alias: format!("{} (转发)", entry.alias),
            host_key,
            pane: Pane::Forward(pane),
        });
        self.active_tab = Some(self.tabs.len() - 1);
        cx.notify();
    }

    fn switch_tab(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx < self.tabs.len() {
            self.active_tab = Some(idx);
            self.refocus_active_terminal(cx);
            cx.notify();
        }
    }

    fn close_tab(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.tabs.len() {
            return;
        }
        self.tabs.remove(idx);
        // 移除 Tab → Entity<TerminalView> 释放 → input_tx 断 → relay 结束 →
        // Connection channel 计数减；归 0 则连接自行 disconnect。
        self.active_tab = match self.active_tab {
            None => None,
            Some(a) if a == idx => {
                if self.tabs.is_empty() {
                    None
                } else if a >= self.tabs.len() {
                    Some(self.tabs.len() - 1)
                } else {
                    Some(a)
                }
            }
            Some(a) if a > idx => Some(a - 1),
            other => other,
        };
        cx.notify();
    }

    fn close_active_tab(&mut self, cx: &mut Context<Self>) {
        if let Some(idx) = self.active_tab {
            self.close_tab(idx, cx);
        }
    }

    fn cycle_tab(&mut self, direction: isize, cx: &mut Context<Self>) {
        let len = self.tabs.len();
        if len == 0 {
            return;
        }
        let current = self.active_tab.unwrap_or(0) as isize;
        let next = (current + direction).rem_euclid(len as isize) as usize;
        self.switch_tab(next, cx);
    }

    /// 从当前标签复制一个终端标签；没有活动标签时把焦点放到快速连接框。
    fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(idx) = self.active_tab {
            if let Some(tab) = self.tabs.get(idx) {
                let target = tab.target.clone();
                self.open_terminal_target(target.clone(), target, cx);
                return;
            }
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

    /// 当前有待处理弹窗的连接（若有）。
    fn pending_connection(&self, cx: &Context<Self>) -> Option<Entity<Connection>> {
        self.pool.pending_prompt_connection(cx)
    }

    /// 把焦点交还给当前活动终端 tab（切换 tab / 关闭模态后调用）。
    fn refocus_active_terminal(&self, cx: &mut Context<Self>) {
        if let Some(idx) = self.active_tab {
            if let Some(tab) = self.tabs.get(idx) {
                if let Pane::Terminal(t) = &tab.pane {
                    t.update(cx, |t, _| t.request_focus());
                }
            }
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
        let main = render_main(self, cx).into_any_element();

        let mut root = div()
            .id("app-shell")
            .flex()
            .flex_row()
            .size_full()
            .bg(rgb(0x121214))
            .text_color(rgb(0xe6e6e6))
            .on_key_down(cx.listener(AppShell::handle_shell_key_down))
            .child(sidebar)
            .child(main);

        if matches!(prompt, PromptDisplay::HostKey { .. } | PromptDisplay::Credential { .. }) {
            root = root.child(render_prompt_modal(self, prompt, cx));
        }
        root
    }
}

fn render_sidebar(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let query = shell.host_query.trim().to_ascii_lowercase();
    let search_focus = shell.host_focus.clone();
    let search_value = shell.host_query.clone();
    let active_tab_key = shell
        .active_tab
        .and_then(|active_idx| shell.tabs.get(active_idx))
        .map(|tab| tab.host_key.clone());

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
    let visible_count = active_count + bank_count;

    let mut active_list = div()
        .id("active-host-list")
        .flex()
        .flex_col()
        .gap_1();
    if active_entries.is_empty() {
        active_list = active_list.child(render_host_group_empty("No active connections"));
    } else {
        for (idx, entry, state) in active_entries {
            let selected = active_tab_key.as_deref() == Some(entry.key.as_str());
            active_list = active_list.child(render_host_entry(
                idx, &entry, state, selected, cx,
            ));
        }
    }

    let mut bank_list = div().id("bank-host-list").flex().flex_col().gap_1();
    if bank_entries.is_empty() {
        bank_list = bank_list.child(render_host_group_empty("No hosts in bank"));
    } else {
        for (idx, entry, state) in bank_entries {
            let selected = active_tab_key.as_deref() == Some(entry.key.as_str());
            bank_list = bank_list.child(render_host_entry(idx, &entry, state, selected, cx));
        }
    }

    // Searching should reveal matching hosts even when their group is collapsed.
    let active_collapsed = shell.active_collapsed && query.is_empty();
    let bank_collapsed = shell.bank_collapsed && query.is_empty();
    let active_group = render_host_group(
        "active",
        "ACTIVE",
        active_count,
        active_collapsed,
        active_list.into_any_element(),
        AppShell::toggle_active_group,
        cx,
    );
    let bank_group = render_host_group(
        "bank",
        "BANK",
        bank_count,
        bank_collapsed,
        bank_list.into_any_element(),
        AppShell::toggle_bank_group,
        cx,
    );

    let list = div()
        .id("host-list")
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap_1()
        .py_2()
        .overflow_y_scroll()
        .child(active_group)
        .child(bank_group);

    let search = div()
        .id("host-search")
        .mx_3()
        .mb_2()
        .px_2()
        .py_1()
        .flex()
        .items_center()
        .bg(rgb(0x121214))
        .border_1()
        .border_color(rgb(0x303036))
        .text_xs()
        .text_color(if search_value.is_empty() {
            rgb(0x6a6a72)
        } else {
            rgb(0xe6e6e8)
        })
        .track_focus(&search_focus)
        .on_click({
            let search_focus = search_focus.clone();
            move |_ev, window, cx| window.focus(&search_focus, cx)
        })
        .on_key_down(cx.listener(AppShell::handle_host_search_key))
        .child(SharedString::from(if search_value.is_empty() {
            "筛选主机或输入 user@host:port".to_string()
        } else {
            search_value
        }));

    let list_footer = if shell.entries.is_empty() {
        "未找到 ~/.ssh/config 中的主机".to_string()
    } else if visible_count == 0 {
        "没有匹配的主机，按 Enter 进行快速连接".to_string()
    } else {
        format!("{} 个主机", visible_count)
    };

    div()
        .w(px(220.))
        .h_full()
        .flex()
        .flex_col()
        .bg(rgb(0x1a1a1d))
        .border_r_1()
        .border_color(rgb(0x2a2a2e))
        .child(
            div()
                .px_4()
                .pt_3()
                .pb_2()
                .flex()
                .items_center()
                .justify_between()
                .text_xs()
                .text_color(rgb(0x8c8c94))
                .child(SharedString::from("Hosts"))
                .child(SharedString::from(format!("{visible_count}"))),
        )
        .child(search)
        .child(list)
        .child(
            div()
                .px_3()
                .py_2()
                .text_xs()
                .text_color(rgb(0x6a6a72))
                .child(SharedString::from(list_footer)),
        )
        .into_any_element()
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
        .px_3()
        .py_1()
        .text_sm()
        .cursor_pointer();
    if selected {
        entry_div = entry_div.bg(rgb(0x2a2a3a));
    }
    entry_div = entry_div
        .hover(|s| s.bg(rgb(0x232327)))
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
                    div()
                        .w(px(8.))
                        .h(px(8.))
                        .rounded_full()
                        .bg(badge_color(&state)),
                )
                .child(
                    div()
                        .flex_1()
                        .text_color(rgb(0xf5f5f7))
                        .child(SharedString::from(alias)),
                )
                .child(
                    div()
                        .id(("sftp-btn", idx))
                        .px_1()
                        .cursor_pointer()
                        .text_xs()
                        .text_color(rgb(0x888892))
                        .hover(|s| s.text_color(rgb(0xe6e6e6)))
                        .child(SharedString::from("📁"))
                        .on_click(cx.listener(move |this, _ev, _w, cx| {
                            this.open_sftp(idx, cx);
                        })),
                )
                .child(
                    div()
                        .id(("fwd-btn", idx))
                        .px_1()
                        .cursor_pointer()
                        .text_xs()
                        .text_color(rgb(0x888892))
                        .hover(|s| s.text_color(rgb(0xe6e6e6)))
                        .child(SharedString::from("⇄"))
                        .on_click(cx.listener(move |this, _ev, _w, cx| {
                            this.open_forward(idx, cx);
                        })),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(0x888892))
                .child(SharedString::from(format!("{badge}{detail}"))),
        );
    entry_div.into_any_element()
}

fn render_host_group(
    id: &'static str,
    title: &'static str,
    count: usize,
    collapsed: bool,
    children: AnyElement,
    toggle: fn(&mut AppShell, &mut Context<AppShell>),
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let caret = if collapsed { "▸" } else { "▾" };
    let header = div()
        .id(format!("host-group-header-{id}"))
        .px_3()
        .py_1()
        .flex()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .text_xs()
        .text_color(rgb(0x8c8c94))
        .hover(|s| s.bg(rgb(0x232327)).text_color(rgb(0xe6e6e6)))
        .on_click(cx.listener(move |this, _ev, _window, cx| toggle(this, cx)))
        .child(
            div()
                .w(px(10.))
                .text_color(rgb(0x6a6a72))
                .child(SharedString::from(caret)),
        )
        .child(
            div()
                .flex_1()
                .child(SharedString::from(title)),
        )
        .child(
            div()
                .text_color(rgb(0x6a6a72))
                .child(SharedString::from(count.to_string())),
        );

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

fn render_host_group_empty(label: &'static str) -> AnyElement {
    div()
        .px_3()
        .py_2()
        .text_xs()
        .text_color(rgb(0x6a6a72))
        .child(SharedString::from(label))
        .into_any_element()
}

fn is_active_connection(state: &Option<ConnState>) -> bool {
    matches!(state, Some(ConnState::Connected))
}

/// 连接状态徽标文字。
fn state_badget(state: &Option<ConnState>) -> String {
    match state {
        None => String::new(),
        Some(ConnState::Connecting) => "连接中 · ".to_string(),
        Some(ConnState::Connected) => "已连接 · ".to_string(),
        Some(ConnState::Error(_)) => "出错 · ".to_string(),
        Some(ConnState::Closed) => "已断开 · ".to_string(),
    }
}

/// 徽标圆点颜色。
fn badge_color(state: &Option<ConnState>) -> gpui::Hsla {
    match state {
        None => hsla(0., 0., 0.3, 1.),
        Some(ConnState::Connecting) => hsla(0.13, 0.8, 0.6, 1.),
        Some(ConnState::Connected) => hsla(0.33, 0.7, 0.5, 1.),
        Some(ConnState::Error(_)) => hsla(0., 0.8, 0.55, 1.),
        Some(ConnState::Closed) => hsla(0., 0., 0.3, 1.),
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

fn render_main(shell: &AppShell, cx: &mut Context<AppShell>) -> impl IntoElement {
    let mut pane = div()
        .flex_1()
        .h_full()
        .bg(rgb(0x121214))
        .flex()
        .flex_col();

    // 标签条。
    pane = pane.child(render_tab_strip(shell, cx));

    // 终端/SFTP 区。
    let mut content = div().flex_1().relative();
    if let Some(idx) = shell.active_tab {
        if let Some(tab) = shell.tabs.get(idx) {
            let pane_el: AnyElement = match &tab.pane {
                Pane::Terminal(t) => t.clone().into_any_element(),
                Pane::Sftp(s) => s.clone().into_any_element(),
                Pane::Forward(f) => f.clone().into_any_element(),
            };
            content = content.child(pane_el);
        }
    } else {
        content = content.child(
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(rgb(0x6a6a72))
                .child(SharedString::from(
                    "从左侧选择一个主机开始连接（主机列表来自 ~/.ssh/config）",
                )),
        );
    }

    if let Some(status) = &shell.status {
        content = content.child(
            div()
                .absolute()
                .bottom_2()
                .left_2()
                .px_3()
                .py_1()
                .text_xs()
                .bg(rgb(0x2a2a2e))
                .text_color(rgb(0xe6e6e6))
                .child(SharedString::from(status.clone())),
        );
    }
    pane = pane.child(content);
    pane
}

fn render_tab_strip(shell: &AppShell, cx: &mut Context<AppShell>) -> impl IntoElement {
    let mut strip = div()
        .flex()
        .flex_row()
        .h(px(32.))
        .bg(rgb(0x18181b))
        .border_b_1()
        .border_color(rgb(0x2a2a2e));

    for (idx, tab) in shell.tabs.iter().enumerate() {
        let is_active = shell.active_tab == Some(idx);
        let state = shell.pool.state_for_key(&tab.host_key, cx);
        let bg = if is_active {
            rgb(0x2a2a3a)
        } else {
            rgb(0x18181b)
        };
        let alias = tab.alias.clone();
        // 容器不绑定 click；标签名与关闭按钮分别绑定，避免事件叠加。
        let container = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .h_full()
            .bg(bg)
            .border_r_1()
            .border_color(rgb(0x2a2a2e))
            .child(
                div()
                    .id(("tab", idx))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(rgb(0xe6e6e6))
                    .hover(|s| s.bg(rgb(0x35355a)))
                    .child(
                        div()
                            .w(px(6.))
                            .h(px(6.))
                            .rounded_full()
                            .bg(tab_badge_color(&state)),
                    )
                    .child(SharedString::from(alias))
                    .on_click(cx.listener(move |this, _ev, _w, cx| {
                        this.switch_tab(idx, cx);
                    })),
            )
            .child(
                div()
                    .id(("tab-close", idx))
                    .px_1()
                    .py_1()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(rgb(0x888892))
                    .hover(|s| s.text_color(rgb(0xff6060)))
                    .child(SharedString::from("×"))
                    .on_click(cx.listener(move |this, _ev, _w, cx| {
                        this.close_tab(idx, cx);
                    })),
            );
        strip = strip.child(container);
    }

    let host_focus = shell.host_focus.clone();
    strip
        .child(div().flex_1())
        .child(
            div()
                .id("new-tab")
                .px_3()
                .h_full()
                .flex()
                .items_center()
                .cursor_pointer()
                .text_color(rgb(0x8c8c94))
                .hover(|s| s.bg(rgb(0x2a2a2e)).text_color(rgb(0xf5f5f7)))
                .child(SharedString::from("+"))
                .on_click(cx.listener(move |this, _ev, window, cx| {
                    this.host_query.clear();
                    host_focus.focus(window, cx);
                    cx.notify();
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

    let (title, body, is_credential): (String, String, bool) = match prompt {
        PromptDisplay::HostKey {
            ref host,
            port,
            ref key_type,
            ref fingerprint,
            changed,
        } => {
            let warn = if changed {
                "⚠️ 主机密钥已变更（可能存在中间人攻击）。\n按计划默认拒绝。"
            } else {
                "未知主机，请核对此指纹后决定。"
            };
            (
                "主机密钥确认".to_string(),
                format!(
                    "{warn}\n主机: {host}:{port}\n密钥类型: {key_type}\n指纹:\n{fingerprint}"
                ),
                false,
            )
        }
        PromptDisplay::Credential { kind, ref prompt } => {
            let title = match kind {
                CredentialKind::Passphrase => "请输入私钥口令",
                CredentialKind::Password => "请输入密码",
            };
            (title.to_string(), prompt.clone(), true)
        }
        PromptDisplay::None => return div().into_any_element(),
    };

    let mut buttons = div().flex().flex_row().gap_2().mt_4();
    match prompt {
        PromptDisplay::HostKey { changed, .. } => {
            if !changed {
                buttons = buttons
                    .child(host_key_button(shell, cx, "接受一次", HostKeyDecision::AcceptOnce))
                    .child(host_key_button(shell, cx, "总是接受", HostKeyDecision::AcceptAlways));
            }
            buttons = buttons.child(host_key_button(shell, cx, "拒绝", HostKeyDecision::Reject));
        }
        PromptDisplay::Credential { .. } => {
            buttons = buttons
                .child(cred_button(shell, cx, "确定", true))
                .child(cred_button(shell, cx, "取消", false));
        }
        PromptDisplay::None => {}
    }

    let mut card = div()
        .w(px(420.))
        .p_5()
        .bg(rgb(0x1f1f23))
        .border_1()
        .border_color(rgb(0x3a3a40))
        .rounded(px(8.))
        .shadow_md()
        .flex()
        .flex_col()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0xf5f5f7))
                .child(SharedString::from(title)),
        )
        .child(
            div()
                .mt_2()
                .text_xs()
                .text_color(rgb(0xb0b0b8))
                .child(SharedString::from(body)),
        );

    if is_credential {
        let masked = "•".repeat(shell.prompt_input.chars().count());
        let input = div()
            .id("prompt-input")
            .w(px(360.))
            .px_3()
            .py_2()
            .mt_2()
            .bg(rgb(0x121214))
            .border_1()
            .border_color(rgb(0x3a3a40))
            .text_sm()
            .text_color(rgb(0xe6e6e6))
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
        .bg(hsla(0., 0., 0., 0.55))
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
    label: &str,
    decision: HostKeyDecision,
) -> impl IntoElement {
    let id = SharedString::from(label.to_string());
    let _ = shell;
    div()
        .id(id)
        .px_3()
        .py_1()
        .text_xs()
        .cursor_pointer()
        .bg(rgb(0x2a2a2e))
        .hover(|s| s.bg(rgb(0x3a3a40)))
        .text_color(rgb(0xe6e6e6))
        .child(SharedString::from(label.to_string()))
        .on_click(cx.listener(move |this, _ev, _w, cx| {
            this.resolve_host_key(decision, cx);
        }))
}

fn cred_button(
    shell: &mut AppShell,
    cx: &mut Context<AppShell>,
    label: &str,
    submit: bool,
) -> impl IntoElement {
    let id = SharedString::from(label.to_string());
    let _ = shell;
    div()
        .id(id)
        .px_3()
        .py_1()
        .text_xs()
        .cursor_pointer()
        .bg(rgb(0x2a2a2e))
        .hover(|s| s.bg(rgb(0x3a3a40)))
        .text_color(rgb(0xe6e6e6))
        .child(SharedString::from(label.to_string()))
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
        assert!(!is_active_connection(&Some(ConnState::Error("failed".to_string()))));
        assert!(!is_active_connection(&None));
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

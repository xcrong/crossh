//! 端口转发面板：列出该主机 config 中的 -L/-R/-D 规则，每条一个开关。
//!
//! 开关经 `Connection::start_forward`/`stop_forward` 控制；启停结果（含端口占用等）
//! 显示在底部消息区。转发规则来自 ~/.ssh/config（只读），不可在此编辑。

use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement, App, AppContext, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::features::connections::Connection;
use crate::features::workspace::pane::{PaneRisk, TerminalPaneInfo, WorkspacePane};
use crate::shared::i18n;
use crossh_core::config::ForwardSpec;
use crossh_ssh::ForwardKind;
use crossh_terminal::settings::TerminalSettings;
use crossh_ui::{icons, theme};

type ForwardKey = (ForwardKind, ForwardSpec);

pub struct ForwardPane {
    conn: Entity<Connection>,
    local: Vec<ForwardSpec>,
    remote: Vec<ForwardSpec>,
    dynamic: Vec<ForwardSpec>,
    active: HashSet<(ForwardKind, ForwardSpec)>,
    pending: HashMap<ForwardKey, u64>,
    next_request_id: u64,
    messages: Vec<String>,
    focus: FocusHandle,
    focus_requested: bool,
}

pub(crate) fn workspace_pane(entity: Entity<ForwardPane>) -> Box<dyn WorkspacePane> {
    Box::new(ForwardWorkspacePane(entity))
}

struct ForwardWorkspacePane(Entity<ForwardPane>);

impl WorkspacePane for ForwardWorkspacePane {
    fn render(&self) -> AnyElement {
        self.0.clone().into_any_element()
    }

    fn title(&self, _cx: &App) -> String {
        crossh_core::terminal::remote_pane_title(&i18n::text("tab.forward"))
    }

    fn terminal_info(&self, _cx: &App) -> Option<TerminalPaneInfo> {
        None
    }

    fn terminal_entity_id(&self) -> Option<gpui::EntityId> {
        None
    }

    fn cwd(&self, _cx: &App) -> Option<String> {
        None
    }

    fn is_command_running(&self, _cx: &App) -> bool {
        false
    }

    fn toggle_low_latency(&self, _cx: &mut App) {}

    fn run_command(&self, _command: &str, _cx: &mut App) {}

    fn handle_system_notification_response(
        &self,
        _response: &gpui::SystemNotificationResponse,
        _cx: &mut App,
    ) -> Option<bool> {
        None
    }

    fn request_focus(&self, cx: &mut App) {
        self.0.update(cx, |pane, cx| pane.request_focus(cx));
    }

    fn request_close(&self, cx: &mut App) {
        self.0.update(cx, |pane, cx| pane.stop_all(cx));
    }

    fn cleanup(&self, cx: &mut App) {
        self.0.update(cx, |pane, cx| pane.stop_all(cx));
    }

    fn apply_terminal_settings(&self, _settings: TerminalSettings, _cx: &mut App) {}

    fn notify_language(&self, cx: &mut App) {
        self.0.update(cx, |_, cx| cx.notify());
    }

    fn risk(&self, cx: &App) -> PaneRisk {
        PaneRisk {
            active_forwards: self.0.read(cx).active_count(),
            ..PaneRisk::default()
        }
    }
}

impl ForwardPane {
    pub(crate) fn active_count(&self) -> usize {
        self.active.len() + self.pending.len()
    }

    pub(crate) fn stop_all(&mut self, cx: &mut Context<Self>) {
        let mut forwards = std::mem::take(&mut self.active);
        forwards.extend(std::mem::take(&mut self.pending).into_keys());
        for (kind, spec) in forwards {
            self.conn.read(cx).stop_forward(spec, kind);
        }
        cx.notify();
    }

    fn request_focus(&mut self, cx: &mut Context<Self>) {
        self.focus_requested = true;
        cx.notify();
    }

    pub fn new(
        conn: Entity<Connection>,
        cx: &mut App,
        forwards: &crossh_core::config::HostConfig,
    ) -> Entity<Self> {
        let focus = cx.focus_handle();
        cx.new(|_cx| Self {
            conn,
            local: forwards.local_forwards.clone(),
            remote: forwards.remote_forwards.clone(),
            dynamic: forwards.dynamic_forwards.clone(),
            active: HashSet::new(),
            pending: HashMap::new(),
            next_request_id: 0,
            messages: Vec::new(),
            focus,
            focus_requested: false,
        })
    }

    fn toggle(&mut self, kind: ForwardKind, spec: ForwardSpec, cx: &mut Context<Self>) {
        let key = (kind, spec.clone());
        if self.active.remove(&key) {
            // 关闭。
            let listen = spec.listen.clone();
            self.conn.read(cx).stop_forward(spec, kind);
            self.push_msg(
                cx,
                rust_i18n::t!(
                    "forward.stopping",
                    kind = forward_kind_label(kind),
                    listen = listen
                )
                .to_string(),
            );
        } else if self.pending.remove(&key).is_some() {
            // 启动尚未回执时也必须发送停止命令；连接层会按队列顺序
            // 在 StartForward 完成后处理它，避免遗留一个无 UI 控制的 listener。
            let listen = spec.listen.clone();
            self.conn.read(cx).stop_forward(spec, kind);
            self.push_msg(
                cx,
                rust_i18n::t!(
                    "forward.stopping",
                    kind = forward_kind_label(kind),
                    listen = listen
                )
                .to_string(),
            );
        } else {
            // 启动：发命令并等回执。
            let rx = self.conn.read(cx).start_forward(spec.clone(), kind);
            let request_id = self.next_request_id;
            self.next_request_id = self.next_request_id.wrapping_add(1);
            self.pending.insert(key.clone(), request_id);
            cx.spawn(async move |weak, cx| {
                let res = rx.await;
                let _ = weak.update(cx, |this, cx| match res {
                    Ok(Ok(())) => {
                        if this.pending.get(&key).copied() != Some(request_id) {
                            return;
                        }
                        this.pending.remove(&key);
                        this.active.insert(key.clone());
                        this.push_msg(
                            cx,
                            rust_i18n::t!(
                                "forward.started",
                                kind = forward_kind_label(key.0),
                                listen = key.1.listen
                            )
                            .to_string(),
                        );
                    }
                    Ok(Err(e)) => {
                        if this.pending.get(&key).copied() != Some(request_id) {
                            return;
                        }
                        this.pending.remove(&key);
                        this.push_msg(
                            cx,
                            rust_i18n::t!(
                                "forward.start_failed",
                                kind = forward_kind_label(key.0),
                                listen = key.1.listen,
                                error = e
                            )
                            .to_string(),
                        );
                    }
                    Err(_) => {
                        if this.pending.get(&key).copied() != Some(request_id) {
                            return;
                        }
                        this.pending.remove(&key);
                        this.push_msg(cx, i18n::text("forward.connection_closed"));
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    fn push_msg(&mut self, cx: &mut Context<Self>, msg: String) {
        self.messages.push(msg);
        if self.messages.len() > 50 {
            self.messages.remove(0);
        }
        cx.notify();
    }
}

impl Render for ForwardPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_requested {
            self.focus_requested = false;
            self.focus.focus(window, cx);
        }

        let mut col = div()
            .size_full()
            .flex()
            .flex_col()
            .px_4()
            .py_4()
            .gap_4()
            .bg(theme::canvas());

        col = col.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .text_color(theme::text())
                .child(
                    div()
                        .w(px(28.))
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(theme::RADIUS_SM))
                        .bg(theme::accent_soft())
                        .child(
                            icons::icon(icons::IconName::ArrowLeftRight, 16.)
                                .text_color(theme::accent()),
                        ),
                )
                .child(SharedString::from(i18n::text("forward.title")))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::faint_text())
                        .child(SharedString::from(i18n::text("forward.source"))),
                ),
        );

        let total = self.local.len() + self.remote.len() + self.dynamic.len();
        if total == 0 {
            col = col.child(
                div()
                    .p_3()
                    .rounded(px(theme::RADIUS_SM))
                    .bg(theme::surface())
                    .border_1()
                    .border_color(theme::border())
                    .text_sm()
                    .text_color(theme::muted_text())
                    .child(SharedString::from(i18n::text("forward.no_config"))),
            );
        }

        col = render_section(
            col,
            i18n::text("forward.local"),
            ForwardKind::Local,
            &self.local,
            &self.active,
            &self.pending,
            cx,
        );
        col = render_section(
            col,
            i18n::text("forward.remote"),
            ForwardKind::Remote,
            &self.remote,
            &self.active,
            &self.pending,
            cx,
        );
        col = render_section(
            col,
            i18n::text("forward.dynamic"),
            ForwardKind::Dynamic,
            &self.dynamic,
            &self.active,
            &self.pending,
            cx,
        );

        if !self.messages.is_empty() {
            col = col.child(
                div()
                    .mt_2()
                    .p_2()
                    .rounded(px(theme::RADIUS_SM))
                    .bg(theme::surface())
                    .border_1()
                    .border_color(theme::border())
                    .text_xs()
                    .text_color(theme::muted_text())
                    .child(icons::icon(icons::IconName::Link, 14.).text_color(theme::info()))
                    .child(SharedString::from(
                        self.messages.last().cloned().unwrap_or_default(),
                    )),
            );
        }
        div()
            .id("forward-pane")
            .size_full()
            .track_focus(&self.focus)
            .tab_stop(true)
            .child(col)
    }
}

fn section_id(kind: ForwardKind) -> &'static str {
    match kind {
        ForwardKind::Local => "fwd-local",
        ForwardKind::Remote => "fwd-remote",
        ForwardKind::Dynamic => "fwd-dynamic",
    }
}

fn forward_kind_label(kind: ForwardKind) -> String {
    i18n::text(match kind {
        ForwardKind::Local => "forward.kind_local",
        ForwardKind::Remote => "forward.kind_remote",
        ForwardKind::Dynamic => "forward.kind_dynamic",
    })
}

/// 渲染一组转发规则（标题 + 每条开关行）。
fn render_section(
    mut col: gpui::Div,
    title: String,
    kind: ForwardKind,
    specs: &[ForwardSpec],
    active: &HashSet<(ForwardKind, ForwardSpec)>,
    pending: &HashMap<ForwardKey, u64>,
    cx: &mut Context<ForwardPane>,
) -> gpui::Div {
    if specs.is_empty() {
        return col;
    }
    let mut section = div().flex().flex_col().gap_1().child(
        div()
            .px_1()
            .text_xs()
            .text_color(theme::muted_text())
            .child(SharedString::from(title)),
    );
    for (i, spec) in specs.iter().enumerate() {
        let on =
            active.contains(&(kind, spec.clone())) || pending.contains_key(&(kind, spec.clone()));
        let spec2 = spec.clone();
        let label = format!(
            "{}  →  {}",
            spec.listen,
            if spec.remote.is_empty() {
                "(SOCKS)".to_string()
            } else {
                spec.remote.clone()
            }
        );
        let row = div()
            .id((section_id(kind), i))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .h(px(34.))
            .px_2()
            .rounded(px(theme::RADIUS_SM))
            .cursor_pointer()
            .bg(if on {
                theme::accent_soft()
            } else {
                theme::canvas()
            })
            .hover(|s| s.bg(theme::raised()))
            .child(div().w(px(10.)).h(px(10.)).rounded_full().bg(if on {
                theme::accent()
            } else {
                theme::border_strong()
            }))
            .child(icons::icon(icons::IconName::Link, 14.).text_color(if on {
                theme::accent()
            } else {
                theme::faint_text()
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(theme::text())
                    .child(SharedString::from(label)),
            )
            .on_click(cx.listener(move |this, _ev, _w, cx| {
                this.toggle(kind, spec2.clone(), cx);
            }));
        section = section.child(row);
    }
    col = col.child(section);
    col
}

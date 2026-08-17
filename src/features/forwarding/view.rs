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

use crossh_ui::{icons, theme};
use crossh_ui_component::StatusDot;

type ForwardKey = (ForwardKind, ForwardSpec);

#[derive(Default)]
struct ForwardTracker {
    active: HashSet<ForwardKey>,
    pending: HashMap<ForwardKey, u64>,
    next_request_id: u64,
}

enum ForwardToggle {
    Start { request_id: u64 },
    Stop,
}

#[derive(Debug, PartialEq, Eq)]
enum ForwardCompletion {
    Started,
    Failed,
    Stale,
}

impl ForwardTracker {
    fn count(&self) -> usize {
        self.active.len() + self.pending.len()
    }

    fn toggle(&mut self, key: ForwardKey) -> ForwardToggle {
        if self.active.remove(&key) || self.pending.remove(&key).is_some() {
            return ForwardToggle::Stop;
        }
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        self.pending.insert(key, request_id);
        ForwardToggle::Start { request_id }
    }

    fn complete(
        &mut self,
        key: &ForwardKey,
        request_id: u64,
        succeeded: bool,
    ) -> ForwardCompletion {
        if self.pending.get(key).copied() != Some(request_id) {
            return ForwardCompletion::Stale;
        }
        self.pending.remove(key);
        if succeeded {
            self.active.insert(key.clone());
            ForwardCompletion::Started
        } else {
            ForwardCompletion::Failed
        }
    }

    fn take_all(&mut self) -> Vec<ForwardKey> {
        let mut forwards = std::mem::take(&mut self.active);
        forwards.extend(std::mem::take(&mut self.pending).into_keys());
        forwards.into_iter().collect()
    }
}

pub struct ForwardPane {
    conn: Entity<Connection>,
    local: Vec<ForwardSpec>,
    remote: Vec<ForwardSpec>,
    dynamic: Vec<ForwardSpec>,
    tracker: ForwardTracker,
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
        self.tracker.count()
    }

    pub(crate) fn stop_all(&mut self, cx: &mut Context<Self>) {
        for (kind, spec) in self.tracker.take_all() {
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
            tracker: ForwardTracker::default(),
            messages: Vec::new(),
            focus,
            focus_requested: false,
        })
    }

    fn toggle(&mut self, kind: ForwardKind, spec: ForwardSpec, cx: &mut Context<Self>) {
        let key = (kind, spec.clone());
        match self.tracker.toggle(key.clone()) {
            ForwardToggle::Stop => {
                // pending 启动也发送停止；连接层按队列顺序处理，避免遗留 listener。
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
            }
            ForwardToggle::Start { request_id } => {
                let rx = self.conn.read(cx).start_forward(spec.clone(), kind);
                cx.spawn(async move |weak, cx| {
                    let res = rx.await;
                    let _ = weak.update(cx, |this, cx| match res {
                        Ok(Ok(())) => {
                            if this.tracker.complete(&key, request_id, true)
                                != ForwardCompletion::Started
                            {
                                return;
                            }
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
                            if this.tracker.complete(&key, request_id, false)
                                != ForwardCompletion::Failed
                            {
                                return;
                            }
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
                            if this.tracker.complete(&key, request_id, false)
                                != ForwardCompletion::Failed
                            {
                                return;
                            }
                            this.push_msg(cx, i18n::text("forward.connection_closed"));
                        }
                    });
                })
                .detach();
            }
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
            &self.tracker.active,
            &self.tracker.pending,
            cx,
        );
        col = render_section(
            col,
            i18n::text("forward.remote"),
            ForwardKind::Remote,
            &self.remote,
            &self.tracker.active,
            &self.tracker.pending,
            cx,
        );
        col = render_section(
            col,
            i18n::text("forward.dynamic"),
            ForwardKind::Dynamic,
            &self.dynamic,
            &self.tracker.active,
            &self.tracker.pending,
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
            .child(
                StatusDot::new(if on {
                    theme::accent()
                } else {
                    theme::border_strong()
                })
                .size(px(10.)),
            )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn key(port: u16) -> ForwardKey {
        (
            ForwardKind::Local,
            ForwardSpec {
                listen: port.to_string(),
                remote: "service.internal:80".into(),
            },
        )
    }

    #[test]
    fn stopped_pending_forward_ignores_its_late_success() {
        let mut tracker = ForwardTracker::default();
        let key = key(8080);
        let ForwardToggle::Start { request_id } = tracker.toggle(key.clone()) else {
            panic!("first toggle must start");
        };
        assert_eq!(tracker.count(), 1);
        assert!(matches!(tracker.toggle(key.clone()), ForwardToggle::Stop));
        assert_eq!(tracker.count(), 0);

        assert_eq!(
            tracker.complete(&key, request_id, true),
            ForwardCompletion::Stale
        );
        assert_eq!(tracker.count(), 0);
        assert!(!tracker.active.contains(&key));
    }

    #[test]
    fn stale_request_cannot_replace_a_newer_pending_start() {
        let mut tracker = ForwardTracker::default();
        let key = key(9090);
        let ForwardToggle::Start { request_id: first } = tracker.toggle(key.clone()) else {
            panic!("first toggle must start");
        };
        tracker.toggle(key.clone());
        let ForwardToggle::Start { request_id: second } = tracker.toggle(key.clone()) else {
            panic!("third toggle must start again");
        };

        assert_eq!(
            tracker.complete(&key, first, true),
            ForwardCompletion::Stale
        );
        assert_eq!(
            tracker.complete(&key, second, true),
            ForwardCompletion::Started
        );
        assert!(tracker.active.contains(&key));
    }

    #[test]
    fn take_all_clears_active_and_pending_forwards() {
        let mut tracker = ForwardTracker::default();
        let active = key(7000);
        let pending = key(7001);
        let ForwardToggle::Start { request_id } = tracker.toggle(active.clone()) else {
            panic!("start");
        };
        assert_eq!(
            tracker.complete(&active, request_id, true),
            ForwardCompletion::Started
        );
        tracker.toggle(pending.clone());

        let all = tracker.take_all();
        assert_eq!(all.len(), 2);
        assert!(all.contains(&active));
        assert!(all.contains(&pending));
        assert_eq!(tracker.count(), 0);
    }
}

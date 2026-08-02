//! 端口转发面板：列出该主机 config 中的 -L/-R/-D 规则，每条一个开关。
//!
//! 开关经 `Connection::start_forward`/`stop_forward` 控制；启停结果（含端口占用等）
//! 显示在底部消息区。转发规则来自 ~/.ssh/config（只读），不可在此编辑。

use std::collections::HashSet;

use gpui::{
    App, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Task, Window, div, px, rgb,
};

use crate::config::ForwardSpec;
use crate::ssh::{Connection, ForwardKind};

pub struct ForwardPane {
    conn: Entity<Connection>,
    local: Vec<ForwardSpec>,
    remote: Vec<ForwardSpec>,
    dynamic: Vec<ForwardSpec>,
    active: HashSet<(ForwardKind, ForwardSpec)>,
    messages: Vec<String>,
    _pending: Option<Task<()>>,
}

impl ForwardPane {
    pub fn new(
        conn: Entity<Connection>,
        cx: &mut App,
        forwards: &crate::config::HostConfig,
    ) -> Entity<Self> {
        cx.new(|_cx| Self {
            conn,
            local: forwards.local_forwards.clone(),
            remote: forwards.remote_forwards.clone(),
            dynamic: forwards.dynamic_forwards.clone(),
            active: HashSet::new(),
            messages: Vec::new(),
            _pending: None,
        })
    }

    fn toggle(&mut self, kind: ForwardKind, spec: ForwardSpec, cx: &mut Context<Self>) {
        if self.active.contains(&(kind, spec.clone())) {
            // 关闭。
            let listen = spec.listen.clone();
            self.active.remove(&(kind, spec.clone()));
            self.conn.read(cx).stop_forward(spec, kind);
            self.push_msg(cx, format!("停止 {:?} {}", kind, listen));
        } else {
            // 启动：发命令并等回执。
            let rx = self.conn.read(cx).start_forward(spec.clone(), kind);
            let key = (kind, spec.clone());
            let task = cx.spawn(async move |weak, cx| {
                let res = rx.await;
                let _ = weak.update(cx, |this, cx| match res {
                    Ok(Ok(())) => {
                        this.active.insert(key.clone());
                        this.push_msg(cx, format!("已启动 {:?} {}", key.0, key.1.listen));
                    }
                    Ok(Err(e)) => {
                        this.push_msg(cx, format!("启动 {:?} {} 失败: {e}", key.0, key.1.listen));
                    }
                    Err(_) => {
                        this.push_msg(cx, "连接已关闭".into());
                    }
                });
            });
            self._pending = Some(task);
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut col = div()
            .size_full()
            .flex()
            .flex_col()
            .px_4()
            .py_3()
            .gap_2()
            .bg(rgb(0x121214));

        col = col.child(
            div()
                .text_sm()
                .text_color(rgb(0xb0b0b8))
                .child(SharedString::from("端口转发（来自 ~/.ssh/config）")),
        );

        let total = self.local.len() + self.remote.len() + self.dynamic.len();
        if total == 0 {
            col = col.child(
                div()
                    .text_xs()
                    .text_color(rgb(0x6a6a72))
                    .child(SharedString::from(
                        "该主机未配置 LocalForward / RemoteForward / DynamicForward。",
                    )),
            );
        }

        col = render_section(
            col,
            "本地 (-L)",
            ForwardKind::Local,
            &self.local,
            &self.active,
            cx,
        );
        col = render_section(
            col,
            "远端 (-R)",
            ForwardKind::Remote,
            &self.remote,
            &self.active,
            cx,
        );
        col = render_section(
            col,
            "动态 (-D SOCKS5)",
            ForwardKind::Dynamic,
            &self.dynamic,
            &self.active,
            cx,
        );

        if !self.messages.is_empty() {
            col = col.child(
                div()
                    .mt_2()
                    .p_2()
                    .bg(rgb(0x18181b))
                    .border_1()
                    .border_color(rgb(0x2a2a2e))
                    .text_xs()
                    .text_color(rgb(0xb0b0b8))
                    .child(SharedString::from(
                        self.messages.last().cloned().unwrap_or_default(),
                    )),
            );
        }
        col
    }
}

fn section_id(kind: ForwardKind) -> &'static str {
    match kind {
        ForwardKind::Local => "fwd-local",
        ForwardKind::Remote => "fwd-remote",
        ForwardKind::Dynamic => "fwd-dynamic",
    }
}

/// 渲染一组转发规则（标题 + 每条开关行）。
fn render_section(
    mut col: gpui::Div,
    title: &str,
    kind: ForwardKind,
    specs: &[ForwardSpec],
    active: &HashSet<(ForwardKind, ForwardSpec)>,
    cx: &mut Context<ForwardPane>,
) -> gpui::Div {
    if specs.is_empty() {
        return col;
    }
    let mut section = div().flex().flex_col().gap_1().child(
        div()
            .text_xs()
            .text_color(rgb(0x888892))
            .child(SharedString::from(title.to_string())),
    );
    for (i, spec) in specs.iter().enumerate() {
        let on = active.contains(&(kind, spec.clone()));
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
            .px_2()
            .py_1()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(0x232327)))
            .child(div().w(px(10.)).h(px(10.)).rounded_full().bg(if on {
                gpui::hsla(0.33, 0.7, 0.5, 1.)
            } else {
                gpui::hsla(0., 0., 0.25, 1.)
            }))
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(rgb(0xe6e6e6))
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

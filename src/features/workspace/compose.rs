//! 批量输入条的终端级状态与交互（与分栏同为终端级）。
//! 逻辑与渲染分离：本模块负责可见性、草稿、发送与按键；渲染在 `compose_bar.rs`。

use gpui::{ClipboardItem, Context, KeyDownEvent, Window};

use crate::features::workspace::view::ActiveView;
use crate::shared::text_editing::handle_text_editing_key;

use super::AppShell;

impl AppShell {
    pub(crate) fn toggle_compose_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.workspace.focused_view() else {
            return;
        };
        let entry = self.workspace.compose_entry_mut(view);
        entry.visible = !entry.visible;
        if entry.visible {
            window.focus(&self.compose_focus, cx);
        } else {
            self.refocus_active_terminal(cx);
        }
        cx.notify();
    }

    pub(crate) fn hide_compose_bar(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.workspace.focused_view() else {
            return;
        };
        let Some(entry) = self.workspace.compose.get_mut(&view) else {
            return;
        };
        if !entry.visible {
            return;
        }
        entry.visible = false;
        self.refocus_active_terminal(cx);
        cx.notify();
    }

    pub(crate) fn send_compose(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.workspace.focused_view() else {
            return;
        };
        let text = self
            .workspace
            .compose
            .get(&view)
            .map(|e| e.state.value.trim().to_string())
            .unwrap_or_default();
        if text.is_empty() {
            return;
        }
        match view {
            ActiveView::RemoteTab(index) => {
                if let Some(tab) = self.workspace.sessions.remote_tabs.get(index) {
                    tab.pane.run_command_without_focus(&text, cx);
                }
            }
            ActiveView::LocalSession(session_id) => {
                if let Some(session) = self.workspace.sessions.local_sessions.get(&session_id) {
                    session.terminal.update(cx, |terminal, term_cx| {
                        terminal.run_command_without_focus(&text, term_cx)
                    });
                }
            }
        }
        if let Some(entry) = self.workspace.compose.get_mut(&view) {
            entry.state.clear();
        }
        self.compose_scroll.set_offset(gpui::Point::default());
        cx.notify();
    }

    pub(crate) fn handle_compose_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.workspace.focused_view() else {
            return;
        };
        let ks = &ev.keystroke;
        let is_send = (ks.modifiers.control || ks.modifiers.platform) && ks.key == "enter";
        if is_send {
            {
                let entry = self.workspace.compose_entry_mut(view);
                entry.state.clear_composition();
            }
            self.send_compose(cx);
            // 保持焦点在 compose，避免 TerminalView::run_command_without_focus 已避免抢占后，
            // 仍因其他原因（如 button 点击）丢失；此处显式夺回。
            window.focus(&self.compose_focus, cx);
            cx.stop_propagation();
            return;
        }
        if ks.key == "escape" {
            {
                let entry = self.workspace.compose_entry_mut(view);
                entry.state.clear_composition();
            }
            self.hide_compose_bar(cx);
            cx.stop_propagation();
            return;
        }
        if ks.key == "enter" && ks.modifiers.shift {
            let entry = self.workspace.compose_entry_mut(view);
            entry.state.clear_composition();
            entry.state.replace_selection("\n");
            cx.notify();
            cx.stop_propagation();
            return;
        }
        let entry = self.workspace.compose_entry_mut(view);
        let state = &mut entry.state;
        let primary = ks.modifiers.control || ks.modifiers.platform;
        let paste_text = if primary && ks.key == "v" {
            cx.read_from_clipboard()
                .and_then(|item| item.text().map(|s| s.to_string()))
        } else {
            None
        };
        let result = handle_text_editing_key(state, ks, paste_text.as_deref());
        if let Some(text) = result.copy_text {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        if result.handled {
            cx.notify();
            cx.stop_propagation();
        }
    }
}

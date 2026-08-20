//! 模态文本编辑器（Quick Command 编辑器 + 固定标签重命名弹窗）的行为：
//! 打开/提交/取消与统一键盘处理。渲染位于 `view.rs`（两者互斥，
//! 键盘入口合并为一个 handler，避免重复的编辑键分支）。

use gpui::{ClipboardEntry, Context, KeyDownEvent, Window};

use crossh_ui::widgets::printable_char;

use crate::shared::text_editing::TextEditingState;

use super::command_editor::QuickCommandEditor;
use super::*;

impl AppShell {
    pub(crate) fn open_quick_command_editor(
        &mut self,
        scope: String,
        command: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = cx.focus_handle();
        self.quick_command_editor = Some(QuickCommandEditor::new(scope, command, focus.clone()));
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(crate) fn submit_quick_command_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.quick_command_editor.take() else {
            return;
        };
        self.command_history
            .edit(&editor.scope, &editor.original, &editor.state.value);
        cx.notify();
    }

    pub(crate) fn cancel_quick_command_editor(&mut self, cx: &mut Context<Self>) {
        if self.quick_command_editor.take().is_some() {
            cx.notify();
        }
    }

    /// 模态文本编辑器的统一键盘处理：固定标签重命名弹窗与 Quick Command
    /// 编辑器共用（两者互斥，最多只有一个打开）。
    pub(crate) fn handle_modal_editor_key(
        &mut self,
        ev: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &ev.keystroke;
        let primary = ks.modifiers.control || ks.modifiers.platform;
        let extend = ks.modifiers.shift;

        if primary && ks.key == "a" {
            if let Some(editor) = self.active_editor_state() {
                editor.clear_composition();
                editor.select_all();
            }
            cx.notify();
            return;
        }

        if primary && matches!(ks.key.as_str(), "c" | "x") {
            if let Some(editor) = self.active_editor_state()
                && let Some(text) = editor.selected_text()
            {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                if ks.key == "x" {
                    editor.clear_composition();
                    editor.replace_selection("");
                }
            }
            cx.notify();
            return;
        }

        if primary && ks.key == "v" {
            let pasted = cx.read_from_clipboard().and_then(|item| {
                item.into_entries().find_map(|entry| match entry {
                    ClipboardEntry::String(value) => Some(value.text),
                    _ => None,
                })
            });
            if let Some(editor) = self.active_editor_state()
                && let Some(text) = pasted
            {
                editor.clear_composition();
                editor.replace_selection(&text);
            }
            cx.notify();
            return;
        }

        match ks.key.as_str() {
            "enter" | "return" => {
                if self.default_command_editor.is_some() {
                    self.submit_default_command(cx);
                } else if self.rename_editor.is_some() {
                    self.submit_rename_local_session(cx);
                } else {
                    self.submit_quick_command_editor(cx);
                }
            }
            "escape" => {
                if self.default_command_editor.is_some() {
                    self.cancel_default_command(cx);
                } else if self.rename_editor.is_some() {
                    self.cancel_rename_local_session(cx);
                } else {
                    self.cancel_quick_command_editor(cx);
                }
            }
            "backspace" => {
                if let Some(editor) = self.active_editor_state() {
                    editor.clear_composition();
                    editor.backspace();
                }
                cx.notify();
            }
            "delete" => {
                if let Some(editor) = self.active_editor_state() {
                    editor.clear_composition();
                    editor.delete();
                }
                cx.notify();
            }
            "left" => {
                if let Some(editor) = self.active_editor_state() {
                    editor.clear_composition();
                    editor.move_horizontal(-1, extend);
                }
                cx.notify();
            }
            "right" => {
                if let Some(editor) = self.active_editor_state() {
                    editor.clear_composition();
                    editor.move_horizontal(1, extend);
                }
                cx.notify();
            }
            "home" => {
                if let Some(editor) = self.active_editor_state() {
                    editor.clear_composition();
                    editor.move_to_boundary(false, extend);
                }
                cx.notify();
            }
            "end" => {
                if let Some(editor) = self.active_editor_state() {
                    editor.clear_composition();
                    editor.move_to_boundary(true, extend);
                }
                cx.notify();
            }
            _ => {
                if let Some(ch) = printable_char(ks)
                    && let Some(editor) = self.active_editor_state()
                {
                    editor.clear_composition();
                    editor.replace_selection(&ch.to_string());
                    cx.notify();
                }
            }
        }
    }

    /// 当前打开的模态编辑器的文本状态（默认命令 > 重命名 > QuickCommand；三者互斥）。
    fn active_editor_state(&mut self) -> Option<&mut TextEditingState> {
        if self.default_command_editor.is_some() {
            self.default_command_editor
                .as_mut()
                .map(|editor| &mut editor.state)
        } else if self.rename_editor.is_some() {
            self.rename_editor.as_mut().map(|editor| &mut editor.state)
        } else {
            self.quick_command_editor
                .as_mut()
                .map(|editor| &mut editor.state)
        }
    }
}

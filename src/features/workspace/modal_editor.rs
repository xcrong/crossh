//! 模态文本编辑器（固定标签重命名弹窗 + 默认命令编辑器）的行为：
//! 打开/提交/取消与统一键盘处理。渲染位于 `view.rs`（两者互斥，
//! 键盘入口合并为一个 handler，避免重复的编辑键分支）。

use gpui::{ClipboardEntry, ClipboardItem, Context, FocusHandle, KeyDownEvent, Window};

use crate::features::workspace::state::LocalSessionId;
use crate::shared::text_editing::{EditingKeystroke, TextEditingState, handle_text_editing_key};

use super::*;

pub(crate) struct PinnedTabEditor {
    pub(crate) session_id: LocalSessionId,
    pub(crate) state: TextEditingState,
    pub(crate) focus: FocusHandle,
}

impl PinnedTabEditor {
    pub(crate) fn new(session_id: LocalSessionId, current: String, focus: FocusHandle) -> Self {
        Self {
            session_id,
            state: TextEditingState::new(current),
            focus,
        }
    }
}

pub(crate) type RenameEditor = PinnedTabEditor;
pub(crate) type DefaultCommandEditor = PinnedTabEditor;

impl AppShell {
    /// 模态文本编辑器的统一键盘处理：固定标签重命名弹窗与默认命令编辑器共用（两者互斥，最多只有一个打开）。
    pub(crate) fn handle_modal_editor_key(
        &mut self,
        ev: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &ev.keystroke;
        // enter/escape 为模态提交/取消语义，优先于通用编辑分发。
        match ks.key.as_str() {
            "enter" | "return" => {
                if self.default_command_editor.is_some() {
                    self.submit_default_command(cx);
                } else if self.rename_editor.is_some() {
                    self.submit_rename_local_session(cx);
                } else {
                    return;
                }
                return;
            }
            "escape" => {
                if self.default_command_editor.is_some() {
                    self.cancel_default_command(cx);
                } else if self.rename_editor.is_some() {
                    self.cancel_rename_local_session(cx);
                } else {
                    return;
                }
                return;
            }
            _ => {}
        }
        let primary = ks.modifiers.control || ks.modifiers.platform;
        let paste_text = if primary && ks.key == "v" {
            cx.read_from_clipboard().and_then(|item| {
                item.into_entries().find_map(|entry| match entry {
                    ClipboardEntry::String(value) => Some(value.text),
                    _ => None,
                })
            })
        } else {
            None
        };
        if let Some(editor) = self.active_editor_state() {
            let editing_ks = EditingKeystroke {
                key: ks.key.clone(),
                key_char: ks.key_char.clone(),
                control: ks.modifiers.control,
                platform: ks.modifiers.platform,
                shift: ks.modifiers.shift,
            };
            let result = handle_text_editing_key(editor, &editing_ks, paste_text.as_deref());
            if let Some(text) = result.copy_text {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            if result.handled {
                cx.notify();
            }
        }
    }

    /// 当前打开的模态编辑器的文本状态（默认命令 > 重命名；两者互斥）。
    fn active_editor_state(&mut self) -> Option<&mut TextEditingState> {
        if self.default_command_editor.is_some() {
            self.default_command_editor
                .as_mut()
                .map(|editor| &mut editor.state)
        } else {
            self.rename_editor.as_mut().map(|editor| &mut editor.state)
        }
    }
}

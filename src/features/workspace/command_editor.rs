//! Quick Command 编辑器的视图侧组装。
//!
//! 文本编辑状态与字符边界运算位于 `crate::shared::text_editing::TextEditingState`；
//! 本文件只保留 `QuickCommandEditor` 的 UI 侧字段（scope/original/focus/scroll）
//! 与便捷委托方法。测试随状态机迁移到 shared/text_editing.rs，保持纯 `#[test]`。

use gpui::{FocusHandle, ScrollHandle};

use crate::shared::text_editing::TextEditingState;

pub(crate) struct QuickCommandEditor {
    pub(crate) scope: String,
    pub(crate) original: String,
    pub(crate) state: TextEditingState,
    pub(crate) scroll: ScrollHandle,
    pub(crate) focus: FocusHandle,
}

impl QuickCommandEditor {
    pub(crate) fn new(scope: String, original: String, focus: FocusHandle) -> Self {
        Self {
            scope,
            original: original.clone(),
            state: TextEditingState::new(original),
            scroll: ScrollHandle::new(),
            focus,
        }
    }

    pub(crate) fn selection(&self) -> Option<(usize, usize)> {
        self.state.selection()
    }

    pub(crate) fn replace_selection(&mut self, text: &str) {
        self.state.replace_selection(text);
    }

    pub(crate) fn backspace(&mut self) {
        self.state.backspace();
    }

    pub(crate) fn delete(&mut self) {
        self.state.delete();
    }

    pub(crate) fn move_horizontal(&mut self, direction: i8, extend: bool) {
        self.state.move_horizontal(direction, extend);
    }

    pub(crate) fn move_to_boundary(&mut self, end: bool, extend: bool) {
        self.state.move_to_boundary(end, extend);
    }

    pub(crate) fn select_all(&mut self) {
        self.state.select_all();
    }

    pub(crate) fn selected_text(&self) -> Option<String> {
        self.state.selected_text()
    }
}

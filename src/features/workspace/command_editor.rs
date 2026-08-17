//! Quick Command 编辑器的视图侧组装。
//!
//! 文本编辑状态与字符边界运算位于 `crate::shared::text_editing::TextEditingState`；
//! 本文件只保留 `QuickCommandEditor` 的 UI 侧字段（scope/original/focus/scroll），
//! 调用方直接操作 `state`。测试随状态机迁移到 shared/text_editing.rs，保持纯 `#[test]`。

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
}

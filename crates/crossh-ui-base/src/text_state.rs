// Copyright (c) 2026 Crossh contributors.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 单行文本快照：值 / 光标 / 锚点 / IME 组合状态，零 `gpui` 依赖。
//!
//! 跨 seam 无 `pub` 字段：`new` + `with_*` builders + 原名 readers；
//! 选区派生走 [`crate::text_selection`]，与 `TextInput` 护栏同源。

/// 单行输入框的状态快照：调用方传入声明，渲染层只读。
#[derive(Clone, Debug)]
pub struct SharedTextState {
    value: String,
    cursor: usize,
    anchor: Option<usize>,
    ime_marked_text: String,
    ime_replacement: Option<(usize, usize)>,
}

impl SharedTextState {
    /// 以完整文本创建快照：光标落在末尾，无锚点、无 IME 组合。
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.len();
        Self {
            value,
            cursor,
            anchor: None,
            ime_marked_text: String::new(),
            ime_replacement: None,
        }
    }

    /// 替换文本；光标收敛到新长度内，超界锚点丢弃。
    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self.cursor = self.cursor.min(self.value.len());
        if self.anchor.is_some_and(|anchor| anchor > self.value.len()) {
            self.anchor = None;
        }
        self
    }

    /// 设置光标字节索引。
    pub fn with_cursor(mut self, cursor: usize) -> Self {
        self.cursor = cursor;
        self
    }

    /// 设置选区锚点；`None` 表示无选区。
    pub fn with_anchor(mut self, anchor: Option<usize>) -> Self {
        self.anchor = anchor;
        self
    }

    /// 设置 IME 组合中的标记文本。
    pub fn with_ime_marked_text(mut self, marked: impl Into<String>) -> Self {
        self.ime_marked_text = marked.into();
        self
    }

    /// 设置 IME 替换区间；`None` 表示无组合。
    pub fn with_ime_replacement(mut self, replacement: Option<(usize, usize)>) -> Self {
        self.ime_replacement = replacement;
        self
    }

    /// 当前文本。
    pub fn value(&self) -> &str {
        &self.value
    }

    /// 光标字节索引。
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// 选区锚点；`None` 表示无选区。
    pub fn anchor(&self) -> Option<usize> {
        self.anchor
    }

    /// IME 组合中的标记文本。
    pub fn ime_marked_text(&self) -> &str {
        &self.ime_marked_text
    }

    /// IME 替换区间。
    pub fn ime_replacement(&self) -> Option<(usize, usize)> {
        self.ime_replacement
    }

    /// 当前有效选区；无锚点或锚点与光标重合时为 `None`。
    pub fn selection(&self) -> Option<(usize, usize)> {
        crate::text_selection::resolve_selection(self.cursor, self.anchor)
    }

    /// 是否存在有效选区。
    pub fn has_selection(&self) -> bool {
        self.selection().is_some()
    }

    /// 选区或光标：有选区取有序选区，否则在光标处坍缩。
    pub fn selection_or_cursor(&self) -> (usize, usize) {
        crate::text_selection::selection_or_cursor(self.cursor, self.anchor)
    }
}

#[cfg(test)]
mod tests {
    use super::SharedTextState;

    fn state(value: &str, cursor: usize, anchor: Option<usize>) -> SharedTextState {
        SharedTextState::new(value)
            .with_cursor(cursor)
            .with_anchor(anchor)
    }

    #[test]
    fn shared_state_selection_bounds() {
        let s = state("hello", 2, Some(5));
        assert_eq!(s.selection(), Some((2, 5)));
        let s2 = state("hello", 5, Some(2));
        assert_eq!(s2.selection(), Some((2, 5)));
        let s3 = state("hello", 3, Some(3));
        assert_eq!(s3.selection(), None);
        let s4 = state("hello", 3, None);
        assert_eq!(s4.selection(), None);
    }

    #[test]
    fn new_places_cursor_at_end_without_ime() {
        let s = SharedTextState::new("hello");
        assert_eq!(s.value(), "hello");
        assert_eq!(s.cursor(), "hello".len());
        assert_eq!(s.anchor(), None);
        assert_eq!(s.ime_marked_text(), "");
        assert_eq!(s.ime_replacement(), None);
        assert!(!s.has_selection());
        assert_eq!(s.selection_or_cursor(), ("hello".len(), "hello".len()));
    }

    #[test]
    fn builders_set_every_field() {
        let s = SharedTextState::new("")
            .with_value("hi")
            .with_cursor(1)
            .with_anchor(Some(0))
            .with_ime_marked_text("a")
            .with_ime_replacement(Some((0, 1)));
        assert_eq!(s.value(), "hi");
        assert_eq!(s.cursor(), 1);
        assert_eq!(s.anchor(), Some(0));
        assert_eq!(s.ime_marked_text(), "a");
        assert_eq!(s.ime_replacement(), Some((0, 1)));
        assert!(s.has_selection());
        assert_eq!(s.selection(), Some((0, 1)));
    }

    #[test]
    fn with_value_clamps_cursor_and_drops_oob_anchor() {
        let s = SharedTextState::new("hello")
            .with_cursor(5)
            .with_anchor(Some(4))
            .with_value("hi");
        assert_eq!(s.value(), "hi");
        assert_eq!(s.cursor(), 2);
        assert_eq!(s.anchor(), None);
    }
}

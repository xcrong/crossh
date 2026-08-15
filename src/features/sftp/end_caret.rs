//! 末尾光标输入的共享状态:路径输入(重命名/新建目录)与上传输入同构——
//! 无 cursor/anchor 状态,光标恒在 `value` 末尾,IME 标记也恒在末尾。
//!
//! 归并 `view_input.rs` 里 Path / Upload 两个分支在 `EntityInputHandler` 各方法中
//! 逐字重复的 IME 协议逻辑。多行 `RemoteEditor` 有独立的光标/选区/替换区间状态,
//! 不属于此类(只在 `view_input.rs` 复用 `bounds_for_range` 的字节换算)。
//!
//! 本结构依赖 gpui 的 `Window`/`Bounds<Pixels>`/`UTF16Selection`(IME caret 定位),
//! 因此作为 feature 内的 UI 层输入状态存在,不放共享纯逻辑层。

use std::ops::Range;

use crossh_ui::widgets::{
    byte_index_for_utf16, ime_caret_bounds, replace_utf16_range, utf16_len, utf16_slice,
};
use gpui::{Bounds, Pixels, UTF16Selection, Window, px};

/// 单行、光标恒在末尾的输入状态。
pub struct EndCaretInput {
    pub value: String,
    pub ime_marked_text: String,
}

impl EndCaretInput {
    /// 以文本内容(光标位于末尾)创建新输入。
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            ime_marked_text: String::new(),
        }
    }

    /// 清空文本与 IME 标记。
    pub fn clear(&mut self) {
        self.value.clear();
        self.ime_marked_text.clear();
    }

    /// UTF-16 选区:光标恒在末尾,无真实选区。
    pub fn selection_range(&self) -> UTF16Selection {
        let position = utf16_len(&self.value);
        UTF16Selection {
            range: position..position,
            reversed: false,
        }
    }

    /// IME 标记的 UTF-16 区间(恒在文本末尾);无组合时返回 `None`。
    pub fn marked_range(&self) -> Option<Range<usize>> {
        (!self.ime_marked_text.is_empty()).then(|| {
            let start = utf16_len(&self.value);
            start..start + utf16_len(&self.ime_marked_text)
        })
    }

    /// 清除 IME 标记。
    pub fn unmark(&mut self) {
        self.ime_marked_text.clear();
    }

    /// 在文本末尾执行 UTF-16 区间替换并清除 IME 标记。
    pub fn replace_at_end(&mut self, range: Option<Range<usize>>, text: &str) {
        let position = utf16_len(&self.value);
        replace_utf16_range(&mut self.value, range.unwrap_or(position..position), text);
        self.ime_marked_text.clear();
    }

    /// 记录新组合文本(先清后写)。
    pub fn mark(&mut self, text: &str) {
        self.ime_marked_text.clear();
        self.ime_marked_text.push_str(text);
    }

    /// IME caret 的窗口坐标 bounds;`font_size` / `caret_size` 由输入框样式决定。
    pub fn bounds_for_range(
        &self,
        range: Range<usize>,
        element_bounds: Bounds<Pixels>,
        window: &Window,
        font_size: f32,
        caret_size: f32,
    ) -> Option<Bounds<Pixels>> {
        let cursor = byte_index_for_utf16(&self.value, range.start);
        Some(ime_caret_bounds(
            window,
            element_bounds,
            &self.value[..cursor],
            px(font_size),
            px(caret_size),
            px(0.),
        ))
    }

    /// 输入长度的 UTF-16 计数。
    pub fn length(&self) -> usize {
        utf16_len(&self.value)
    }

    /// 按 UTF-16 区间切片文本。
    pub fn text_for_range(&self, range: Range<usize>) -> String {
        utf16_slice(&self.value, range)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_always_sits_at_the_end() {
        let input = EndCaretInput::new("a中😀");
        assert_eq!(input.selection_range().range, 4..4);
        assert!(!input.selection_range().reversed);
    }

    #[test]
    fn marked_range_is_anchored_at_the_text_end_in_utf16() {
        let mut input = EndCaretInput::new("a中");
        assert_eq!(input.marked_range(), None);
        input.mark("😀");
        assert_eq!(input.marked_range(), Some(2..4));
        assert_eq!(input.ime_marked_text, "😀");
    }

    #[test]
    fn mark_clears_previous_composition_before_recording() {
        let mut input = EndCaretInput::new("");
        input.mark("你");
        input.mark("你好");
        assert_eq!(input.ime_marked_text, "你好");
        assert_eq!(input.marked_range(), Some(0..2));

        input.unmark();
        assert!(input.ime_marked_text.is_empty());
        assert_eq!(input.marked_range(), None);
    }

    #[test]
    fn replace_at_end_inserts_and_replaces_in_utf16_and_clears_mark() {
        let mut input = EndCaretInput::new("a中😀");
        input.ime_marked_text.push('x');
        input.replace_at_end(None, "b");
        assert_eq!(input.value, "a中😀b");
        assert!(input.ime_marked_text.is_empty());

        input.replace_at_end(Some(1..3), "日");
        assert_eq!(input.value, "a日b");
        assert_eq!(input.length(), 3);

        input.replace_at_end(Some(1..2), "😀");
        assert_eq!(input.value, "a😀b");
        assert_eq!(input.length(), 4);
    }

    #[test]
    fn clear_empties_value_and_mark() {
        let mut input = EndCaretInput::new("abc");
        input.mark("x");
        input.clear();
        assert_eq!(input.value, "");
        assert!(input.ime_marked_text.is_empty());
        assert_eq!(input.length(), 0);
        assert_eq!(input.selection_range().range, 0..0);
    }

    #[test]
    fn text_for_range_slices_by_utf16() {
        let input = EndCaretInput::new("a中😀b");
        assert_eq!(input.text_for_range(1..3), "中😀");
    }
}

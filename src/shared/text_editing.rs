//! 文本编辑状态机与 UTF-8 字符边界工具（纯逻辑，零 UI 依赖）。
//!
//! 本模块是应用程序"文本编辑"的共享纯逻辑层，遵循架构红线：
//! **逻辑不依赖 UI**——除标准库文本处理外，这里不允许出现 `gpui` 等任何 UI
//! 依赖（`scripts/check-architecture.sh` 亦会拦截）。焦点、滚动、重绘
//! （`cx.notify()`）等 UI 职责由各 feature 的视图层（`QuickCommandEditor`、
//! `CommitEditor`、设置输入框、SFTP 编辑器）自行承担，本模块只负责状态与编辑运算。
//!
//! `TextEditingState` 统一了各编辑器共享的字段集合（value/cursor/anchor/IME
//! 标记），配合本模块的四个字符边界函数，消除各 feature 之间重复的边界实现。

/// 文本编辑的纯状态：文本内容、光标、选区锚点与 IME 组合状态。
///
/// `value` 为 UTF-8 文本，`cursor`/`anchor`/`ime_replacement` 均为字节索引，
/// 编辑方法保证光标始终落在合法字符边界上。
pub struct TextEditingState {
    pub value: String,
    pub cursor: usize,
    pub anchor: Option<usize>,
    pub ime_marked_text: String,
    pub ime_replacement: Option<(usize, usize)>,
}

impl TextEditingState {
    /// 以完整文本（光标位于末尾）创建新状态。
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

    /// 清空文本与全部编辑状态（含 IME 组合），光标回到 0。
    ///
    /// 供 CommitEditor / 设置输入框作为“提交后重置”入口。
    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
        self.anchor = None;
        self.ime_marked_text.clear();
        self.ime_replacement = None;
    }

    /// 清除 IME 组合状态（标记文本与替换区间），保留文本与光标。
    pub fn clear_composition(&mut self) {
        self.ime_marked_text.clear();
        self.ime_replacement = None;
    }

    /// 当前有效选区；无选区或锚点与光标重合时返回 `None`。
    pub fn selection(&self) -> Option<(usize, usize)> {
        selection_bounds(self.anchor, self.cursor)
    }

    /// 用 `text` 替换当前选区（无选区则插入到光标处），光标移到插入文本末尾。
    pub fn replace_selection(&mut self, text: &str) {
        let (start, end) = self.selection().unwrap_or((self.cursor, self.cursor));
        self.value.replace_range(start..end, text);
        self.cursor = start + text.len();
        self.anchor = None;
    }

    /// 退格：有选区则删除选区；否则删除光标前一字符。
    pub fn backspace(&mut self) {
        if let Some((start, end)) = self.selection() {
            self.value.replace_range(start..end, "");
            self.cursor = start;
            self.anchor = None;
            return;
        }
        let start = previous_char_boundary(&self.value, self.cursor);
        if start != self.cursor {
            self.value.replace_range(start..self.cursor, "");
            self.cursor = start;
        }
    }

    /// 删除：有选区则删除选区；否则删除光标处字符。
    pub fn delete(&mut self) {
        if let Some((start, end)) = self.selection() {
            self.value.replace_range(start..end, "");
            self.cursor = start;
            self.anchor = None;
            return;
        }
        let end = next_char_boundary(&self.value, self.cursor);
        if end != self.cursor {
            self.value.replace_range(self.cursor..end, "");
        }
    }

    /// 左右移动光标；`extend` 时扩展选区，否则从选区两端跳出并清除选区。
    pub fn move_horizontal(&mut self, direction: i8, extend: bool) {
        if !extend && let Some((start, end)) = self.selection() {
            self.cursor = if direction < 0 { start } else { end };
            self.anchor = None;
            return;
        }

        if extend && self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }
        self.cursor = if direction < 0 {
            previous_char_boundary(&self.value, self.cursor)
        } else {
            next_char_boundary(&self.value, self.cursor)
        };
        if !extend {
            self.anchor = None;
        }
    }

    /// 跳到文本首（`end = false`）或文本尾（`end = true`）；`extend` 时保留锚点。
    pub fn move_to_boundary(&mut self, end: bool, extend: bool) {
        if extend && self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }
        self.cursor = if end { self.value.len() } else { 0 };
        if !extend {
            self.anchor = None;
        }
    }

    /// 全选。
    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.value.len();
    }

    /// 当前选区文本。
    pub fn selected_text(&self) -> Option<String> {
        self.selection()
            .map(|(start, end)| self.value[start..end].to_string())
    }
}

/// `cursor` 前一个 UTF-8 字符边界的字节索引；光标已在开头则返回 0。
pub fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

/// `cursor` 处字符的下一个 UTF-8 字符边界的字节索引；光标已在结尾则返回 `cursor`。
pub fn next_char_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .chars()
        .next()
        .map(|character| cursor + character.len_utf8())
        .unwrap_or(cursor)
}

/// 由锚点与光标计算选区边界（左闭右开）；锚点为空或与光标重合时返回 `None`。
pub fn selection_bounds(anchor: Option<usize>, cursor: usize) -> Option<(usize, usize)> {
    let anchor = anchor?;
    (anchor != cursor).then_some(if anchor < cursor {
        (anchor, cursor)
    } else {
        (cursor, anchor)
    })
}

/// 把可能落在 UTF-8 字符内部的字节索引向下收敛到合法字符边界。
pub fn clamp_char_boundary(value: &str, index: usize) -> usize {
    let mut index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_edits_always_leave_cursor_on_a_character_boundary() {
        let mut editor = TextEditingState::new("a中😀b");
        for expected in ["a中😀", "a中", "a", ""] {
            editor.backspace();
            assert_eq!(editor.value, expected);
            assert!(editor.value.is_char_boundary(editor.cursor));
        }

        editor.replace_selection("中😀");
        editor.move_to_boundary(false, false);
        editor.delete();
        assert_eq!(editor.value, "😀");
        assert!(editor.value.is_char_boundary(editor.cursor));
    }

    #[test]
    fn reversed_selection_replacement_collapses_at_inserted_text_end() {
        let mut editor = TextEditingState::new("alpha中omega");
        editor.cursor = 5;
        editor.anchor = Some("alpha中".len());
        assert_eq!(editor.selected_text().as_deref(), Some("中"));

        editor.replace_selection("😀");
        assert_eq!(editor.value, "alpha😀omega");
        assert_eq!(editor.cursor, "alpha😀".len());
        assert_eq!(editor.anchor, None);
    }

    #[test]
    fn horizontal_moves_land_on_boundaries_and_extend_keeps_anchor() {
        let mut editor = TextEditingState::new("a中b");
        editor.move_horizontal(-1, false);
        assert_eq!(editor.cursor, "a中".len());
        assert_eq!(editor.anchor, None);
        editor.move_horizontal(-1, false);
        assert_eq!(editor.cursor, "a".len());

        editor.move_horizontal(1, true);
        assert_eq!(editor.anchor, Some(1));
        assert_eq!(editor.cursor, "a中".len());
        assert_eq!(editor.selected_text().as_deref(), Some("中"));

        editor.move_horizontal(1, false);
        assert_eq!(editor.cursor, "a中".len());
        assert_eq!(editor.anchor, None);
    }

    #[test]
    fn select_all_replace_composition_and_clear_round_trip() {
        let mut editor = TextEditingState::new("你好");
        editor.select_all();
        assert_eq!(editor.selection(), Some((0, "你好".len())));
        editor.replace_selection("x");
        assert_eq!(editor.value, "x");
        assert_eq!(editor.cursor, 1);
        assert_eq!(editor.anchor, None);

        editor.ime_replacement = Some((1, 1));
        editor.ime_marked_text.push('中');
        editor.clear_composition();
        assert!(editor.ime_marked_text.is_empty());
        assert_eq!(editor.ime_replacement, None);

        editor.clear();
        assert_eq!(editor.value, "");
        assert_eq!(editor.cursor, 0);
        assert_eq!(editor.anchor, None);
        assert_eq!(editor.selection(), None);
    }

    #[test]
    fn boundary_helpers_respect_utf8_and_selection_direction() {
        let text = "a中😀b";
        assert_eq!(previous_char_boundary(text, text.len()), "a中😀".len());
        assert_eq!(previous_char_boundary(text, "a".len()), 0);
        assert_eq!(next_char_boundary(text, "a中".len()), "a中😀".len());
        assert_eq!(next_char_boundary(text, text.len()), text.len());

        assert_eq!(selection_bounds(None, 4), None);
        assert_eq!(selection_bounds(Some("a中".len()), "a中".len()), None);
        assert_eq!(
            selection_bounds(Some("a中😀".len()), "a中".len()),
            Some(("a中".len(), "a中😀".len()))
        );
        assert_eq!(
            selection_bounds(Some("a中".len()), "a中😀".len()),
            Some(("a中".len(), "a中😀".len()))
        );

        assert_eq!(clamp_char_boundary("model", 99), 5);
        assert_eq!(clamp_char_boundary("模型", 2), 0);
        assert_eq!(clamp_char_boundary("模型", 3), 3);
    }
}

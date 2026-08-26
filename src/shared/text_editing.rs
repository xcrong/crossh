//! 文本编辑状态机与 UTF-8 字符边界工具（纯逻辑，零 UI 依赖）。
//!
//! 本模块是应用程序"文本编辑"的共享纯逻辑层，遵循架构红线：
//! **逻辑不依赖 UI**——除标准库文本处理外，这里不允许出现任何 UI 框架
//! 依赖（`scripts/check-architecture.sh` 亦会拦截）。焦点、滚动、重绘
//! （`cx.notify()`）等 UI 职责由各 feature 的视图层（`QuickCommandEditor`、
//! `CommitEditor`、设置输入框、SFTP 编辑器）自行承担，本模块只负责状态与编辑运算。
//!
//! `TextEditingState` 统一了各编辑器共享的字段集合（value/cursor/anchor/IME
//! 标记），配合本模块的四个字符边界函数，消除各 feature 之间重复的边界实现。

/// 统一按键分发的结果：是否已消费该按键，以及 `ctrl/cmd+c/x` 时需要写入剪贴板的文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEditingKeyResult {
    pub handled: bool,
    pub copy_text: Option<String>,
}

/// 与编辑分发相关的按键快照；由视图层从 GPUI 的 `Keystroke` 转换而来。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditingKeystroke {
    pub key: String,
    pub key_char: Option<String>,
    pub control: bool,
    pub platform: bool,
    pub shift: bool,
}

/// 将 `TextEditingState` 的通用编辑键（`backspace/delete/left/right/home/end`、
/// `ctrl/cmd+a/c/x/v` 与可打印字符）收敛到单一分发器。
///
/// 调用方负责剪贴板 I/O：`clipboard_text` 传入粘贴内容（`ctrl/cmd+v` 时从 `cx` 读取），
/// 返回的 `copy_text` 若为 `Some` 则由调用方写入剪贴板。
/// 所有会改变文本/光标的分支内部已调用 `clear_composition()`，调用方无需重复。
/// 未匹配的按键返回 `handled: false`。
pub fn handle_text_editing_key(
    state: &mut TextEditingState,
    ks: &EditingKeystroke,
    clipboard_text: Option<&str>,
) -> TextEditingKeyResult {
    let primary = ks.control || ks.platform;
    let extend = ks.shift;
    if primary && ks.key == "a" {
        state.clear_composition();
        state.select_all();
        return TextEditingKeyResult {
            handled: true,
            copy_text: None,
        };
    }
    if primary && matches!(ks.key.as_str(), "c" | "x") {
        if let Some(text) = state.selected_text() {
            let copy = text.clone();
            if ks.key == "x" {
                state.clear_composition();
                state.replace_selection("");
            }
            return TextEditingKeyResult {
                handled: true,
                copy_text: Some(copy),
            };
        }
        return TextEditingKeyResult {
            handled: true,
            copy_text: None,
        };
    }
    if primary && ks.key == "v" {
        if let Some(pasted) = clipboard_text {
            state.clear_composition();
            state.replace_selection(pasted);
        }
        return TextEditingKeyResult {
            handled: true,
            copy_text: None,
        };
    }
    match ks.key.as_str() {
        "backspace" => {
            state.clear_composition();
            state.backspace();
            TextEditingKeyResult {
                handled: true,
                copy_text: None,
            }
        }
        "delete" => {
            state.clear_composition();
            state.delete();
            TextEditingKeyResult {
                handled: true,
                copy_text: None,
            }
        }
        "left" => {
            state.clear_composition();
            state.move_horizontal(-1, extend);
            TextEditingKeyResult {
                handled: true,
                copy_text: None,
            }
        }
        "right" => {
            state.clear_composition();
            state.move_horizontal(1, extend);
            TextEditingKeyResult {
                handled: true,
                copy_text: None,
            }
        }
        "home" => {
            state.clear_composition();
            state.move_to_boundary(false, extend);
            TextEditingKeyResult {
                handled: true,
                copy_text: None,
            }
        }
        "end" => {
            state.clear_composition();
            state.move_to_boundary(true, extend);
            TextEditingKeyResult {
                handled: true,
                copy_text: None,
            }
        }
        _ => {
            if let Some(ch) = printable_key_char(ks) {
                state.clear_composition();
                state.replace_selection(&ch.to_string());
                TextEditingKeyResult {
                    handled: true,
                    copy_text: None,
                }
            } else {
                TextEditingKeyResult {
                    handled: false,
                    copy_text: None,
                }
            }
        }
    }
}

/// 键盘事件的可打印字符；带控制/平台修饰键时返回 None。
fn printable_key_char(ks: &EditingKeystroke) -> Option<char> {
    if ks.control || ks.platform {
        return None;
    }
    ks.key_char.as_ref().and_then(|s| s.chars().next())
}

/// 文本编辑的纯状态：文本内容、光标、选区锚点与 IME 组合状态。
///
/// `value` 为 UTF-8 文本，`cursor`/`anchor`/`ime_replacement` 均为字节索引，
/// 编辑方法保证光标始终落在合法字符边界上。
#[derive(Clone, Debug, PartialEq, Eq)]
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

    /// 用 `text` 替换当前选区（无选区则插入到光标处），光标移到插入文本末尾；
    /// 返回文本是否实际发生变化（选区非空或 `text` 非空）。
    pub fn replace_selection(&mut self, text: &str) -> bool {
        debug_assert!(self.value.is_char_boundary(self.cursor));
        let (start, end) = self.selection().unwrap_or((self.cursor, self.cursor));
        self.value.replace_range(start..end, text);
        self.cursor = start + text.len();
        self.anchor = None;
        !text.is_empty() || start != end
    }

    /// 退格：有选区则删除选区；否则删除光标前一字符。实际删除后清除锚点，
    /// 避免光标与锚点重合时的陈旧锚点演变成幽灵选区。返回是否实际发生了删除。
    pub fn backspace(&mut self) -> bool {
        debug_assert!(self.value.is_char_boundary(self.cursor));
        if let Some((start, end)) = self.selection() {
            self.value.replace_range(start..end, "");
            self.cursor = start;
            self.anchor = None;
            return true;
        }
        let start = previous_char_boundary(&self.value, self.cursor);
        if start != self.cursor {
            self.value.replace_range(start..self.cursor, "");
            self.cursor = start;
            self.anchor = None;
            true
        } else {
            false
        }
    }

    /// 删除：有选区则删除选区；否则删除光标处字符。实际删除后同样清除锚点。
    /// 返回是否实际发生了删除。
    pub fn delete(&mut self) -> bool {
        debug_assert!(self.value.is_char_boundary(self.cursor));
        if let Some((start, end)) = self.selection() {
            self.value.replace_range(start..end, "");
            self.cursor = start;
            self.anchor = None;
            return true;
        }
        let end = next_char_boundary(&self.value, self.cursor);
        if end != self.cursor {
            self.value.replace_range(self.cursor..end, "");
            self.anchor = None;
            true
        } else {
            false
        }
    }

    /// 左右移动光标；`extend` 时扩展选区，否则从选区两端跳出并清除选区。
    /// 返回光标是否实际移动（选区端跳跃无条件返回 `true`）。
    pub fn move_horizontal(&mut self, direction: i8, extend: bool) -> bool {
        debug_assert!(self.value.is_char_boundary(self.cursor));
        if !extend && let Some((start, end)) = self.selection() {
            self.cursor = if direction < 0 { start } else { end };
            self.anchor = None;
            return true;
        }

        if extend && self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }
        let next = if direction < 0 {
            previous_char_boundary(&self.value, self.cursor)
        } else {
            next_char_boundary(&self.value, self.cursor)
        };
        if next == self.cursor {
            return false;
        }
        self.cursor = next;
        if !extend {
            self.anchor = None;
        }
        true
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
    fn non_selection_backspace_clears_stale_anchor() {
        let mut editor = TextEditingState::new("a中b");
        editor.cursor = 4;
        editor.anchor = Some(4);
        editor.backspace();
        assert_eq!(editor.value, "ab");
        assert_eq!(editor.cursor, 1);
        assert_eq!(editor.anchor, None);
    }

    #[test]
    fn non_selection_delete_clears_stale_anchor() {
        let mut editor = TextEditingState::new("a中b");
        editor.cursor = 1;
        editor.anchor = Some(1);
        editor.delete();
        assert_eq!(editor.value, "ab");
        assert_eq!(editor.cursor, 1);
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
    }

    #[test]
    fn spec_20260818_merge_sftp_text_editing_backspace_reports_actual_change() {
        let mut editor = TextEditingState::new("a中b");
        editor.cursor = 0;
        assert!(!editor.backspace());
        assert_eq!(editor.value, "a中b");
        assert_eq!(editor.cursor, 0);

        editor.cursor = "a中".len();
        assert!(editor.backspace());
        assert_eq!(editor.value, "ab");
        assert_eq!(editor.cursor, 1);
        assert_eq!(editor.anchor, None);

        editor.value = "ab".to_string();
        editor.cursor = 1;
        editor.anchor = Some(2);
        assert!(editor.backspace());
        assert_eq!(editor.value, "a");
        assert_eq!(editor.cursor, 1);
        assert_eq!(editor.anchor, None);
    }

    #[test]
    fn spec_20260818_merge_sftp_text_editing_delete_reports_actual_change() {
        let mut editor = TextEditingState::new("a中b");
        editor.cursor = editor.value.len();
        assert!(!editor.delete());
        assert_eq!(editor.value, "a中b");

        editor.cursor = 1;
        assert!(editor.delete());
        assert_eq!(editor.value, "ab");
        assert_eq!(editor.cursor, 1);
        assert_eq!(editor.anchor, None);
    }

    #[test]
    fn spec_20260818_merge_sftp_text_editing_horizontal_move_reports_actual_change() {
        let mut editor = TextEditingState::new("a中b");
        editor.cursor = 0;
        assert!(!editor.move_horizontal(-1, false));
        assert_eq!(editor.cursor, 0);

        assert!(editor.move_horizontal(1, false));
        assert_eq!(editor.cursor, 1);
        editor.cursor = editor.value.len();
        assert!(!editor.move_horizontal(1, false));
        assert_eq!(editor.cursor, editor.value.len());

        editor.cursor = "a中".len();
        editor.anchor = Some(1);
        assert!(editor.move_horizontal(1, false));
        assert_eq!(editor.cursor, "a中".len());
        assert_eq!(editor.anchor, None);

        editor.anchor = Some("a中".len());
        editor.cursor = "a中".len();
        assert!(editor.move_horizontal(-1, false));
        assert_eq!(editor.cursor, 1);
        assert_eq!(editor.anchor, None);
    }

    #[test]
    fn spec_20260818_merge_sftp_text_editing_replace_selection_reports_actual_change() {
        let mut editor = TextEditingState::new("a中b");
        assert!(!editor.replace_selection(""));
        assert_eq!(editor.value, "a中b");
        assert_eq!(editor.cursor, "a中b".len());

        assert!(editor.replace_selection("x"));
        assert_eq!(editor.value, "a中bx");
        assert_eq!(editor.cursor, "a中bx".len());

        editor.cursor = 1;
        editor.anchor = Some("a中".len());
        assert!(editor.replace_selection(""));
        assert_eq!(editor.value, "abx");
        assert_eq!(editor.cursor, 1);
        assert_eq!(editor.anchor, None);
    }

    #[test]
    fn spec_20260818_merge_sftp_text_editing_empty_replace_selection_clears_stale_anchor() {
        let mut editor = TextEditingState::new("ab");
        editor.cursor = 1;
        editor.anchor = Some(1);
        assert!(!editor.replace_selection(""));
        assert_eq!(editor.value, "ab");
        assert_eq!(editor.cursor, 1);
        assert_eq!(editor.anchor, None);
    }

    fn keystroke(
        key: &str,
        control: bool,
        platform: bool,
        shift: bool,
        key_char: Option<&str>,
    ) -> EditingKeystroke {
        EditingKeystroke {
            key: key.to_string(),
            key_char: key_char.map(|s| s.to_string()),
            control,
            platform,
            shift,
        }
    }

    #[test]
    fn handle_text_editing_key_covers_select_all_copy_cut_paste_and_navigation() {
        let mut state = TextEditingState::new("hello");
        state.cursor = 2;
        let result =
            handle_text_editing_key(&mut state, &keystroke("a", true, false, false, None), None);
        assert!(result.handled);
        assert_eq!(state.selection(), Some((0, 5)));
        assert_eq!(result.copy_text, None);

        let result =
            handle_text_editing_key(&mut state, &keystroke("c", true, false, false, None), None);
        assert!(result.handled);
        assert_eq!(result.copy_text.as_deref(), Some("hello"));
        assert_eq!(state.value, "hello");

        let result =
            handle_text_editing_key(&mut state, &keystroke("x", true, false, false, None), None);
        assert!(result.handled);
        assert_eq!(result.copy_text.as_deref(), Some("hello"));
        assert_eq!(state.value, "");
        assert_eq!(state.cursor, 0);

        state.value = "abc".to_string();
        state.cursor = 3;
        let result = handle_text_editing_key(
            &mut state,
            &keystroke("v", true, false, false, None),
            Some("XYZ"),
        );
        assert!(result.handled);
        assert_eq!(state.value, "abcXYZ");

        state.cursor = state.value.len();
        let result = handle_text_editing_key(
            &mut state,
            &keystroke("backspace", false, false, false, None),
            None,
        );
        assert!(result.handled);
        assert_eq!(state.value, "abcXY");

        let result = handle_text_editing_key(
            &mut state,
            &keystroke("left", false, false, false, None),
            None,
        );
        assert!(result.handled);
        assert_eq!(state.cursor, "abcX".len());

        let result = handle_text_editing_key(
            &mut state,
            &keystroke("home", false, false, false, None),
            None,
        );
        assert!(result.handled);
        assert_eq!(state.cursor, 0);

        let result = handle_text_editing_key(
            &mut state,
            &keystroke("a", false, false, false, Some("a")),
            None,
        );
        assert!(result.handled);
        assert_eq!(state.value, "aabcXY");
        assert_eq!(state.cursor, 1);

        state.ime_marked_text = "__".to_string();
        state.ime_replacement = Some((0, 1));
        let result = handle_text_editing_key(
            &mut state,
            &keystroke("end", false, false, false, None),
            None,
        );
        assert!(result.handled);
        assert!(state.ime_marked_text.is_empty());
        assert_eq!(state.ime_replacement, None);
        assert_eq!(state.cursor, state.value.len());
    }

    #[test]
    fn handle_text_editing_key_returns_unhandled_for_unknown_keys() {
        let mut state = TextEditingState::new("hi");
        let result = handle_text_editing_key(
            &mut state,
            &keystroke("f1", false, false, false, None),
            None,
        );
        assert!(!result.handled);
        assert_eq!(result.copy_text, None);
        assert_eq!(state.value, "hi");
    }

    #[test]
    fn handle_text_editing_key_paste_without_clipboard_still_handled() {
        let mut state = TextEditingState::new("hi");
        let before = state.clone();
        let result =
            handle_text_editing_key(&mut state, &keystroke("v", true, false, false, None), None);
        assert!(result.handled);
        assert_eq!(state, before);
    }
}

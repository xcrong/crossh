//! 文本输入管道：收敛 `String` 与 `TextEditingState` 的 IME/选区重复逻辑。

use std::ops::Range;

use super::text_editing::{
    TextEditingState, byte_index_for_utf16, replace_utf16_range, utf16_len, utf16_offset_for_byte,
};

/// UTF-16 坐标系下的选区（与 GPUI 的 `UTF16Selection` 同构，由视图层转换）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Utf16Selection {
    pub range: std::ops::Range<usize>,
    pub reversed: bool,
}

/// `String` 型输入的选区：始终为光标在末尾的空选区。
pub fn plain_selected_range(value: &str) -> Utf16Selection {
    let position = utf16_len(value);
    Utf16Selection {
        range: position..position,
        reversed: false,
    }
}

/// `TextEditingState` 型输入的选区。
pub fn editing_selected_range(state: &TextEditingState) -> Utf16Selection {
    let (start, end) = state.selection().unwrap_or((state.cursor, state.cursor));
    Utf16Selection {
        range: utf16_offset_for_byte(&state.value, start)..utf16_offset_for_byte(&state.value, end),
        reversed: state.anchor.is_some_and(|anchor| anchor > state.cursor),
    }
}

/// `String` 型输入的 IME 标记区间。
pub fn plain_marked_range(value: &str, marked: &str) -> Option<Range<usize>> {
    (!marked.is_empty()).then(|| {
        let start = utf16_len(value);
        start..start + utf16_len(marked)
    })
}

/// `TextEditingState` 型输入的 IME 标记区间。
pub fn editing_marked_range(state: &TextEditingState) -> Option<Range<usize>> {
    let (start, _) = state.ime_replacement?;
    (!state.ime_marked_text.is_empty()).then(|| {
        let start = utf16_offset_for_byte(&state.value, start);
        start..start + utf16_len(&state.ime_marked_text)
    })
}

/// 清除 `TextEditingState` 的 IME 组合状态，保留文本与光标语义。
pub fn editing_unmark(state: &mut TextEditingState) {
    if let Some((start, end)) = state.ime_replacement.take() {
        state.cursor = end;
        state.anchor = (start != end).then_some(start);
    }
    state.ime_marked_text.clear();
}

/// 在 `TextEditingState` 上执行 `replace_text_in_range` 语义。
pub fn editing_replace(
    state: &mut TextEditingState,
    replacement_range: Option<Range<usize>>,
    text: &str,
) {
    let (start, end) = if let Some(range) = state.ime_replacement.take() {
        range
    } else if let Some(range) = replacement_range {
        (
            byte_index_for_utf16(&state.value, range.start),
            byte_index_for_utf16(&state.value, range.end),
        )
    } else {
        state.selection().unwrap_or((state.cursor, state.cursor))
    };
    state.value.replace_range(start..end, text);
    state.cursor = start + text.len();
    state.anchor = None;
    state.ime_marked_text.clear();
}

/// 在 `TextEditingState` 上执行 `replace_and_mark_text_in_range` 语义。
pub fn editing_mark_text(state: &mut TextEditingState, new_text: &str) {
    if state.ime_replacement.is_none() {
        let replacement = state.selection().unwrap_or((state.cursor, state.cursor));
        state.ime_replacement = Some(replacement);
        state.cursor = replacement.0;
        state.anchor = None;
    }
    state.ime_marked_text.clear();
    state.ime_marked_text.push_str(new_text);
}

/// 在 `String` 型输入上执行 `replace_text_in_range` 语义。
pub fn plain_replace(
    value: &mut String,
    marked: &mut String,
    replacement_range: Option<Range<usize>>,
    text: &str,
) {
    let position = utf16_len(value);
    replace_utf16_range(value, replacement_range.unwrap_or(position..position), text);
    marked.clear();
}

/// 在 `String` 型输入上执行 `replace_and_mark_text_in_range` 语义。
pub fn plain_mark(marked: &mut String, new_text: &str) {
    marked.clear();
    marked.push_str(new_text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::text_editing::TextEditingState;

    #[test]
    fn plain_and_editing_selected_ranges_cover_all_fields() {
        let plain = "hi中";
        let range = plain_selected_range(plain);
        assert_eq!(range.range, 3..3);
        assert!(!range.reversed);

        let mut editing = TextEditingState::new("a中b");
        editing.cursor = "a".len();
        editing.anchor = Some("a中".len());
        let e_range = editing_selected_range(&editing);
        assert!(e_range.reversed);
    }

    #[test]
    fn marked_ranges_match_shell_input_semantics() {
        assert_eq!(plain_marked_range("hi", ""), None);
        assert_eq!(plain_marked_range("hi", "中"), Some(2..3));

        let mut state = TextEditingState::new("hello");
        assert_eq!(editing_marked_range(&state), None);
        state.ime_replacement = Some((1, 3));
        state.ime_marked_text = "中".into();
        let range = editing_marked_range(&state).unwrap();
        assert_eq!(range, 1..2);
    }

    #[test]
    fn editing_unmark_clears_and_restores_cursor() {
        let mut state = TextEditingState::new("hello");
        state.ime_replacement = Some((1, 4));
        state.ime_marked_text = "xx".into();
        editing_unmark(&mut state);
        assert_eq!(state.cursor, 4);
        assert_eq!(state.anchor, Some(1));
        assert!(state.ime_marked_text.is_empty());
        assert_eq!(state.ime_replacement, None);
    }

    #[test]
    fn editing_replace_and_mark_round_trip() {
        let mut state = TextEditingState::new("hello");
        state.cursor = 2;
        editing_replace(&mut state, None, "X");
        assert_eq!(state.value, "heXllo");
        assert_eq!(state.cursor, 3);

        let mut state2 = TextEditingState::new("hello");
        state2.cursor = 1;
        editing_mark_text(&mut state2, "中");
        assert_eq!(state2.ime_marked_text, "中");
        assert_eq!(state2.ime_replacement, Some((1, 1)));
    }

    #[test]
    fn plain_replace_and_mark_round_trip() {
        let mut value = "hello".to_string();
        let mut marked = "old".to_string();
        plain_replace(&mut value, &mut marked, Some(1..3), "X");
        assert_eq!(value, "hXlo");
        assert!(marked.is_empty());

        let mut marked2 = String::new();
        plain_mark(&mut marked2, "新");
        assert_eq!(marked2, "新");
    }
}

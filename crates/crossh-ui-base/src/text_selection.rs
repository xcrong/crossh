// Copyright (c) 2026 Crossh contributors.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 纯文本选区护栏：`&str` / `usize` 字节索引计算，零 `gpui` 依赖。
//!
//! 供 `TextInput` 的掩码 / 选区高亮分支与 [`crate::text_state::SharedTextState`]
//! 复用同一份排序 / 钳制 / 有效性语义；非法区间如何回退（caret 重绘）
//! 由调用方（渲染层）决定，本模块只判不画。

/// 选区归一化：`None` 保持 `None`，`Some` 按升序排列端点。
pub fn normalize_selection(selection: Option<(usize, usize)>) -> Option<(usize, usize)> {
    selection.map(|(a, b)| if a <= b { (a, b) } else { (b, a) })
}

/// 把字节索引钳到合法光标位：超界收敛到 `value.len()`，
/// 落在多字节字符内部时同样回退到末尾，避免切片 panic。
pub fn clamp_to_char_boundary(value: &str, index: usize) -> usize {
    let clamped = index.min(value.len());
    if value.is_char_boundary(clamped) {
        clamped
    } else {
        value.len()
    }
}

/// 选区有效性：起止有序、不超界、两端都在字符边界上。
pub fn is_valid_selection(value: &str, start: usize, end: usize) -> bool {
    start <= end
        && end <= value.len()
        && value.is_char_boundary(start)
        && value.is_char_boundary(end)
}

/// 是否渲染选区高亮：仅非掩码（`masked == false`，即 `display` 为空，
/// value 与展示串等长）且选区非空（起止不等）时为真。
pub fn should_highlight_selection(selection: Option<(usize, usize)>, masked: bool) -> bool {
    !masked && selection.is_some_and(|(start, end)| start != end)
}

/// 是否按光标拆分渲染（before / caret / after）：有明确光标且非掩码时为真。
pub fn use_cursor_split(cursor: Option<usize>, masked: bool) -> bool {
    cursor.is_some() && !masked
}

/// 由光标 + 锚点派生有序选区：无锚点或锚点与光标重合时为 `None`。
pub fn resolve_selection(cursor: usize, anchor: Option<usize>) -> Option<(usize, usize)> {
    let anchor = anchor?;
    (anchor != cursor).then_some(if anchor < cursor {
        (anchor, cursor)
    } else {
        (cursor, anchor)
    })
}

/// 选区或光标：有选区取有序选区，否则光标处坍缩为 `(cursor, cursor)`。
pub fn selection_or_cursor(cursor: usize, anchor: Option<usize>) -> (usize, usize) {
    resolve_selection(cursor, anchor).unwrap_or((cursor, cursor))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_orders_or_keeps_none() {
        assert_eq!(normalize_selection(None), None);
        assert_eq!(normalize_selection(Some((2, 5))), Some((2, 5)));
        assert_eq!(normalize_selection(Some((5, 2))), Some((2, 5)));
        assert_eq!(normalize_selection(Some((3, 3))), Some((3, 3)));
    }

    #[test]
    fn clamp_falls_back_to_len_on_mid_char_and_overflow() {
        let value = "h\u{00e9}llo";
        assert_eq!(clamp_to_char_boundary(value, 0), 0);
        assert_eq!(clamp_to_char_boundary(value, 1), 1);
        // `é` 占两字节：索引 2 落在字符内部，回退到末尾。
        assert_eq!(clamp_to_char_boundary(value, 2), value.len());
        assert_eq!(clamp_to_char_boundary(value, 3), 3);
        // 超界收敛到末尾；空串任何索引都收敛到 0。
        assert_eq!(clamp_to_char_boundary(value, value.len() + 10), value.len());
        assert_eq!(clamp_to_char_boundary("", 7), 0);
    }

    #[test]
    fn validity_rejects_mid_char_overflow_and_reversed() {
        let value = "h\u{00e9}llo";
        assert!(is_valid_selection(value, 0, value.len()));
        assert!(is_valid_selection("", 0, 0));
        assert!(!is_valid_selection(value, 1, 2));
        assert!(!is_valid_selection(value, 2, 3));
        assert!(!is_valid_selection(value, 0, value.len() + 1));
        assert!(!is_valid_selection(value, 4, 3));
    }

    #[test]
    fn highlight_only_when_unmasked_and_non_empty() {
        assert!(should_highlight_selection(Some((1, 3)), false));
        assert!(!should_highlight_selection(Some((3, 3)), false));
        assert!(!should_highlight_selection(None, false));
        assert!(!should_highlight_selection(Some((1, 3)), true));
        assert!(!should_highlight_selection(None, true));
    }

    #[test]
    fn cursor_split_only_when_cursor_given_and_unmasked() {
        assert!(use_cursor_split(Some(2), false));
        assert!(!use_cursor_split(None, false));
        assert!(!use_cursor_split(Some(2), true));
        assert!(!use_cursor_split(None, true));
    }

    #[test]
    fn resolve_returns_ordered_pair_or_none() {
        assert_eq!(resolve_selection(2, Some(5)), Some((2, 5)));
        assert_eq!(resolve_selection(5, Some(2)), Some((2, 5)));
        // 锚点与光标重合或缺失时无选区。
        assert_eq!(resolve_selection(3, Some(3)), None);
        assert_eq!(resolve_selection(3, None), None);
    }

    #[test]
    fn selection_or_cursor_collapses_without_selection() {
        assert_eq!(selection_or_cursor(2, Some(5)), (2, 5));
        assert_eq!(selection_or_cursor(3, Some(3)), (3, 3));
        assert_eq!(selection_or_cursor(3, None), (3, 3));
    }
}

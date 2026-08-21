//! UTF-16 坐标系与 UTF-8 字节索引之间的换算工具（纯逻辑，零 UI 依赖）。
//!
//! AppKit IME / GPUI 文本输入系统以 UTF-16 code unit 为偏移单位，
//! 而本应用的编辑状态以 UTF-8 字节索引为准。本模块收敛两者之间的
//! 换算与替换逻辑，供各 feature 的输入处理层复用。

use std::ops::Range;

/// Count UTF-16 code units, which is the indexing scheme used by AppKit IME APIs.
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Convert a UTF-16 offset to the nearest valid UTF-8 byte boundary.
pub fn byte_index_for_utf16(text: &str, offset: usize) -> usize {
    if offset == 0 {
        return 0;
    }

    let mut utf16_offset = 0;
    for (byte_index, ch) in text.char_indices() {
        if utf16_offset >= offset {
            return byte_index;
        }
        utf16_offset += ch.len_utf16();
        if utf16_offset >= offset {
            return byte_index + ch.len_utf8();
        }
    }
    text.len()
}

pub fn utf16_offset_for_byte(text: &str, byte_index: usize) -> usize {
    text[..byte_index].encode_utf16().count()
}

pub fn utf16_slice(text: &str, range: Range<usize>) -> String {
    let start = byte_index_for_utf16(text, range.start.min(range.end));
    let end = byte_index_for_utf16(text, range.end.max(range.start));
    text[start..end].to_string()
}

pub fn replace_utf16_range(text: &mut String, range: Range<usize>, replacement: &str) -> usize {
    let start = byte_index_for_utf16(text, range.start.min(range.end));
    let end = byte_index_for_utf16(text, range.end.max(range.start));
    text.replace_range(start..end, replacement);
    start + replacement.len()
}

#[cfg(test)]
mod tests {
    use super::{byte_index_for_utf16, replace_utf16_range, utf16_len, utf16_slice};

    #[test]
    fn utf16_helpers_handle_cjk_and_surrogate_pairs() {
        let text = "a中😀b";

        assert_eq!(utf16_len(text), 5);
        assert_eq!(byte_index_for_utf16(text, 0), 0);
        assert_eq!(byte_index_for_utf16(text, 1), 1);
        assert_eq!(byte_index_for_utf16(text, 2), 4);
        assert_eq!(byte_index_for_utf16(text, 3), 8);
        assert_eq!(byte_index_for_utf16(text, 4), 8);
        assert_eq!(byte_index_for_utf16(text, 5), 9);
        assert_eq!(utf16_slice(text, 1..3), "中😀");

        let mut replaced = text.to_string();
        replace_utf16_range(&mut replaced, 1..3, "中文");
        assert_eq!(replaced, "a中文b");
    }
}

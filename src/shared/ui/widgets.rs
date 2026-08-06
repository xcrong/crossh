//! 跨 UI 模块复用的小组件与工具函数。

use gpui::{
    Bounds, ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Font, Hsla, Keystroke,
    ParentElement, Pixels, Point, Render, SharedString, Styled, TextRun, Window, canvas, div, px,
    size,
};

use std::ops::Range;

use crate::shared::ui::theme;

/// 简单的纯文本 tooltip：按内容收缩，长文本最多 480px 并自动换行。
pub struct LocalPathTooltip {
    pub path: SharedString,
}

impl Render for LocalPathTooltip {
    fn render(
        &mut self,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        div()
            .w_auto()
            .max_w(px(480.))
            .px_2()
            .py_1()
            .bg(theme::raised())
            .border_1()
            .border_color(theme::border_strong())
            .text_xs()
            .text_color(theme::text())
            .whitespace_normal()
            .child(self.path.clone())
    }
}

/// Full command preview for quick-command rows.
pub struct CommandTooltip {
    pub command: SharedString,
}

impl Render for CommandTooltip {
    fn render(
        &mut self,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        div()
            .w_auto()
            .max_w(px(520.))
            .px_3()
            .py_2()
            .bg(theme::canvas())
            .border_1()
            .border_color(theme::border_strong())
            .text_xs()
            .text_color(theme::text())
            .whitespace_normal()
            .child(self.command.clone())
    }
}

/// Focused text fields use a small explicit caret because these fields are
/// rendered as GPUI layout elements rather than native text inputs.
pub fn text_caret(height: gpui::Pixels) -> impl gpui::IntoElement {
    div()
        .w(px(2.))
        .h(height)
        .flex_shrink_0()
        .rounded(px(1.))
        .bg(theme::accent())
}

/// Register the focused element with the platform text input system during paint.
pub fn ime_input_canvas<V>(focus: FocusHandle, entity: Entity<V>) -> impl gpui::IntoElement
where
    V: EntityInputHandler + 'static,
{
    canvas(
        |_bounds, _window, _cx| (),
        move |bounds, _state, window, cx| {
            window.handle_input(&focus, ElementInputHandler::new(bounds, entity.clone()), cx);
        },
    )
    .absolute()
    .left_0()
    .top_0()
    .size_full()
}

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

/// Measure text using the same system UI font family used by GPUI's input fields.
pub fn text_width(window: &Window, text: &str, font_size: Pixels) -> Pixels {
    if text.is_empty() {
        return px(0.);
    }

    let run = TextRun {
        len: text.len(),
        font: Font::default(),
        color: Hsla::default(),
        ..Default::default()
    };
    window
        .text_system()
        .shape_line(
            SharedString::from(text.to_string()),
            font_size,
            &[run],
            None,
        )
        .width()
}

/// Return the caret/candidate rectangle for a single-line input field.
pub fn ime_caret_bounds(
    window: &Window,
    element_bounds: Bounds<Pixels>,
    text_before_cursor: &str,
    font_size: Pixels,
    prefix_width: Pixels,
    scroll_x: Pixels,
) -> Bounds<Pixels> {
    let line_height = px((font_size.as_f32() * 1.25).max(14.));
    let vertical_offset =
        px(((element_bounds.size.height.as_f32() - line_height.as_f32()) / 2.0).max(0.0));
    let x =
        element_bounds.origin.x + prefix_width + text_width(window, text_before_cursor, font_size)
            - scroll_x;
    Bounds {
        origin: Point::new(x, element_bounds.origin.y + vertical_offset),
        size: size(px(1.), line_height),
    }
}

/// 键盘事件的可打印字符；带控制/平台修饰键时返回 None。
pub fn printable_char(ks: &Keystroke) -> Option<char> {
    if ks.modifiers.control || ks.modifiers.platform {
        return None;
    }
    ks.key_char.as_ref().and_then(|s| s.chars().next())
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

//! 跨 UI 模块复用的小组件与工具函数。

use gpui::{
    AnyElement, Bounds, ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Font, Hsla,
    IntoElement, Keystroke, ParentElement, Pixels, Point, SharedString, Styled, TextRun, Window,
    canvas, div, px, size,
};

use crate::theme;

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

/// 单段纯文本片段：flex 行内不收缩、不折行。
/// 用于输入框等按段拼装的排版，保证分段边界与滚动行为一致。
pub fn text_span(text: impl Into<SharedString>) -> AnyElement {
    div()
        .flex_shrink_0()
        .whitespace_nowrap()
        .child(text.into())
        .into_any_element()
}

/// IME 组合标记文本片段：下划线 + 主题强调色，用于输入法组合中的待确认文本。
/// 与 `text_span` 同结构，仅多下划线强调，替换两端文本时需要保持视觉等价。
pub fn marked_text_span(text: impl Into<SharedString>) -> AnyElement {
    div()
        .flex_shrink_0()
        .whitespace_nowrap()
        .underline()
        .text_decoration_color(theme::accent())
        .child(text.into())
        .into_any_element()
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

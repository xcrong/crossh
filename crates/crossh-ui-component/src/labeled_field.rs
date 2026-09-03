//! 带标签的表单行：固定宽度的 faint 标签 + 右侧输入区。
//!
//! 把「标签 + 输入框」横向行的骨架收敛到一处：多行并列时，
//! 对齐（`items_center` + `gap_2`）与标签列宽只用写一次，
//! 避免某行漏写修饰导致输入框错位。

use gpui::{
    Div, ElementId, FocusHandle, InteractiveElement, IntoElement, ParentElement, SharedString,
    Stateful, StatefulInteractiveElement, Styled, div, px,
};

use crate::theme;

/// 标签列固定宽度；中英文短标签（名称/Name/URL/地址）均不换行不挤压。
pub const LABELED_FIELD_LABEL_WIDTH: f32 = 40.;

/// 横向表单行：`[label | input]`，点击行内空白聚焦输入框。
///
/// 输入框由调用方完整构造传入（含 `.flex_1()` 与自身的 id/焦点/按键处理），
/// 本函数只拥有行的对齐骨架与标签列。
pub fn labeled_field(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    focus: FocusHandle,
    input: impl IntoElement,
) -> Stateful<Div> {
    div()
        .id(id)
        .w_full()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .w(px(LABELED_FIELD_LABEL_WIDTH))
                .flex_shrink_0()
                .text_xs()
                .text_color(theme::faint_text())
                .child(label.into()),
        )
        .child(input)
        .on_click(move |_event, window, cx| {
            window.focus(&focus, cx);
            cx.stop_propagation();
        })
}

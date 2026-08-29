use gpui::{
    AnyElement, Div, FocusHandle, InteractiveElement, IntoElement, ParentElement, SharedString,
    Stateful, StatefulInteractiveElement, Styled, div,
};

use crate::theme;

/// 统一的列表容器骨架，收敛 `git/` 4 个页面重复的 pane 容器样式。
///
/// 封装 `size_full + min_h_0 + flex_col + bg(sidebar) + focus_ring` 以及
/// `track_focus + tab_stop + on_click focus` 的焦点样板，调用方仅需
/// 串联 `key_context`、工具栏与 `on_action`。
pub fn list_pane(
    id: impl Into<SharedString>,
    focus: FocusHandle,
    key_context: &'static str,
) -> Stateful<Div> {
    let focus_for_click = focus.clone();
    div()
        .id(id.into())
        .key_context(key_context)
        .track_focus(&focus)
        .tab_stop(true)
        .size_full()
        .min_h_0()
        .flex()
        .flex_col()
        .bg(theme::sidebar())
        .focus(|style| style.border_color(theme::focus_ring()))
        .on_click(move |_event, window, cx| window.focus(&focus_for_click, cx))
}

/// 统一的 `OperationState::Error` 横幅，收敛 `branch` / `stash` 重复的
/// `border_b_1 + danger` 5 行样板。
pub fn pane_operation_error(message: SharedString) -> AnyElement {
    div()
        .w_full()
        .flex_shrink_0()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(theme::danger())
        .text_xs()
        .text_color(theme::danger())
        .child(message)
        .into_any_element()
}

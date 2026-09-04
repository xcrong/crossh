//! 列表筛选条：统一外边距行 + 框内搜索图标 + 单行输入统一样式。
//!
//! 收敛三处的筛选输入：主侧栏项目筛选、Git 提交历史搜索、Note 笔记筛选。
//! 三家统一为 `TextEditingState` + `shared::text_editing::handle_text_editing_key`
//! 的编辑语义（插入/删除/光标/选区/剪贴板/IME），各家仅保留提交语义与过滤目标差异：
//! 侧栏 Enter 打开匹配项目，Git Esc 回列表，Note Esc 先清空（空时才关窗口）。
//! 外边距由 [`filter_row`] 统一为 `m_2`（含 Git 工具栏内的行），本组件不碰各家的输入状态机与提交回调。

use gpui::{
    Div, ElementId, FocusHandle, InteractiveElement, Rgba, SharedString, Stateful, Styled, div,
};

use crossh_ui::{icons, theme};

use crate::text_input::TextInput;

/// 筛选输入的文字颜色：空值用 faint（与 placeholder 同色系），非空用正文色。
pub fn filter_text_color(is_empty: bool) -> Rgba {
    if is_empty {
        theme::faint_text()
    } else {
        theme::text()
    }
}
/// 筛选条外边距行：三处统一 `m_2` 呼吸空间。
/// 调用方继续 `.child(输入框)`，有尾部操作（如 Git 刷新按钮）再追加 `.child(...)`。
pub fn filter_row(id: impl Into<SharedString>) -> Stateful<Div> {
    div().id(id.into()).m_2().flex().items_center().gap_2()
}

/// 筛选框统一样式：值/占位/IME + 选区/光标透传 + 空值颜色 + `surface` 背景 +
/// 横向撑满 + 框内后缀搜索图标。
///
/// 调用方仍需链式追加 `.entity(...).on_key_down(...)`（`TextInput` 要求 entity 注册 IME）。
pub fn filter_text_input<V>(
    id: impl Into<ElementId>,
    focus: FocusHandle,
    value: impl Into<SharedString>,
    placeholder: impl Into<SharedString>,
    ime_marked_text: impl Into<SharedString>,
    selection: Option<(usize, usize)>,
    cursor: usize,
) -> TextInput<V> {
    let value = value.into();
    let empty = value.as_ref().is_empty();
    TextInput::new(id, focus)
        .value(value)
        .placeholder(placeholder)
        .ime_marked_text(ime_marked_text)
        .selection(selection)
        .cursor(cursor)
        .text_color(filter_text_color(empty))
        .bg(theme::surface())
        .flex_1()
        .suffix_icon(icons::IconName::Search)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_text_color_tracks_emptiness() {
        assert_eq!(filter_text_color(true), theme::faint_text());
        assert_eq!(filter_text_color(false), theme::text());
    }
}

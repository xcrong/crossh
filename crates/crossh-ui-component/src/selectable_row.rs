use gpui::{Div, InteractiveElement, Pixels, SharedString, Stateful, Styled, div};

use crate::theme;

/// 统一的选中行骨架，收敛 `git/` 4 个页面重复的
/// `border_l_2 + border_color + bg + hover + cursor_pointer` 样板。
///
/// 高度由调用方传入（`px(34)` / `px(60)` / `px(68)` / `px(78)` 等），
/// 布局方向与内边距由调用方在返回的 `Div` 上继续链式追加，
/// 避免将 `flex_col` / `pr_2` / `px_3` 等页面差异硬编码进组件。
pub fn selectable_row(
    id: impl Into<SharedString>,
    selected: bool,
    height: Pixels,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .h(height)
        .w_full()
        .cursor_pointer()
        .border_l_2()
        .border_color(if selected {
            theme::accent()
        } else {
            theme::sidebar()
        })
        .bg(if selected {
            theme::raised()
        } else {
            theme::sidebar()
        })
        .hover(|style| style.bg(theme::raised()))
}

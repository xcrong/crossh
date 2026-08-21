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

/// 结构化封装的 `SelectableRow`，与 [`selectable_row`] 函数等价，满足
/// Issue P0-2 建议的 `SelectableRow { id, selected, height, on_select }` 形态。
///
/// `on_select` 回调由调用方在返回的 `Div` 上通过 `.on_click` 追加，
/// 组件仅负责一致的选中态边框与背景，保持无状态与可组合。
#[derive(Clone)]
pub struct SelectableRow {
    id: SharedString,
    selected: bool,
    height: Pixels,
}

impl SelectableRow {
    pub fn new(id: impl Into<SharedString>, selected: bool, height: impl Into<Pixels>) -> Self {
        Self {
            id: id.into(),
            selected,
            height: height.into(),
        }
    }

    /// 产出与 [`selectable_row`] 等价的骨架 `Div`。
    pub fn scaffold(self) -> Stateful<Div> {
        selectable_row(self.id, self.selected, self.height)
    }

    /// 别名，保持与 `ListPane::div` 对称的调用体验。
    pub fn div(self) -> Stateful<Div> {
        self.scaffold()
    }
}

#[cfg(test)]
mod tests {
    use gpui::px;

    use super::{SelectableRow, selectable_row};

    #[test]
    fn selectable_row_builders_are_chainable() {
        let _row = selectable_row("row-1", true, px(34.));
        let _row2 = selectable_row("row-2", false, px(60.));
        let _row3 = SelectableRow::new("row-3", true, px(68.)).scaffold();
        let _row4 = SelectableRow::new("row-4", false, px(78.)).div();
    }
}

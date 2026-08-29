use gpui::{Div, FontWeight, IntoElement, ParentElement, SharedString, Styled, div, px};

use crossh_ui::{icons, theme};

/// 统一的 Pane 顶部工具栏，收敛 `branch_render` / `stash_render` /
/// `history_render` / `render` 中重复的 `h(38).px_3.flex.items_center.gap_2.bg(surface).border_b_1` 样板。
///
/// 高度与间距在组件内固化，调用方仅需通过 [`PaneToolbar::new`] 传入标题与图标，
/// 再链式追加操作按钮：`PaneToolbar::new(title, icon).child(btn).into_any_element()`。
/// 搜索类工具栏可直接使用 [`pane_toolbar`] 基座自行组合子元素。
pub fn pane_toolbar() -> Div {
    div()
        .h(px(38.))
        .flex_shrink_0()
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .bg(theme::surface())
        .border_b_1()
        .border_color(theme::border())
}

pub struct PaneToolbar {
    inner: Div,
}

impl PaneToolbar {
    pub fn new(title: impl Into<SharedString>, icon: icons::IconName) -> Self {
        let inner = pane_toolbar()
            .child(icons::icon(icon, 14.).text_color(theme::accent()))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text())
                    .child(title.into()),
            )
            .child(div().flex_1());
        Self { inner }
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.inner = self.inner.child(child);
        self
    }

    pub fn into_any_element(self) -> gpui::AnyElement {
        self.inner.into_any_element()
    }
}

impl IntoElement for PaneToolbar {
    type Element = Div;

    fn into_element(self) -> Self::Element {
        self.inner
    }
}

//! 轻量提示文本：`text_xs` + `faint_text` 的小字号占位提示。

use gpui::{
    App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div,
    prelude::FluentBuilder, px,
};

use crate::theme;

/// 单行小字号提示文本(如空列表、加载中、无选中项)。
///
/// 默认无内边距、无圆角;`centered()` 模式会包一层
/// `flex_1().flex().items_center().justify_center()` 的居中容器,
/// 内部提示带固定的 16px 内边距(与 `p_4` 等价)。
/// 依赖父容器为 flex,否则 `flex_1` 退化为内容宽度。
#[derive(IntoElement)]
pub struct Hint {
    text: SharedString,
    padding_x: gpui::Pixels,
    padding_y: gpui::Pixels,
    radius: Option<gpui::Pixels>,
    centered: bool,
}

impl Hint {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            padding_x: px(0.),
            padding_y: px(0.),
            radius: None,
            centered: false,
        }
    }

    pub fn padding_x(mut self, padding: impl Into<gpui::Pixels>) -> Self {
        self.padding_x = padding.into();
        self
    }

    pub fn padding_y(mut self, padding: impl Into<gpui::Pixels>) -> Self {
        self.padding_y = padding.into();
        self
    }

    /// 可选圆角,默认无。
    pub fn radius(mut self, radius: impl Into<gpui::Pixels>) -> Self {
        self.radius = Some(radius.into());
        self
    }

    /// 列表空态统一样式：`px(12)` + `py(16)`，收敛 15+ 处重复的 `padding_x/y` 样板。
    pub fn padded(mut self) -> Self {
        self.padding_x = px(12.);
        self.padding_y = px(16.);
        self
    }

    /// 在父容器中水平垂直居中。
    ///
    /// 包一层 `flex_1().flex().items_center().justify_center()` 容器,
    /// 内部提示带固定 16px 内边距(与 `p_4` 等价),此时外层容器高为其
    /// 内容高度,`flex_1` 撑满剩余空间依赖父元素为 flex 布局。
    pub fn centered(mut self) -> Self {
        self.centered = true;
        self
    }
}

impl RenderOnce for Hint {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let inner = div()
            .text_xs()
            .text_color(theme::faint_text())
            .when(self.centered, |inner| inner.px(px(16.)).py(px(16.)))
            .when(!self.centered, |inner| {
                inner.px(self.padding_x).py(self.padding_y)
            })
            .when_some(self.radius, |inner, radius| inner.rounded(radius))
            .child(self.text);
        if self.centered {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(inner)
        } else {
            inner
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::px;

    use super::Hint;

    #[test]
    fn hint_defaults_to_no_padding_radius_or_centering() {
        let hint = Hint::new("loading");
        assert_eq!(hint.text.as_ref(), "loading");
        assert_eq!(hint.padding_x, px(0.));
        assert_eq!(hint.padding_y, px(0.));
        assert_eq!(hint.radius, None);
        assert!(!hint.centered);
    }

    #[test]
    fn hint_builder_sets_padding_radius_and_centered() {
        let hint = Hint::new("nothing here")
            .padding_x(px(8.))
            .padding_y(px(16.))
            .radius(px(4.))
            .centered();
        assert_eq!(hint.padding_x, px(8.));
        assert_eq!(hint.padding_y, px(16.));
        assert_eq!(hint.radius, Some(px(4.)));
        assert!(hint.centered);
    }

    #[test]
    fn hint_padded_sets_unified_empty_padding() {
        let hint = Hint::new("empty").padded();
        assert_eq!(hint.padding_x, px(12.));
        assert_eq!(hint.padding_y, px(16.));
    }
}

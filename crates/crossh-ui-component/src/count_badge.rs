//! 紧凑计数 pill：`rounded_full` + `raised` 背景的小号数字徽标。

use gpui::{
    App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div,
    prelude::FluentBuilder, px,
};

use crate::theme;

/// 紧凑的计数 pill(如分组标题的 host 数)。
///
/// 默认固定 `min_width`/`height` 为 18px,内容垂直水平居中;
/// `unbounded()` 去掉固定尺寸,由 flex 按内容 + 内边距自适配。
#[derive(IntoElement)]
pub struct CountBadge {
    label: SharedString,
    min_width: gpui::Pixels,
    height: gpui::Pixels,
    padding_x: gpui::Pixels,
    padding_y: gpui::Pixels,
    unbounded: bool,
}

impl CountBadge {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            min_width: px(18.),
            height: px(18.),
            padding_x: px(4.),
            padding_y: px(4.),
            unbounded: false,
        }
    }

    pub fn min_width(mut self, min_width: impl Into<gpui::Pixels>) -> Self {
        self.min_width = min_width.into();
        self
    }

    pub fn padding_x(mut self, padding: impl Into<gpui::Pixels>) -> Self {
        self.padding_x = padding.into();
        self
    }

    pub fn padding_y(mut self, padding: impl Into<gpui::Pixels>) -> Self {
        self.padding_y = padding.into();
        self
    }

    /// 去掉固定 `min_width`/`height`,尺寸由内容 + 内边距决定。
    pub fn unbounded(mut self) -> Self {
        self.unbounded = true;
        self
    }
}

impl RenderOnce for CountBadge {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .when(!self.unbounded, |el| {
                el.min_w(self.min_width).h(self.height)
            })
            .px(self.padding_x)
            .py(self.padding_y)
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(theme::raised())
            .text_xs()
            .text_color(theme::muted_text())
            .child(self.label)
    }
}

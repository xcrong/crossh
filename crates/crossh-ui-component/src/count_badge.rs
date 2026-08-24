//! 紧凑计数 pill：`rounded_full` + `raised` 背景的小号数字徽标。

use gpui::{
    App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div,
    prelude::FluentBuilder, px,
};

use crate::theme;

/// 紧凑的计数 pill(如分组标题的 host 数、quick commands 的进度计数)。
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

#[cfg(test)]
mod tests {
    use gpui::px;

    use super::CountBadge;

    #[test]
    fn count_badge_defaults_to_fixed_18px_size_with_4px_padding() {
        let badge = CountBadge::new("3");
        assert_eq!(badge.label.as_ref(), "3");
        assert_eq!(badge.min_width, px(18.));
        assert_eq!(badge.padding_x, px(4.));
        assert_eq!(badge.padding_y, px(4.));
        assert!(!badge.unbounded);
    }

    #[test]
    fn count_badge_builder_sets_size_padding_and_unbounded() {
        let badge = CountBadge::new(format!("{}/{}", 12, 50))
            .unbounded()
            .min_width(px(20.))
            .padding_x(px(8.))
            .padding_y(px(4.));
        assert!(badge.unbounded);
        assert_eq!(badge.min_width, px(20.));
        assert_eq!(badge.padding_x, px(8.));
        assert_eq!(badge.padding_y, px(4.));
    }
}

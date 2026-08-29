use gpui::{IntoElement, ParentElement, Render, SharedString, Styled, Window, div, px};

use crate::theme;

/// A compact text tooltip.
///
/// Rendered as an entity (GPUI's `.tooltip(...)` expects an `AnyView`), so
/// builders are set before the entity is created:
/// `cx.new(|_| Tooltip::new(text).wide()).into()`.
#[derive(Clone)]
pub struct Tooltip {
    text: SharedString,
    wide: bool,
}

impl Tooltip {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            wide: false,
        }
    }

    /// Roomier variant for longer content such as full command previews:
    /// canvas background and a wider max width.
    pub fn wide(mut self) -> Self {
        self.wide = true;
        self
    }
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let mut tip = div()
            .border_1()
            .border_color(theme::border_strong())
            .rounded(px(theme::RADIUS_SM))
            .text_xs()
            .text_color(theme::text())
            .whitespace_normal()
            .child(self.text.clone());
        if self.wide {
            tip = tip.max_w(px(520.)).px_3().py_2().bg(theme::canvas());
        } else {
            tip = tip.max_w(px(480.)).px_2().py_1().bg(theme::raised());
        }
        tip
    }
}

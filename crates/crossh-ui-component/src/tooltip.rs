use gpui::{IntoElement, ParentElement, Render, SharedString, Styled, Window, div, px};

use crate::theme;

pub struct Tooltip {
    pub text: SharedString,
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(360.))
            .px_2()
            .py_1()
            .bg(theme::raised())
            .border_1()
            .border_color(theme::border_strong())
            .rounded(px(theme::RADIUS_SM))
            .text_xs()
            .text_color(theme::text())
            .whitespace_normal()
            .child(self.text.clone())
    }
}

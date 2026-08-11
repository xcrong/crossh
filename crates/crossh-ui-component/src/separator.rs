use gpui::{App, IntoElement, RenderOnce, Styled, div, prelude::FluentBuilder, px};

use crate::theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SeparatorOrientation {
    #[default]
    Horizontal,
    Vertical,
}

/// A one-pixel separator for component layouts.
#[derive(IntoElement)]
pub struct Separator {
    orientation: SeparatorOrientation,
}

impl Separator {
    pub fn horizontal() -> Self {
        Self {
            orientation: SeparatorOrientation::Horizontal,
        }
    }

    pub fn vertical() -> Self {
        Self {
            orientation: SeparatorOrientation::Vertical,
        }
    }

    pub fn orientation(mut self, orientation: SeparatorOrientation) -> Self {
        self.orientation = orientation;
        self
    }
}

impl RenderOnce for Separator {
    fn render(self, _window: &mut gpui::Window, _cx: &mut App) -> impl IntoElement {
        div()
            .flex_shrink_0()
            .bg(theme::border())
            .when(
                self.orientation == SeparatorOrientation::Horizontal,
                |this| this.w_full().h(px(1.)),
            )
            .when(self.orientation == SeparatorOrientation::Vertical, |this| {
                this.w(px(1.)).h_full()
            })
    }
}

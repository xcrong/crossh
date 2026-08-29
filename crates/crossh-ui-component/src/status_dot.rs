use gpui::{App, IntoElement, Pixels, RenderOnce, Rgba, Styled, Window, div, px};

/// A small circular status indicator colored by state.
///
/// The component only knows colors; mapping business states to colors is
/// owned by the features that build the dot.
#[derive(IntoElement)]
pub struct StatusDot {
    color: Rgba,
    size: Pixels,
    border: Option<Rgba>,
}

impl StatusDot {
    pub fn new(color: Rgba) -> Self {
        Self {
            color,
            size: px(6.),
            border: None,
        }
    }

    pub fn size(mut self, size: impl Into<Pixels>) -> Self {
        self.size = size.into();
        self
    }

    pub fn border(mut self, color: Rgba) -> Self {
        self.border = Some(color);
        self
    }
}

impl RenderOnce for StatusDot {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut dot = div()
            .w(self.size)
            .h(self.size)
            .rounded_full()
            .bg(self.color);
        if let Some(border) = self.border {
            dot = dot.border_1().border_color(border);
        }
        dot
    }
}

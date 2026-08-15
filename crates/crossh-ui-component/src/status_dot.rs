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

#[cfg(test)]
mod tests {
    use gpui::px;

    use super::StatusDot;

    const ACCENT: gpui::Rgba = gpui::Rgba {
        r: 1.0,
        g: 0.5,
        b: 0.0,
        a: 1.0,
    };

    #[test]
    fn status_dot_defaults_to_six_pixels_without_border() {
        let dot = StatusDot::new(ACCENT);
        assert_eq!(dot.color, ACCENT);
        assert_eq!(dot.size, px(6.));
        assert_eq!(dot.border, None);
    }

    #[test]
    fn status_dot_builder_sets_size_and_border() {
        let dot = StatusDot::new(ACCENT).size(px(10.)).border(ACCENT);
        assert_eq!(dot.size, px(10.));
        assert_eq!(dot.border, Some(ACCENT));
    }
}

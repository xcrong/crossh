use gpui::{App, IntoElement, ParentElement, RenderOnce, SharedString, Styled};

use crate::badge::BadgeTone;
use crate::layout::h_flex;
use crate::theme;

/// A compact text-only metric for status bars and other dense chrome.
#[derive(IntoElement)]
pub struct StatusMetric {
    label: SharedString,
    tone: BadgeTone,
}

impl StatusMetric {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            tone: BadgeTone::default(),
        }
    }

    pub fn tone(mut self, tone: BadgeTone) -> Self {
        self.tone = tone;
        self
    }
}

impl RenderOnce for StatusMetric {
    fn render(self, _window: &mut gpui::Window, _cx: &mut App) -> impl IntoElement {
        let foreground = match self.tone {
            BadgeTone::Neutral => theme::muted_text(),
            BadgeTone::Accent | BadgeTone::Success => theme::accent(),
            BadgeTone::Info => theme::info(),
            BadgeTone::Warning => theme::warning(),
            BadgeTone::Danger => theme::danger(),
        };
        h_flex()
            .flex_shrink_0()
            .text_xs()
            .text_color(foreground)
            .child(self.label)
    }
}

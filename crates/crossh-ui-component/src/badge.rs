use gpui::{App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, px};

use crate::layout::h_flex;
use crate::theme;

/// Semantic color for a compact status badge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BadgeTone {
    #[default]
    Neutral,
    Accent,
    Info,
    Warning,
    Danger,
    Success,
}

/// A compact, non-interactive status badge.
#[derive(IntoElement)]
pub struct Badge {
    label: SharedString,
    tone: BadgeTone,
}

impl Badge {
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

impl RenderOnce for Badge {
    fn render(self, _window: &mut gpui::Window, _cx: &mut App) -> impl IntoElement {
        let (background, foreground) = match self.tone {
            BadgeTone::Neutral => (theme::raised(), theme::muted_text()),
            BadgeTone::Accent => (theme::accent(), theme::canvas()),
            BadgeTone::Info => (theme::info(), theme::canvas()),
            BadgeTone::Warning => (theme::warning(), theme::canvas()),
            BadgeTone::Danger => (theme::danger(), theme::canvas()),
            BadgeTone::Success => (theme::accent(), theme::canvas()),
        };
        h_flex()
            .flex_shrink_0()
            .justify_center()
            .px(px(6.))
            .py(px(2.))
            .rounded(px(theme::RADIUS_SM))
            .bg(background)
            .text_xs()
            .text_color(foreground)
            .child(self.label)
    }
}

#[cfg(test)]
mod tests {
    use super::{Badge, BadgeTone};

    #[test]
    fn badge_builder_sets_tone() {
        let badge = Badge::new("up").tone(BadgeTone::Info);
        assert_eq!(badge.tone, BadgeTone::Info);
    }
}

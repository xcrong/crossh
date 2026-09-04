use gpui::{
    App, FontWeight, IntoElement, ParentElement, RenderOnce, SharedString, Styled, div, px,
};

use crossh_ui_base::abbreviation;

use crate::theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AvatarKind {
    #[default]
    Project,
    Command,
}

/// A compact text identity for entities displayed in workspace rails.
#[derive(IntoElement)]
pub struct Avatar {
    label: SharedString,
    kind: AvatarKind,
}

impl Avatar {
    pub fn new(value: &str) -> Self {
        Self {
            label: SharedString::from(abbreviation(value)),
            kind: AvatarKind::default(),
        }
    }

    pub fn kind(mut self, kind: AvatarKind) -> Self {
        self.kind = kind;
        self
    }
}

impl RenderOnce for Avatar {
    fn render(self, _window: &mut gpui::Window, _cx: &mut App) -> impl IntoElement {
        let color = match self.kind {
            AvatarKind::Project => theme::accent(),
            AvatarKind::Command => theme::muted_text(),
        };
        div()
            .w(px(30.))
            .h(px(30.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(theme::RADIUS_SM))
            .border_1()
            .border_color(theme::border_strong())
            .bg(theme::raised())
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(color)
            .child(self.label)
    }
}

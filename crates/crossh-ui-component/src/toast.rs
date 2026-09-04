use gpui::{App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div, px};

use crossh_ui_base::toaster_bottom_offset;

use crate::layout::h_flex;
use crate::status_dot::StatusDot;
use crate::theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToastTone {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

impl ToastTone {
    fn color(self) -> gpui::Rgba {
        match self {
            Self::Info => theme::info(),
            Self::Success => theme::accent(),
            Self::Warning => theme::warning(),
            Self::Error => theme::danger(),
        }
    }
}

/// Small, non-interactive feedback label. State and lifetime stay with the caller.
#[derive(IntoElement)]
pub struct Toast {
    label: SharedString,
    tone: ToastTone,
}

impl Toast {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            tone: ToastTone::default(),
        }
    }

    pub fn tone(mut self, tone: ToastTone) -> Self {
        self.tone = tone;
        self
    }
}

impl RenderOnce for Toast {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let color = self.tone.color();
        h_flex()
            .min_w_0()
            .max_w(px(420.))
            .gap_2()
            .px_3()
            .py_2()
            .rounded(px(theme::RADIUS_SM))
            .border_1()
            .border_color(color)
            .bg(theme::overlay())
            .shadow_md()
            .text_xs()
            .text_color(theme::text())
            .child(
                div()
                    .flex_shrink_0()
                    .child(StatusDot::new(color).size(px(6.))),
            )
            .child(div().min_w_0().flex_1().child(self.label))
    }
}

/// Positions one Toast above the application's bottom status bar.
#[derive(IntoElement)]
pub struct Toaster {
    toast: Toast,
}

impl Toaster {
    pub fn new(toast: Toast) -> Self {
        Self { toast }
    }
}

impl RenderOnce for Toaster {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(px(toaster_bottom_offset(theme::STATUS_BAR_HEIGHT, 8.)))
            .flex()
            .justify_center()
            .px_3()
            .child(self.toast)
    }
}

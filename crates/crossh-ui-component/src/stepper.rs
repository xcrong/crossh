use std::rc::Rc;

use gpui::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, Styled, Window, div, px,
};

use crossh_ui::icons::{IconName, icon};

use crate::button::{Button, ButtonSize, ButtonVariant};
use crate::theme;

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A stateless decrement / value / increment control.
///
/// The minus and plus buttons are icon-only secondary buttons whose ids are
/// derived from the stepper id (`<id>-decrease` / `<id>-increase`), so the
/// owning feature only supplies the value text and the two handlers.
#[derive(IntoElement)]
pub struct Stepper {
    id: ElementId,
    value: Option<SharedString>,
    decrease_tooltip: Option<SharedString>,
    increase_tooltip: Option<SharedString>,
    on_decrease: Option<ClickHandler>,
    on_increase: Option<ClickHandler>,
}

impl Stepper {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            value: None,
            decrease_tooltip: None,
            increase_tooltip: None,
            on_decrease: None,
            on_increase: None,
        }
    }

    pub fn value(mut self, value: impl Into<SharedString>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn tooltips(
        mut self,
        decrease: impl Into<SharedString>,
        increase: impl Into<SharedString>,
    ) -> Self {
        self.decrease_tooltip = Some(decrease.into());
        self.increase_tooltip = Some(increase.into());
        self
    }

    pub fn on_decrease(
        mut self,
        on_decrease: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_decrease = Some(Rc::new(on_decrease));
        self
    }

    pub fn on_increase(
        mut self,
        on_increase: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_increase = Some(Rc::new(on_increase));
        self
    }
}

impl RenderOnce for Stepper {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let decrease_id = ElementId::Name(format!("{}-decrease", self.id).into());
        let increase_id = ElementId::Name(format!("{}-increase", self.id).into());

        let mut decrease = Button::new(decrease_id)
            .size(ButtonSize::Icon(px(30.)))
            .variant(ButtonVariant::Secondary)
            .icon(
                icon(IconName::Minus, 14.)
                    .text_color(theme::muted_text())
                    .hover(|style| style.text_color(theme::text())),
            );
        if let Some(tooltip) = self.decrease_tooltip {
            decrease = decrease.tooltip(tooltip);
        }
        if let Some(handler) = self.on_decrease {
            decrease = decrease.on_click(move |event, window, cx| handler(event, window, cx));
        }

        let mut increase = Button::new(increase_id)
            .size(ButtonSize::Icon(px(30.)))
            .variant(ButtonVariant::Secondary)
            .icon(
                icon(IconName::Plus, 14.)
                    .text_color(theme::muted_text())
                    .hover(|style| style.text_color(theme::text())),
            );
        if let Some(tooltip) = self.increase_tooltip {
            increase = increase.tooltip(tooltip);
        }
        if let Some(handler) = self.on_increase {
            increase = increase.on_click(move |event, window, cx| handler(event, window, cx));
        }

        div()
            .flex()
            .items_center()
            .gap_1()
            .child(decrease)
            .child(
                div()
                    .w(px(64.))
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(theme::RADIUS_SM))
                    .bg(theme::raised())
                    .text_xs()
                    .text_color(theme::text())
                    .child(self.value.unwrap_or_default()),
            )
            .child(increase)
    }
}

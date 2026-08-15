use std::rc::Rc;

use gpui::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::theme;

/// Handler invoked when the switch is clicked, carrying the new on/off state.
type ToggleHandler = Rc<dyn Fn(bool, &ClickEvent, &mut Window, &mut App)>;

/// A stateless on/off switch with an animatable-looking pill track and knob.
///
/// The component only renders the visual state given by [`ToggleSwitch::on`];
/// the owning feature decides the new state and persists it in its handler.
#[derive(IntoElement)]
pub struct ToggleSwitch {
    id: ElementId,
    on: bool,
    on_toggle: Option<ToggleHandler>,
}

impl ToggleSwitch {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            on: false,
            on_toggle: None,
        }
    }

    pub fn on(mut self, on: bool) -> Self {
        self.on = on;
        self
    }

    pub fn on_toggle(
        mut self,
        on_toggle: impl Fn(bool, &ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle = Some(Rc::new(on_toggle));
        self
    }
}

impl RenderOnce for ToggleSwitch {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let new_state = next_state(self.on);
        let mut track = div()
            .id(self.id)
            .w(px(42.))
            .h(px(24.))
            .p_1()
            .flex()
            .items_center()
            .rounded_full()
            .cursor_pointer()
            .bg(if self.on {
                theme::accent()
            } else {
                theme::border_strong()
            });
        track = if self.on {
            track.justify_end()
        } else {
            track.justify_start()
        };
        track = track.child(
            div()
                .w(px(18.))
                .h(px(18.))
                .rounded_full()
                .bg(theme::canvas()),
        );
        if let Some(on_toggle) = self.on_toggle {
            track = track.on_click(move |event, window, cx| {
                on_toggle(new_state, event, window, cx);
            });
        }
        track
    }
}

/// The state a click transitions to: a click on an on-switch turns it off.
fn next_state(on: bool) -> bool {
    !on
}

#[cfg(test)]
mod tests {
    use gpui::ElementId;

    use super::{ToggleSwitch, next_state};

    #[test]
    fn toggle_switch_defaults_to_off_without_handler() {
        let toggle = ToggleSwitch::new("preview-toggle");
        assert_eq!(toggle.id, ElementId::Name("preview-toggle".into()));
        assert!(!toggle.on);
        assert!(toggle.on_toggle.is_none());
    }

    #[test]
    fn toggle_switch_builder_sets_state_and_handler() {
        let toggle = ToggleSwitch::new("preview-toggle")
            .on(true)
            .on_toggle(|_, _, _, _| {});
        assert!(toggle.on);
        assert!(toggle.on_toggle.is_some());
    }

    #[test]
    fn toggle_click_flips_state_both_ways() {
        assert!(!next_state(true));
        assert!(next_state(false));
    }
}

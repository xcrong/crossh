use gpui::{
    AnyElement, App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce, Styled,
    Window, div, px,
};

use crate::theme;

/// Shared bottom status-bar shell. Features own the content and interactions.
#[derive(IntoElement)]
pub struct StatusBar {
    id: ElementId,
    children: Vec<AnyElement>,
}

impl StatusBar {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            children: Vec::new(),
        }
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }
}

impl RenderOnce for StatusBar {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .id(self.id)
            .h(px(theme::STATUS_BAR_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .px_3()
            .border_t_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .text_xs()
            .text_color(theme::muted_text())
            .children(self.children)
    }
}

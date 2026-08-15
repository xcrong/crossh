use gpui::{
    Div, InteractiveElement, ScrollHandle, Stateful, StatefulInteractiveElement, Styled, div,
};

/// A horizontal flex container with vertically centered children.
pub fn h_flex() -> Div {
    div().flex().flex_row().items_center()
}

/// A vertical flex container.
pub fn v_flex() -> Div {
    div().flex().flex_col()
}

/// A vertically scrollable container that tracks its scroll offset with the
/// given handle. Parent-layout modifiers such as `flex_1().min_h_0()` stay at
/// the call site. Chain an element id next: the placeholder id set here is
/// replaced (and the stateful wrapper kept) by the caller's own `.id(...)`.
pub fn scroll_y(handle: &ScrollHandle) -> Stateful<Div> {
    div()
        .id(ELEMENT_PLACEHOLDER_ID)
        .overflow_y_scroll()
        .track_scroll(handle)
}

/// Placeholder element id used internally by [`scroll_y`]; overwritten by the
/// caller's chain.
const ELEMENT_PLACEHOLDER_ID: usize = 0;

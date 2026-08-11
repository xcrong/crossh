use gpui::{Div, Styled, div};

/// A horizontal flex container with vertically centered children.
pub fn h_flex() -> Div {
    div().flex().flex_row().items_center()
}

/// A vertical flex container.
pub fn v_flex() -> Div {
    div().flex().flex_col()
}

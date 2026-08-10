use gpui::{SharedString, Styled, Svg, px, svg};

pub use crossh_assets::IconName;

pub fn icon(name: IconName, size: f32) -> Svg {
    svg().path(SharedString::from(name.path())).size(px(size))
}

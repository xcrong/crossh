use gpui::{Img, SharedString, Styled, Svg, img, px, svg};

pub use crossh_assets::IconName;

pub fn icon(name: IconName, size: f32) -> Svg {
    svg().path(SharedString::from(name.path())).size(px(size))
}

pub fn logo(size: f32) -> Img {
    img(SharedString::from(crossh_assets::LOGO_PATH)).size(px(size))
}

use gpui::{Img, SharedString, Styled, Svg, img, px, svg};

pub use crossh_assets::IconName;

pub fn icon(name: IconName, size: f32) -> Svg {
    // 缺失图标不 panic：开发期用断言暴露，release 记日志并回退占位图标，避免白屏。
    let path = name.path();
    debug_assert!(
        crossh_assets::load(path).is_some(),
        "missing icon asset: {path}"
    );
    if crossh_assets::load(path).is_none() {
        log::warn!("missing icon asset, fallback to placeholder: {path}");
        // 占位回退用最简单的关闭图标；自身缺失时返回无路径的空 Svg，避免递归。
        if name != IconName::X {
            return svg()
                .path(SharedString::from(IconName::X.path()))
                .size(px(size));
        }
        return svg().size(px(size));
    }
    svg().path(SharedString::from(path)).size(px(size))
}

pub fn logo(size: f32) -> Img {
    img(SharedString::from(crossh_assets::LOGO_PATH)).size(px(size))
}

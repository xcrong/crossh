use crossh_theme::Rgb;
use gpui::{Hsla, Rgba, hsla, rgb};

/// Crossh's visual language: a graphite command deck with a mint signal.
///
/// Color values live in `crossh-theme`; this module adapts them to GPUI and
/// keeps GPUI-specific selection and focus colors next to the renderer.
pub const SIDEBAR_WIDTH: f32 = 252.0;
pub const SIDEBAR_MIN_WIDTH: f32 = 216.0;
pub const SIDEBAR_MAX_WIDTH: f32 = 360.0;
pub const SIDEBAR_RAIL_WIDTH: f32 = 44.0;
pub const TITLEBAR_HEIGHT: f32 = 42.0;
pub const TAB_HEIGHT: f32 = 38.0;
pub const ROW_HEIGHT: f32 = 36.0;
pub const RADIUS_SM: f32 = 4.0;
pub const RADIUS_MD: f32 = 8.0;
pub const QUICK_COMMANDS_WIDTH: f32 = 240.0;
pub const QUICK_COMMANDS_MIN_WIDTH: f32 = 240.0;
pub const QUICK_COMMANDS_MAX_WIDTH: f32 = 460.0;
pub const QUICK_COMMANDS_RAIL_WIDTH: f32 = 40.0;

fn color(value: Rgb) -> Rgba {
    rgb(value.hex())
}

pub fn canvas() -> Rgba {
    color(crossh_theme::canvas())
}

pub fn sidebar() -> Rgba {
    color(crossh_theme::sidebar())
}

pub fn surface() -> Rgba {
    color(crossh_theme::surface())
}

pub fn raised() -> Rgba {
    color(crossh_theme::raised())
}

pub fn border() -> Rgba {
    color(crossh_theme::border())
}

pub fn border_strong() -> Rgba {
    color(crossh_theme::border_strong())
}

pub fn overlay() -> Rgba {
    color(crossh_theme::overlay())
}

pub fn text() -> Rgba {
    color(crossh_theme::text())
}

pub fn muted_text() -> Rgba {
    color(crossh_theme::muted_text())
}

pub fn faint_text() -> Rgba {
    color(crossh_theme::faint_text())
}

pub fn accent() -> Rgba {
    color(crossh_theme::accent())
}

pub fn accent_hover() -> Rgba {
    color(crossh_theme::accent_hover())
}

/// Background tint of an active text selection. The translucent accent keeps
/// the underlying glyphs readable while staying clearly visible on dark
/// terminal backgrounds.
pub fn selection() -> Hsla {
    Hsla::from(accent()).opacity(0.45)
}

pub fn accent_soft() -> Rgba {
    color(crossh_theme::accent_soft())
}

pub fn info() -> Rgba {
    color(crossh_theme::info())
}

pub fn warning() -> Rgba {
    color(crossh_theme::warning())
}

pub fn danger() -> Rgba {
    color(crossh_theme::danger())
}

pub fn danger_hover() -> Rgba {
    color(crossh_theme::danger_hover())
}

pub fn diff_add_bg() -> Rgba {
    color(crossh_theme::diff_add_bg())
}

pub fn diff_add_fg() -> Rgba {
    color(crossh_theme::diff_add_fg())
}

pub fn diff_del_bg() -> Rgba {
    color(crossh_theme::diff_del_bg())
}

pub fn diff_del_fg() -> Rgba {
    color(crossh_theme::diff_del_fg())
}

pub fn scrim() -> Hsla {
    hsla(0.0, 0.0, 0.0, 0.62)
}

pub fn focus_ring() -> Hsla {
    Hsla::from(accent()).opacity(0.9)
}

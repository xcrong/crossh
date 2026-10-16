use gpui::{App, Hsla, Rgba, hsla, rgb};
use terminal::theme::CrosshTheme;

use crate::palette::Rgb;

/// Crossh's visual language: a graphite command deck with a mint signal.
///
/// Color values live in `crate::palette`; this module adapts them to GPUI and
/// keeps GPUI-specific selection and focus colors next to the renderer.
pub const SIDEBAR_WIDTH: f32 = 252.0;
pub const SIDEBAR_MIN_WIDTH: f32 = 216.0;
pub const SIDEBAR_MAX_WIDTH: f32 = 360.0;
pub const SIDEBAR_RAIL_WIDTH: f32 = 44.0;
pub const TITLEBAR_HEIGHT: f32 = 42.0;
pub const TAB_HEIGHT: f32 = 38.0;
pub const STATUS_BAR_HEIGHT: f32 = 27.0;
// tty7 侧栏单行 28px（搜索/切换同高）；36px 太空，30px 仍保留 24px 行内命中区。
pub const ROW_HEIGHT: f32 = 30.0;
pub const RADIUS_SM: f32 = 4.0;
pub const RADIUS_MD: f32 = 8.0;

fn color(value: Rgb) -> Rgba {
    rgb(value.hex())
}

pub fn canvas() -> Rgba {
    color(crate::palette::canvas())
}

pub fn sidebar() -> Rgba {
    color(crate::palette::sidebar())
}

pub fn surface() -> Rgba {
    color(crate::palette::surface())
}

pub fn raised() -> Rgba {
    color(crate::palette::raised())
}

pub fn border() -> Rgba {
    color(crate::palette::border())
}

pub fn border_strong() -> Rgba {
    color(crate::palette::border_strong())
}

pub fn overlay() -> Rgba {
    color(crate::palette::overlay())
}

pub fn text() -> Rgba {
    color(crate::palette::text())
}

pub fn muted_text() -> Rgba {
    color(crate::palette::muted_text())
}

pub fn faint_text() -> Rgba {
    color(crate::palette::faint_text())
}

pub fn accent() -> Rgba {
    color(crate::palette::accent())
}

pub fn accent_hover() -> Rgba {
    color(crate::palette::accent_hover())
}

/// Background tint of an active text selection. The translucent accent keeps
/// the underlying glyphs readable while staying clearly visible on dark
/// terminal backgrounds.
pub fn selection() -> Hsla {
    Hsla::from(accent()).opacity(0.45)
}

pub fn accent_soft() -> Rgba {
    color(crate::palette::accent_soft())
}

pub fn info() -> Rgba {
    color(crate::palette::info())
}

pub fn warning() -> Rgba {
    color(crate::palette::warning())
}

pub fn danger() -> Rgba {
    color(crate::palette::danger())
}

pub fn danger_hover() -> Rgba {
    color(crate::palette::danger_hover())
}

pub fn diff_add_bg() -> Rgba {
    color(crate::palette::diff_add_bg())
}

pub fn diff_add_fg() -> Rgba {
    color(crate::palette::diff_add_fg())
}

pub fn diff_del_bg() -> Rgba {
    color(crate::palette::diff_del_bg())
}

pub fn diff_del_fg() -> Rgba {
    color(crate::palette::diff_del_fg())
}

pub fn scrim() -> Hsla {
    hsla(0.0, 0.0, 0.0, 0.62)
}

pub fn focus_ring() -> Hsla {
    Hsla::from(accent()).opacity(0.9)
}

/// Make the terminal surfaces match the Crossh workbench surrounding them.
/// ANSI families and cursor stay on the pinned One Dark values in
/// `terminal::theme::CrosshTheme::dark`; link hover follows the accent like
/// the previous `install_crossh_theme` override did.
pub fn install_crossh_theme(cx: &mut App) {
    CrosshTheme::set(
        cx,
        CrosshTheme {
            terminal_background: to_hsla(canvas()),
            terminal_ansi_background: to_hsla(canvas()),
            terminal_foreground: to_hsla(text()),
            terminal_bright_foreground: to_hsla(text()),
            terminal_dim_foreground: to_hsla(muted_text()),
            link_text_hover: to_hsla(accent()),
            ..CrosshTheme::dark()
        },
    );
}

fn to_hsla(value: Rgba) -> Hsla {
    Hsla::from(value)
}

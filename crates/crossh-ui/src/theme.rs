use gpui::{Hsla, Rgba, hsla, rgb};

/// Crossh's visual language: a graphite command deck with a mint signal.
///
/// The palette intentionally has four distinct depth levels. The terminal is
/// the canvas; chrome, controls, and popovers should read as separate layers
/// without competing with the shell itself.
pub const SIDEBAR_WIDTH: f32 = 252.0;
pub const SIDEBAR_MIN_WIDTH: f32 = 216.0;
pub const SIDEBAR_MAX_WIDTH: f32 = 360.0;
pub const TITLEBAR_HEIGHT: f32 = 42.0;
pub const TAB_HEIGHT: f32 = 38.0;
pub const ROW_HEIGHT: f32 = 36.0;
pub const RADIUS_SM: f32 = 4.0;
pub const RADIUS_MD: f32 = 8.0;
pub const QUICK_COMMANDS_WIDTH: f32 = 300.0;
pub const QUICK_COMMANDS_MIN_WIDTH: f32 = 240.0;
pub const QUICK_COMMANDS_MAX_WIDTH: f32 = 460.0;

pub fn canvas() -> Rgba {
    rgb(0x0d1014)
}

pub fn sidebar() -> Rgba {
    rgb(0x12171c)
}

pub fn surface() -> Rgba {
    rgb(0x171d23)
}

pub fn raised() -> Rgba {
    rgb(0x202930)
}

pub fn border() -> Rgba {
    rgb(0x28323a)
}

pub fn border_strong() -> Rgba {
    rgb(0x3a4854)
}

pub fn overlay() -> Rgba {
    rgb(0x262f38)
}

pub fn text() -> Rgba {
    rgb(0xe7edf1)
}

pub fn muted_text() -> Rgba {
    rgb(0x9aa6b0)
}

pub fn faint_text() -> Rgba {
    rgb(0x65717c)
}

pub fn accent() -> Rgba {
    rgb(0x7de0bd)
}

pub fn accent_hover() -> Rgba {
    rgb(0x95efd0)
}

/// Background tint of an active text selection. The translucent accent keeps
/// the underlying glyphs readable while staying clearly visible on dark
/// terminal backgrounds.
pub fn selection() -> Hsla {
    Hsla::from(accent()).opacity(0.45)
}

pub fn accent_soft() -> Rgba {
    rgb(0x173a34)
}

pub fn info() -> Rgba {
    rgb(0x87bfff)
}

pub fn warning() -> Rgba {
    rgb(0xf3c66e)
}

pub fn danger() -> Rgba {
    rgb(0xf28b8b)
}

pub fn danger_hover() -> Rgba {
    rgb(0xffa4a4)
}

pub fn diff_add_bg() -> Rgba {
    rgb(0x1c3327)
}

pub fn diff_add_fg() -> Rgba {
    rgb(0x8fe3b0)
}

pub fn diff_del_bg() -> Rgba {
    rgb(0x3a2222)
}

pub fn diff_del_fg() -> Rgba {
    rgb(0xf2a2a2)
}

pub fn scrim() -> Hsla {
    hsla(0.0, 0.0, 0.0, 0.62)
}

pub fn focus_ring() -> Hsla {
    Hsla::from(accent()).opacity(0.9)
}

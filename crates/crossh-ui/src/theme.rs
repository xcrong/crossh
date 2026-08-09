use gpui::{Hsla, Rgba, hsla, rgb};

/// Crossh's visual language: a quiet graphite workbench with a mint signal
/// reserved for the active surface and healthy connections.
pub const SIDEBAR_WIDTH: f32 = 248.0;
pub const SIDEBAR_MIN_WIDTH: f32 = 216.0;
pub const SIDEBAR_MAX_WIDTH: f32 = 360.0;
pub const TITLEBAR_HEIGHT: f32 = 38.0;
pub const TAB_HEIGHT: f32 = 36.0;
pub const ROW_HEIGHT: f32 = 34.0;
pub const RADIUS_SM: f32 = 5.0;
pub const RADIUS_MD: f32 = 7.0;
pub const QUICK_COMMANDS_WIDTH: f32 = 300.0;
pub const QUICK_COMMANDS_MIN_WIDTH: f32 = 240.0;
pub const QUICK_COMMANDS_MAX_WIDTH: f32 = 460.0;

pub fn canvas() -> Rgba {
    rgb(0x0f1114)
}

pub fn sidebar() -> Rgba {
    rgb(0x15191d)
}

pub fn surface() -> Rgba {
    rgb(0x1d2329)
}

pub fn raised() -> Rgba {
    rgb(0x232b32)
}

pub fn border() -> Rgba {
    rgb(0x2a323a)
}

pub fn border_strong() -> Rgba {
    rgb(0x3a4650)
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
    rgb(0x69d7b0)
}

/// Background tint of an active text selection. The translucent accent keeps
/// the underlying glyphs readable while staying clearly visible on dark
/// terminal backgrounds.
pub fn selection() -> Hsla {
    Hsla::from(accent()).opacity(0.45)
}

pub fn accent_soft() -> Rgba {
    rgb(0x1d3a33)
}

pub fn info() -> Rgba {
    rgb(0x78b7ff)
}

pub fn warning() -> Rgba {
    rgb(0xf1c878)
}

pub fn danger() -> Rgba {
    rgb(0xf07d7d)
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
    hsla(0.43, 0.58, 0.62, 0.9)
}

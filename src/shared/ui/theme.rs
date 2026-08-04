use gpui::{Hsla, Rgba, hsla, rgb};

/// Crossh's visual language: a quiet graphite workbench with a mint signal
/// reserved for the active surface and healthy connections.
pub(crate) const SIDEBAR_WIDTH: f32 = 248.0;
pub(crate) const SIDEBAR_MIN_WIDTH: f32 = 216.0;
pub(crate) const SIDEBAR_MAX_WIDTH: f32 = 360.0;
pub(crate) const TITLEBAR_HEIGHT: f32 = 38.0;
pub(crate) const TAB_HEIGHT: f32 = 36.0;
pub(crate) const ROW_HEIGHT: f32 = 34.0;
pub(crate) const RADIUS_SM: f32 = 5.0;
pub(crate) const RADIUS_MD: f32 = 7.0;

pub(crate) fn canvas() -> Rgba {
    rgb(0x0f1114)
}

pub(crate) fn sidebar() -> Rgba {
    rgb(0x15191d)
}

pub(crate) fn surface() -> Rgba {
    rgb(0x1d2329)
}

pub(crate) fn raised() -> Rgba {
    rgb(0x232b32)
}

pub(crate) fn border() -> Rgba {
    rgb(0x2a323a)
}

pub(crate) fn border_strong() -> Rgba {
    rgb(0x3a4650)
}

pub(crate) fn text() -> Rgba {
    rgb(0xe7edf1)
}

pub(crate) fn muted_text() -> Rgba {
    rgb(0x9aa6b0)
}

pub(crate) fn faint_text() -> Rgba {
    rgb(0x65717c)
}

pub(crate) fn accent() -> Rgba {
    rgb(0x69d7b0)
}

pub(crate) fn accent_soft() -> Rgba {
    rgb(0x1d3a33)
}

pub(crate) fn info() -> Rgba {
    rgb(0x78b7ff)
}

pub(crate) fn warning() -> Rgba {
    rgb(0xf1c878)
}

pub(crate) fn danger() -> Rgba {
    rgb(0xf07d7d)
}

pub(crate) fn scrim() -> Hsla {
    hsla(0.0, 0.0, 0.0, 0.62)
}

pub(crate) fn focus_ring() -> Hsla {
    hsla(0.43, 0.58, 0.62, 0.9)
}

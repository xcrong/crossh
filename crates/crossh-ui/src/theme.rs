use std::sync::Arc;

use gpui::{App, Hsla, Rgba, hsla, rgb};
use theme::GlobalTheme;

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
pub const ROW_HEIGHT: f32 = 36.0;
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

/// Keep Zed's syntax and ANSI defaults, but make its terminal surfaces match
/// the Crossh workbench surrounding them.
pub fn install_crossh_theme(cx: &mut App) {
    let mut theme = GlobalTheme::theme(cx).as_ref().clone();
    let canvas = to_hsla(canvas());
    let sidebar = to_hsla(sidebar());
    let surface = to_hsla(surface());
    let raised = to_hsla(raised());
    let overlay = to_hsla(overlay());
    let border = to_hsla(border());
    let border_strong = to_hsla(border_strong());
    let text = to_hsla(text());
    let muted_text = to_hsla(muted_text());
    let faint_text = to_hsla(faint_text());
    let accent = to_hsla(accent());
    let accent_soft = to_hsla(accent_soft());
    let info = to_hsla(info());
    let warning = to_hsla(warning());
    let danger = to_hsla(danger());

    let colors = &mut theme.styles.colors;
    colors.border = border;
    colors.border_variant = border;
    colors.border_focused = accent;
    colors.border_selected = accent;
    colors.border_disabled = border;
    colors.elevated_surface_background = overlay;
    colors.surface_background = surface;
    colors.background = canvas;
    colors.element_background = raised;
    colors.element_hover = overlay;
    colors.element_active = accent_soft;
    colors.element_selected = accent_soft;
    colors.element_selection_background = selection();
    colors.drop_target_border = border_strong;
    colors.text = text;
    colors.text_muted = muted_text;
    colors.text_placeholder = faint_text;
    colors.text_disabled = faint_text;
    colors.text_accent = accent;
    colors.icon = text;
    colors.icon_muted = muted_text;
    colors.icon_disabled = faint_text;
    colors.icon_placeholder = faint_text;
    colors.icon_accent = accent;
    colors.status_bar_background = surface;
    colors.title_bar_background = surface;
    colors.title_bar_inactive_background = sidebar;
    colors.toolbar_background = sidebar;
    colors.tab_bar_background = surface;
    colors.tab_inactive_background = surface;
    colors.tab_active_background = accent_soft;
    colors.panel_background = surface;
    colors.panel_focused_border = accent;
    colors.panel_overlay_background = overlay;
    colors.panel_overlay_hover = raised;
    colors.pane_focused_border = accent;
    colors.pane_group_border = border;
    colors.editor_background = canvas;
    colors.editor_gutter_background = canvas;
    colors.editor_subheader_background = surface;
    colors.editor_active_line_background = accent_soft;
    colors.terminal_background = canvas;
    colors.terminal_ansi_background = canvas;
    colors.terminal_foreground = text;
    colors.terminal_bright_foreground = text;
    colors.terminal_dim_foreground = muted_text;
    colors.link_text_hover = accent;
    colors.version_control_added = accent;
    colors.version_control_deleted = danger;
    colors.version_control_modified = warning;
    colors.version_control_renamed = info;
    colors.version_control_conflict = danger;
    colors.version_control_ignored = faint_text;
    colors.version_control_word_added = accent;
    colors.version_control_word_deleted = danger;

    theme.id = "crossh".to_string();
    theme.name = "Crossh".into();
    GlobalTheme::update_theme(cx, Arc::new(theme));
}

fn to_hsla(value: Rgba) -> Hsla {
    Hsla::from(value)
}

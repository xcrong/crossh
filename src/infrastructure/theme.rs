//! Application-wide theme bridge between Crossh chrome and Zed's renderer.

use std::sync::Arc;

use gpui::{App, Hsla};
use theme::GlobalTheme;

use crossh_ui::theme as crossh_theme;

/// Keep Zed's syntax and ANSI defaults, but make its terminal surfaces match
/// the Crossh workbench surrounding them.
pub(crate) fn install_crossh_theme(cx: &mut App) {
    let mut theme = GlobalTheme::theme(cx).as_ref().clone();
    let canvas = color(crossh_theme::canvas());
    let sidebar = color(crossh_theme::sidebar());
    let surface = color(crossh_theme::surface());
    let raised = color(crossh_theme::raised());
    let overlay = color(crossh_theme::overlay());
    let border = color(crossh_theme::border());
    let border_strong = color(crossh_theme::border_strong());
    let text = color(crossh_theme::text());
    let muted_text = color(crossh_theme::muted_text());
    let faint_text = color(crossh_theme::faint_text());
    let accent = color(crossh_theme::accent());
    let accent_soft = color(crossh_theme::accent_soft());
    let info = color(crossh_theme::info());
    let warning = color(crossh_theme::warning());
    let danger = color(crossh_theme::danger());

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
    colors.element_selection_background = crossh_theme::selection();
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

fn color(color: gpui::Rgba) -> Hsla {
    Hsla::from(color)
}

//! Crossh's terminal palette, replacing Zed's `theme` crate for rendering.
//!
//! The workbench colors (background/foreground) are owned by
//! `crossh_ui::theme`; the 16 ANSI families plus cursor/link colors are pinned
//! to Zed One Dark literals so terminal output renders as before.
//!
//! ponytail: the ANSI pin is a deliberate simplification — upstream resolves
//! these from scale functions at the pinned rev, not from the One Dark JSON,
//! so per-channel drift is possible. If pixel parity ever matters, sample the
//! rendered values and paste them here.

use gpui::{App, Global, Hsla, hsla};

/// Terminal palette shared by the emulator (`get_color_at_index`) and the
/// terminal element (`convert_color`, cursor, link hover).
#[derive(Clone, Debug)]
pub struct CrosshTheme {
    pub terminal_background: Hsla,
    pub terminal_ansi_background: Hsla,
    pub terminal_foreground: Hsla,
    pub terminal_bright_foreground: Hsla,
    pub terminal_dim_foreground: Hsla,
    pub terminal_ansi_black: Hsla,
    pub terminal_ansi_red: Hsla,
    pub terminal_ansi_green: Hsla,
    pub terminal_ansi_yellow: Hsla,
    pub terminal_ansi_blue: Hsla,
    pub terminal_ansi_magenta: Hsla,
    pub terminal_ansi_cyan: Hsla,
    pub terminal_ansi_white: Hsla,
    pub terminal_ansi_bright_black: Hsla,
    pub terminal_ansi_bright_red: Hsla,
    pub terminal_ansi_bright_green: Hsla,
    pub terminal_ansi_bright_yellow: Hsla,
    pub terminal_ansi_bright_blue: Hsla,
    pub terminal_ansi_bright_magenta: Hsla,
    pub terminal_ansi_bright_cyan: Hsla,
    pub terminal_ansi_bright_white: Hsla,
    pub terminal_ansi_dim_black: Hsla,
    pub terminal_ansi_dim_red: Hsla,
    pub terminal_ansi_dim_green: Hsla,
    pub terminal_ansi_dim_yellow: Hsla,
    pub terminal_ansi_dim_blue: Hsla,
    pub terminal_ansi_dim_magenta: Hsla,
    pub terminal_ansi_dim_cyan: Hsla,
    pub terminal_ansi_dim_white: Hsla,
    pub link_text_hover: Hsla,
    pub cursor: Hsla,
}

impl Global for CrosshTheme {}

impl CrosshTheme {
    pub fn dark() -> Self {
        Self {
            terminal_background: hsla(0.611, 0.130, 0.180, 1.0),
            terminal_ansi_background: hsla(0.611, 0.130, 0.180, 1.0),
            terminal_foreground: hsla(0.608, 0.135, 0.710, 1.0),
            terminal_bright_foreground: hsla(0.593, 0.148, 0.880, 1.0),
            terminal_dim_foreground: hsla(0.615, 0.139, 0.451, 1.0),
            terminal_ansi_black: hsla(0.611, 0.130, 0.180, 1.0),
            terminal_ansi_red: hsla(0.987, 0.652, 0.651, 1.0),
            terminal_ansi_green: hsla(0.264, 0.381, 0.620, 1.0),
            terminal_ansi_yellow: hsla(0.108, 0.671, 0.690, 1.0),
            terminal_ansi_blue: hsla(0.575, 0.816, 0.659, 1.0),
            terminal_ansi_magenta: hsla(0.795, 0.598, 0.669, 1.0),
            terminal_ansi_cyan: hsla(0.519, 0.470, 0.549, 1.0),
            terminal_ansi_white: hsla(0.608, 0.135, 0.710, 1.0),
            terminal_ansi_bright_black: hsla(0.615, 0.139, 0.451, 1.0),
            terminal_ansi_bright_red: hsla(0.990, 0.706, 0.720, 1.0),
            terminal_ansi_bright_green: hsla(0.252, 0.500, 0.671, 1.0),
            terminal_ansi_bright_yellow: hsla(0.113, 1.000, 0.761, 1.0),
            terminal_ansi_bright_blue: hsla(0.585, 1.000, 0.761, 1.0),
            terminal_ansi_bright_magenta: hsla(0.785, 0.675, 0.759, 1.0),
            terminal_ansi_bright_cyan: hsla(0.513, 0.629, 0.651, 1.0),
            terminal_ansi_bright_white: hsla(0.000, 0.000, 0.980, 1.0),
            terminal_ansi_dim_black: hsla(0.622, 0.113, 0.261, 1.0),
            terminal_ansi_dim_red: hsla(0.988, 0.331, 0.492, 1.0),
            terminal_ansi_dim_green: hsla(0.272, 0.233, 0.455, 1.0),
            terminal_ansi_dim_yellow: hsla(0.109, 0.396, 0.539, 1.0),
            terminal_ansi_dim_blue: hsla(0.579, 0.430, 0.475, 1.0),
            terminal_ansi_dim_magenta: hsla(0.792, 0.311, 0.478, 1.0),
            terminal_ansi_dim_cyan: hsla(0.519, 0.394, 0.388, 1.0),
            terminal_ansi_dim_white: hsla(0.569, 0.057, 0.584, 1.0),
            link_text_hover: hsla(0.585, 0.716, 0.682, 1.0),
            cursor: hsla(0.585, 0.716, 0.682, 1.0),
        }
    }

    pub fn get(cx: &App) -> &Self {
        cx.try_global::<Self>().unwrap_or_else(|| {
            static FALLBACK: std::sync::LazyLock<CrosshTheme> =
                std::sync::LazyLock::new(CrosshTheme::dark);
            &*FALLBACK
        })
    }

    pub fn set(cx: &mut App, theme: Self) {
        cx.set_global(theme);
    }
}

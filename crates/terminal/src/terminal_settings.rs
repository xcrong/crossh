//! Crossh fork of Zed terminal_settings — decoupled from Zed settings.
//! Bridges to `crossh_terminal::TerminalSettings` for user-facing knobs,
//! keeps terminal-internal knobs locally with `Global` fallback.

use collections::HashMap;
use gpui::{App, FontFallbacks, FontFeatures, FontWeight, Global, Pixels, px};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use task::Shell;

// Re-exported so `terminal::terminal_settings::{CursorShape, AlternateScroll}` stays import-compatible.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AlternateScroll {
    On,
    Off,
    #[default]
    Auto,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Bar,
    Hollow,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalBlink {
    #[default]
    Off,
    Terminal,
    List,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalBell {
    #[default]
    Off,
    On,
    Audible,
    Visual,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalDockPosition {
    #[default]
    Bottom,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalLineHeight {
    #[default]
    Comfortable,
    Standard,
    Custom(f32),
}
impl TerminalLineHeight {
    pub fn value(&self) -> f32 {
        match self {
            TerminalLineHeight::Comfortable => 1.618,
            TerminalLineHeight::Standard => 1.3,
            TerminalLineHeight::Custom(v) => v.max(1.0),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VenvSettings {
    #[default]
    Off,
    On,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkingDirectory {
    #[default]
    CurrentProjectDirectory,
    AlwaysHome,
    Always { directory: String },
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ShowScrollbar {
    #[default]
    Auto,
    System,
    Always,
    Never,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Toolbar {
    pub breadcrumbs: bool,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ScrollbarSettings {
    pub show: Option<ShowScrollbar>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct FontFamilyName(pub String);

impl From<String> for FontFamilyName {
    fn from(s: String) -> Self {
        Self(s)
    }
}

// The full settings shape kept for source compatibility. Only a subset is
// actually read via `get_global` in `terminal.rs` (`keep_selection_on_copy`,
// `open_links_in_mouse_mode`); the rest has sensible defaults so callers that
// construct a default still get a usable terminal. Crossh's user knobs come
// from `crossh_terminal::TerminalSettings` and are applied by `TerminalView`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TerminalSettings {
    pub shell: Shell,
    pub working_directory: WorkingDirectory,
    pub font_size: Option<Pixels>,
    pub font_family: Option<FontFamilyName>,
    pub font_fallbacks: Option<FontFallbacks>,
    pub font_features: Option<FontFeatures>,
    pub font_weight: Option<FontWeight>,
    pub line_height: TerminalLineHeight,
    pub env: HashMap<String, String>,
    pub cursor_shape: CursorShape,
    pub blinking: TerminalBlink,
    pub alternate_scroll: AlternateScroll,
    pub option_as_meta: bool,
    pub copy_on_select: bool,
    pub keep_selection_on_copy: bool,
    pub open_links_in_mouse_mode: bool,
    pub button: bool,
    pub dock: TerminalDockPosition,
    pub starts_open: bool,
    pub flexible: bool,
    pub default_width: Pixels,
    pub default_height: Pixels,
    pub detect_venv: VenvSettings,
    pub max_scroll_history_lines: Option<usize>,
    pub scroll_multiplier: f32,
    pub toolbar: Toolbar,
    pub scrollbar: ScrollbarSettings,
    pub minimum_contrast: f32,
    pub path_hyperlink_regexes: Vec<String>,
    pub path_hyperlink_timeout_ms: u64,
    pub show_count_badge: bool,
    pub bell: TerminalBell,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            shell: Shell::System,
            working_directory: WorkingDirectory::CurrentProjectDirectory,
            font_size: None,
            font_family: None,
            font_fallbacks: None,
            font_features: None,
            font_weight: None,
            line_height: TerminalLineHeight::Comfortable,
            env: HashMap::default(),
            cursor_shape: CursorShape::Block,
            blinking: TerminalBlink::Off,
            alternate_scroll: AlternateScroll::On,
            option_as_meta: false,
            copy_on_select: false,
            keep_selection_on_copy: false,
            open_links_in_mouse_mode: true,
            button: false,
            dock: TerminalDockPosition::Bottom,
            starts_open: true,
            flexible: true,
            default_width: px(640.),
            default_height: px(320.),
            detect_venv: VenvSettings::Off,
            max_scroll_history_lines: None,
            scroll_multiplier: 1.0,
            toolbar: Toolbar { breadcrumbs: false },
            scrollbar: ScrollbarSettings { show: None },
            minimum_contrast: 45.0,
            path_hyperlink_regexes: Vec::new(),
            path_hyperlink_timeout_ms: 100,
            show_count_badge: false,
            bell: TerminalBell::Off,
        }
    }
}

impl Global for TerminalSettings {}

impl TerminalSettings {
    pub fn get_global(cx: &App) -> &Self {
        cx.try_global::<Self>().unwrap_or_else(|| {
            static FALLBACK: LazyLock<TerminalSettings> = LazyLock::new(TerminalSettings::default);
            &*FALLBACK
        })
    }

    /// Bridge from Crossh's own terminal settings (font / scrollback) — kept
    /// separate from Zed's global so `TerminalView::from_local_zed` can stay
    /// self-contained.
    pub fn from_crossh(crossh: &crossh_terminal::TerminalSettings) -> Self {
        let mut base = Self::default();
        base.font_size = Some(px(crossh.font_size));
        base.max_scroll_history_lines = Some(crossh.scrollback);
        base
    }
}

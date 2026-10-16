//! Crossh fork of Zed terminal_settings — decoupled from Zed settings.
//! Bridges to `crossh_terminal::TerminalSettings` for user-facing knobs,
//! keeps terminal-internal knobs locally with `Global` fallback.

use std::collections::HashMap;
use std::sync::LazyLock;

use gpui::{App, FontFallbacks, FontFeatures, FontWeight, Global, Pixels, px};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::shell::Shell;

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
    Always {
        directory: String,
    },
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
            path_hyperlink_regexes: default_path_hyperlink_regexes(),
            path_hyperlink_timeout_ms: 100,
            show_count_badge: false,
            bell: TerminalBell::Off,
        }
    }
}

/// Default `terminal.path_hyperlink_regexes`, vendored from Zed's
/// `assets/settings/default.json` at the pinned revision. Without these,
/// bare paths (`src/main.rs:12`, `File "...", line 4`) don't linkify —
/// the fork previously left this empty because it has no settings store.
fn default_path_hyperlink_regexes() -> Vec<String> {
    vec![
        // Python-style diagnostics
        r#"File "(?<path>[^"]+)", line (?<line>[0-9]+)"#.to_string(),
        // Common path syntax with optional line, column, description,
        // trailing punctuation, or surrounding symbols or quotes.
        // (Zed stores this as an array of lines joined with newlines;
        // pre-joined here.)
        r##"(?x)
(?<path>
    (
        # multi-char path: first char (not opening delimiter, space, or box drawing char)
        [^({\[<"\u0060'\ \u2500-\u257F]
        # middle chars: non-space, and colon/paren only if not followed by digit/paren/space
        ([^\ :(]|[:(][^0-9()\ ])*
        # last char: not closing delimiter or colon
        [^()}\]>"'`.,;:\ ]
    |
        # single-char path: not delimiter, punctuation, space, or box drawing char
        [^(){}\[\]<>"'`.,;:\ \u2500-\u257F]
    )
    # optional line/column suffix (included in path for PathWithPosition::parse_str)
    (:+[0-9]+(:[0-9]+)?|:?\([0-9]+([,:]?[0-9]+)?\))?
)"##
        .to_string(),
    ]
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
        Self {
            font_size: Some(px(crossh.font_size)),
            max_scroll_history_lines: Some(crossh.scrollback),
            ..Self::default()
        }
    }
}

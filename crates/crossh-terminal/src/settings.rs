//! Terminal-owned settings and their validation rules.

use serde::{Deserialize, Serialize};

pub const DEFAULT_FONT_SIZE: f32 = 14.0;
pub const DEFAULT_SCROLLBACK: usize = 10_000;
pub const MIN_FONT_SIZE: f32 = 10.0;
pub const MAX_FONT_SIZE: f32 = 24.0;
pub const MIN_SCROLLBACK: usize = 100;
pub const MAX_SCROLLBACK: usize = 100_000;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TerminalSettings {
    #[serde(default = "default_show_timestamps")]
    pub show_timestamps: bool,
    #[serde(
        default = "default_notifications_enabled",
        rename = "terminal_notifications"
    )]
    pub notifications_enabled: bool,
    #[serde(default = "default_font_size", rename = "terminal_font_size")]
    pub font_size: f32,
    #[serde(default = "default_scrollback", rename = "terminal_scrollback")]
    pub scrollback: usize,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            show_timestamps: default_show_timestamps(),
            notifications_enabled: default_notifications_enabled(),
            font_size: default_font_size(),
            scrollback: default_scrollback(),
        }
    }
}

impl TerminalSettings {
    pub fn normalized(mut self) -> Self {
        self.font_size = if self.font_size.is_finite() {
            self.font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
        } else {
            DEFAULT_FONT_SIZE
        };
        self.scrollback = self.scrollback.clamp(MIN_SCROLLBACK, MAX_SCROLLBACK);
        self
    }
}

fn default_show_timestamps() -> bool {
    true
}

fn default_notifications_enabled() -> bool {
    true
}

fn default_font_size() -> f32 {
    DEFAULT_FONT_SIZE
}

fn default_scrollback() -> usize {
    DEFAULT_SCROLLBACK
}

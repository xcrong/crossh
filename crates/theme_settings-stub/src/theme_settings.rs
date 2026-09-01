//! crossh stub for theme_settings — minimal for terminal_element
use gpui::{App, Global, Pixels, px};

pub use settings::FontFamilyName;

#[derive(Clone, Debug, Default)]
pub struct ThemeSettings {
    pub buffer_font: BufferFont,
}

#[derive(Clone, Debug, Default)]
pub struct BufferFont {
    pub family: String,
    pub fallbacks: Option<Vec<String>>,
}

impl Global for ThemeSettings {}

impl ThemeSettings {
    pub fn get_global(cx: &App) -> &Self {
        cx.global::<Self>()
    }
    pub fn buffer_font_size(&self, _cx: &App) -> Pixels {
        px(14.0)
    }
}

pub fn adjusted_font_size(size: Pixels, _cx: &App) -> Pixels {
    size
}

pub fn init(_themes: LoadThemes, _cx: &mut App) {}

#[derive(Clone, Copy, Debug)]
pub enum LoadThemes {
    JustBase,
    All,
}

pub fn observe_buffer_font_size_adjustment(_cx: &mut App) {}

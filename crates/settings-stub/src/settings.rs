//! crossh 定向复制+删除：Zed settings 的极轻桩 — 仅保留 terminal 所需形状
use collections::HashMap;
use gpui::{App, FontFeatures, FontWeight, Global, Pixels, SharedString, px};
use indexmap::IndexMap;
use parking_lot::Mutex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::any::TypeId;
use std::sync::{Arc, OnceLock};
use std::path::PathBuf;

pub use settings_macros::RegisterSetting;

// ---- Settings trait (minimal) ----
pub trait Settings: 'static + Send + Sync + Sized {
    fn from_settings(content: &SettingsContent) -> Self;
    fn get_global(_cx: &App) -> &'static Self
    where
        Self: Sized,
    {
        static MAP: OnceLock<Mutex<HashMap<TypeId, usize>>> = OnceLock::new();
        let map = MAP.get_or_init(|| Mutex::new(HashMap::default()));
        let mut map = map.lock();
        let id = TypeId::of::<Self>();
        if let Some(&addr) = map.get(&id) {
            unsafe { &*(addr as *const Self) }
        } else {
            let boxed =
                Box::new(unsafe { std::mem::MaybeUninit::<Self>::zeroed().assume_init() });
            let addr = Box::into_raw(boxed) as usize;
            map.insert(id, addr);
            unsafe { &*(addr as *const Self) }
        }
    }
    fn override_global(_value: Self, _cx: &mut App) where Self: Sized {}
}

#[derive(Clone, Copy, Debug)]
pub struct SettingsLocation;
pub struct SettingsStore;
impl Global for SettingsStore {}
impl SettingsStore {
    pub fn test(_cx: &mut App) -> Self { Self }
    pub fn update_user_settings<F>(&mut self, _cx: &mut App, _f: F) where F: FnOnce(&mut SettingsContent) {}
}

// ---- IntoGpui ----
pub trait IntoGpui {
    type Output;
    fn into_gpui(self) -> Self::Output;
}
impl IntoGpui for FontSize {
    type Output = Pixels;
    fn into_gpui(self) -> Pixels { px(self.0) }
}
impl IntoGpui for PixelSetting {
    type Output = Pixels;
    fn into_gpui(self) -> Pixels { px(self.0) }
}
impl IntoGpui for FontWeightContent {
    type Output = FontWeight;
    fn into_gpui(self) -> FontWeight { FontWeight(self.0.clamp(100., 950.)) }
}
impl IntoGpui for FontFeaturesContent {
    type Output = FontFeatures;
    fn into_gpui(self) -> FontFeatures { FontFeatures(Arc::new(self.0.into_iter().collect())) }
}
impl IntoGpui for FontFamilyName {
    type Output = SharedString;
    fn into_gpui(self) -> SharedString { SharedString::from(self.0) }
}

// ---- merge_from ----
pub mod merge_from {
    pub trait MergeFrom {
        fn merge_from(&mut self, other: &Self);
        fn merge_from_option(&mut self, other: Option<&Self>) { if let Some(o) = other { self.merge_from(o); } }
    }
    impl<T: Clone> MergeFrom for Option<T> { fn merge_from(&mut self, other: &Self) { if other.is_some() { *self = other.clone(); } } }
}

// ---- Enums / content types ----
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AlternateScroll { On, Off, #[default] Auto }

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PathHyperlinkRegex {
    SingleLine(String),
    MultiLine(Vec<String>),
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ShowScrollbar { #[default] Auto, System, Always, Never }

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalBell { #[default] Off, On, Audible, Visual }

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalBlink { #[default] Off, Terminal, List }

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalDockPosition { #[default] Bottom, Left, Right }

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalLineHeight { #[default] Comfortable, Standard, Custom(f32) }

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VenvSettings { #[default] On, Off }

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkingDirectory {
    #[default]
    CurrentProjectDirectory,
    AlwaysHome,
    Always { directory: PathBuf },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Shell {
    #[default]
    System,
    Program(String),
    WithArguments { program: String, args: Vec<String>, title_override: Option<String> },
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CursorShapeContent { #[default] Block, Underline, Bar, Hollow }

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct FontFamilyName(pub Arc<str>);
impl From<String> for FontFamilyName { fn from(s: String) -> Self { Self(s.into()) } }
impl From<&str> for FontFamilyName { fn from(s: &str) -> Self { Self(s.into()) } }
impl std::fmt::Display for FontFamilyName { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}", self.0) } }

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct FontSize(pub f32);
impl From<f32> for FontSize { fn from(v: f32) -> Self { Self(v) } }
impl std::fmt::Display for FontSize { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{:.2}", self.0) } }

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FontFeaturesContent(pub IndexMap<String, u32>);

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct FontWeightContent(pub f32);
impl Default for FontWeightContent { fn default() -> Self { Self(400.0) } }
impl From<f32> for FontWeightContent { fn from(v: f32) -> Self { Self(v) } }
impl FontWeightContent {
    pub const THIN: Self = Self(100.0);
    pub const EXTRA_LIGHT: Self = Self(200.0);
    pub const LIGHT: Self = Self(300.0);
    pub const NORMAL: Self = Self(400.0);
    pub const MEDIUM: Self = Self(500.0);
    pub const SEMIBOLD: Self = Self(600.0);
    pub const BOLD: Self = Self(700.0);
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct PixelSetting(pub f32);
impl From<f32> for PixelSetting { fn from(v: f32) -> Self { Self(v) } }
impl From<PixelSetting> for Pixels { fn from(p: PixelSetting) -> Self { px(p.0) } }

// ---- SettingsContent ----
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SettingsContent {
    pub terminal: Option<TerminalSettingsContent>,
    #[serde(flatten)]
    pub project: ProjectSettingsContent,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ProjectSettingsContent {
    pub terminal: Option<ProjectTerminalSettingsContent>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TerminalSettingsContent {
    #[serde(flatten)]
    pub project: ProjectTerminalSettingsContent,
    pub font_size: Option<FontSize>,
    pub font_family: Option<FontFamilyName>,
    pub font_fallbacks: Option<Vec<FontFamilyName>>,
    pub line_height: Option<TerminalLineHeight>,
    pub font_features: Option<FontFeaturesContent>,
    pub font_weight: Option<FontWeightContent>,
    pub cursor_shape: Option<CursorShapeContent>,
    pub blinking: Option<TerminalBlink>,
    pub alternate_scroll: Option<AlternateScroll>,
    pub option_as_meta: Option<bool>,
    pub copy_on_select: Option<bool>,
    pub keep_selection_on_copy: Option<bool>,
    pub open_links_in_mouse_mode: Option<bool>,
    pub button: Option<bool>,
    pub dock: Option<TerminalDockPosition>,
    pub starts_open: Option<bool>,
    pub flexible: Option<bool>,
    pub default_width: Option<PixelSetting>,
    pub default_height: Option<PixelSetting>,
    pub scroll_multiplier: Option<f32>,
    pub max_scroll_history_lines: Option<usize>,
    pub toolbar: Option<TerminalToolbarContent>,
    pub scrollbar: Option<ScrollbarSettingsContent>,
    pub minimum_contrast: Option<f32>,
    pub show_count_badge: Option<bool>,
    pub bell: Option<TerminalBell>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProjectTerminalSettingsContent {
    pub shell: Option<Shell>,
    pub working_directory: Option<WorkingDirectory>,
    pub env: Option<HashMap<String, String>>,
    pub detect_venv: Option<VenvSettings>,
    pub path_hyperlink_regexes: Option<Vec<PathHyperlinkRegex>>,
    pub path_hyperlink_timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TerminalToolbarContent {
    pub breadcrumbs: Option<bool>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ScrollbarSettingsContent {
    pub show: Option<ShowScrollbar>,
}

pub fn init(_cx: &mut App) {}

pub mod fallible_options {
    use serde::de::{Deserialize, Deserializer};
    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        Ok(Option::<T>::deserialize(deserializer).unwrap_or(None))
    }
}
#[doc(hidden)]
pub mod private {
    pub use inventory;
    pub struct RegisteredSetting {
        pub settings_value: fn() -> Box<dyn std::any::Any>,
        pub from_settings: fn(&crate::SettingsContent) -> Box<dyn std::any::Any>,
        pub id: fn() -> std::any::TypeId,
    }
    inventory::collect!(RegisteredSetting);
    pub struct SettingValue<T> {
        pub global_value: Option<T>,
        pub local_values: Vec<T>,
    }
}

impl merge_from::MergeFrom for TerminalSettingsContent { fn merge_from(&mut self, other: &Self) { *self = other.clone(); } }
impl merge_from::MergeFrom for ProjectTerminalSettingsContent { fn merge_from(&mut self, other: &Self) { *self = other.clone(); } }
impl merge_from::MergeFrom for SettingsContent { fn merge_from(&mut self, other: &Self) { *self = other.clone(); } }
impl merge_from::MergeFrom for ProjectSettingsContent { fn merge_from(&mut self, other: &Self) { *self = other.clone(); } }

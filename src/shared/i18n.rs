//! 应用界面国际化与全局设置：语言偏好、系统语言检测和 GPUI 全局状态。

use std::fs;
use std::path::{Path, PathBuf};

use gpui::{App, BorrowAppContext, Global, ReadGlobal};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locale {
    English,
    SimplifiedChinese,
}

pub const DEFAULT_TERMINAL_FONT_SIZE: f32 = 14.0;
pub const DEFAULT_TERMINAL_SCROLLBACK: usize = 1000;
pub const MIN_TERMINAL_FONT_SIZE: f32 = 10.0;
pub const MAX_TERMINAL_FONT_SIZE: f32 = 24.0;
pub const MIN_TERMINAL_SCROLLBACK: usize = 100;
pub const MAX_TERMINAL_SCROLLBACK: usize = 100_000;
/// 侧栏 Local 分组保留的最近本地目录数。
pub const DEFAULT_RECENT_LOCAL_DIRS_MAX: usize = 10;
pub const MIN_RECENT_LOCAL_DIRS_MAX: usize = 1;
pub const MAX_RECENT_LOCAL_DIRS_MAX: usize = 50;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AppSettings {
    #[serde(default)]
    pub language: LanguagePreference,
    #[serde(default = "default_show_timestamps")]
    pub show_timestamps: bool,
    #[serde(default = "default_terminal_notifications")]
    pub terminal_notifications: bool,
    #[serde(default = "default_terminal_font_size")]
    pub terminal_font_size: f32,
    #[serde(default = "default_terminal_scrollback")]
    pub terminal_scrollback: usize,
    /// 最近打开过的本地目录（最近优先），退出后仍保留在侧栏 Local 分组。
    #[serde(default)]
    pub recent_local_dirs: Vec<PathBuf>,
    #[serde(default = "default_recent_local_dirs_max")]
    pub recent_local_dirs_max: usize,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            language: LanguagePreference::default(),
            show_timestamps: default_show_timestamps(),
            terminal_notifications: default_terminal_notifications(),
            terminal_font_size: default_terminal_font_size(),
            terminal_scrollback: default_terminal_scrollback(),
            recent_local_dirs: Vec::new(),
            recent_local_dirs_max: default_recent_local_dirs_max(),
        }
    }
}

impl AppSettings {
    pub fn normalized(mut self) -> Self {
        self.terminal_font_size = if self.terminal_font_size.is_finite() {
            self.terminal_font_size
                .clamp(MIN_TERMINAL_FONT_SIZE, MAX_TERMINAL_FONT_SIZE)
        } else {
            DEFAULT_TERMINAL_FONT_SIZE
        };
        self.terminal_scrollback = self
            .terminal_scrollback
            .clamp(MIN_TERMINAL_SCROLLBACK, MAX_TERMINAL_SCROLLBACK);
        self.recent_local_dirs_max = self
            .recent_local_dirs_max
            .clamp(MIN_RECENT_LOCAL_DIRS_MAX, MAX_RECENT_LOCAL_DIRS_MAX);
        if self.recent_local_dirs.len() > self.recent_local_dirs_max {
            self.recent_local_dirs.truncate(self.recent_local_dirs_max);
        }
        self
    }
}

fn default_show_timestamps() -> bool {
    true
}

fn default_terminal_notifications() -> bool {
    true
}

fn default_terminal_font_size() -> f32 {
    DEFAULT_TERMINAL_FONT_SIZE
}

fn default_terminal_scrollback() -> usize {
    DEFAULT_TERMINAL_SCROLLBACK
}

fn default_recent_local_dirs_max() -> usize {
    DEFAULT_RECENT_LOCAL_DIRS_MAX
}

impl Locale {
    pub const fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::SimplifiedChinese => "zh-CN",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub enum LanguagePreference {
    #[default]
    System,
    English,
    #[serde(rename = "zh-CN")]
    SimplifiedChinese,
}

impl LanguagePreference {
    pub const ALL: [Self; 3] = [Self::System, Self::English, Self::SimplifiedChinese];

    pub const fn locale(self) -> Locale {
        match self {
            Self::English => Locale::English,
            Self::SimplifiedChinese => Locale::SimplifiedChinese,
            Self::System => Locale::English,
        }
    }

    pub fn resolve(self) -> Locale {
        match self {
            Self::System => system_locale(),
            _ => self.locale(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct I18nState {
    pub preference: LanguagePreference,
    pub locale: Locale,
    pub settings: AppSettings,
}

impl Global for I18nState {}

const SETTINGS_FILE_NAME: &str = "settings.toml";

pub fn init<C: BorrowAppContext>(cx: &mut C) {
    let settings = load_settings().normalized();
    let preference = settings.language;
    let locale = preference.resolve();
    rust_i18n::set_locale(locale.code());
    cx.set_global(I18nState {
        preference,
        locale,
        settings,
    });
}

pub fn settings(cx: &App) -> AppSettings {
    I18nState::global(cx).settings.clone()
}

pub fn set_settings<C: BorrowAppContext>(cx: &mut C, settings: AppSettings) {
    let settings = settings.normalized();
    let locale = settings.language.resolve();
    rust_i18n::set_locale(locale.code());
    if let Err(error) = save_settings(&settings) {
        log::warn!("failed to save settings: {error}");
    }
    let language = settings.language;
    cx.update_global::<I18nState, _>(|state, _| {
        *state = I18nState {
            preference: language,
            locale,
            settings,
        };
    });
}

pub fn text(key: &str) -> String {
    rust_i18n::t!(key).to_string()
}

#[cfg(test)]
fn text_for(key: &str, locale: Locale) -> String {
    rust_i18n::t!(key, locale = locale.code()).to_string()
}

pub fn preference_label(preference: LanguagePreference) -> String {
    text(match preference {
        LanguagePreference::System => "language.system",
        LanguagePreference::English => "language.english",
        LanguagePreference::SimplifiedChinese => "language.simplified_chinese",
    })
}

fn load_settings() -> AppSettings {
    let Some(path) = settings_path() else {
        return AppSettings::default();
    };
    match fs::read_to_string(path) {
        Ok(contents) => match toml::from_str::<AppSettings>(&contents) {
            Ok(settings) => settings,
            Err(error) => {
                log::warn!("failed to parse settings: {error}");
                AppSettings::default()
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => AppSettings::default(),
        Err(error) => {
            log::warn!("failed to read settings: {error}");
            AppSettings::default()
        }
    }
}

fn save_settings(settings: &AppSettings) -> std::io::Result<()> {
    let Some(path) = settings_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let contents = toml::to_string_pretty(settings).map_err(std::io::Error::other)?;
    fs::write(path, contents)
}

fn settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| settings_path_from_home(&home))
}

fn settings_path_from_home(home: &Path) -> PathBuf {
    home.join(".config").join("crossh").join(SETTINGS_FILE_NAME)
}

fn system_locale() -> Locale {
    sys_locale::get_locale()
        .as_deref()
        .map(locale_from_system_tag)
        .unwrap_or(Locale::English)
}

fn locale_from_system_tag(tag: &str) -> Locale {
    if tag.eq_ignore_ascii_case("zh") || tag.to_ascii_lowercase().starts_with("zh-") {
        Locale::SimplifiedChinese
    } else {
        Locale::English
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_system_tags_resolve_to_simplified_chinese() {
        assert_eq!(locale_from_system_tag("zh-CN"), Locale::SimplifiedChinese);
        assert_eq!(locale_from_system_tag("zh-Hans"), Locale::SimplifiedChinese);
        assert_eq!(locale_from_system_tag("en-US"), Locale::English);
    }

    #[test]
    fn explicit_preferences_override_system_locale() {
        assert_eq!(LanguagePreference::English.resolve(), Locale::English);
        assert_eq!(
            LanguagePreference::SimplifiedChinese.resolve(),
            Locale::SimplifiedChinese
        );
    }

    #[test]
    fn resource_lookup_supports_both_locales() {
        assert_eq!(text_for("language.english", Locale::English), "English");
        assert_eq!(
            text_for("language.english", Locale::SimplifiedChinese),
            "英语"
        );
        assert_eq!(text_for("quit.title", Locale::English), "Quit Crossh?");
        assert_eq!(
            text_for("quit.title", Locale::SimplifiedChinese),
            "要退出 Crossh 吗？"
        );
    }

    #[test]
    fn language_preference_round_trips_as_stable_toml() {
        let encoded = toml::to_string(&AppSettings {
            language: LanguagePreference::SimplifiedChinese,
            ..AppSettings::default()
        })
        .expect("language preference should serialize");
        assert!(encoded.lines().any(|line| line == "language = \"zh-CN\""));

        let decoded: AppSettings = toml::from_str(&encoded).expect("settings should deserialize");
        assert_eq!(
            decoded,
            AppSettings {
                language: LanguagePreference::SimplifiedChinese,
                ..AppSettings::default()
            }
        );
    }

    #[test]
    fn missing_new_settings_use_defaults() {
        let settings: AppSettings = toml::from_str("language = \"English\"")
            .expect("legacy language-only settings should deserialize");
        assert_eq!(
            settings,
            AppSettings {
                language: LanguagePreference::English,
                ..AppSettings::default()
            }
        );
    }

    #[test]
    fn terminal_settings_round_trip() {
        let settings = AppSettings {
            language: LanguagePreference::English,
            show_timestamps: false,
            terminal_font_size: 18.0,
            terminal_scrollback: 5000,
            ..AppSettings::default()
        };
        let encoded = toml::to_string(&settings).expect("settings should serialize");
        let decoded: AppSettings = toml::from_str(&encoded).expect("settings should deserialize");
        assert_eq!(decoded, settings);
    }

    #[test]
    fn terminal_settings_are_normalized_to_safe_ranges() {
        let settings = AppSettings {
            terminal_font_size: 200.0,
            terminal_scrollback: usize::MAX,
            ..AppSettings::default()
        }
        .normalized();
        assert_eq!(settings.terminal_font_size, MAX_TERMINAL_FONT_SIZE);
        assert_eq!(settings.terminal_scrollback, MAX_TERMINAL_SCROLLBACK);
    }

    #[test]
    fn recent_local_dirs_round_trip_and_are_capped() {
        let settings = AppSettings {
            recent_local_dirs: vec![
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/c"),
            ],
            recent_local_dirs_max: 2,
            ..AppSettings::default()
        }
        .normalized();
        assert_eq!(
            settings.recent_local_dirs,
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );

        let encoded = toml::to_string(&settings).expect("settings should serialize");
        let decoded: AppSettings = toml::from_str(&encoded).expect("settings should deserialize");
        assert_eq!(decoded, settings);
    }

    #[test]
    fn recent_local_dirs_max_is_clamped() {
        let settings = AppSettings {
            recent_local_dirs_max: 0,
            ..AppSettings::default()
        }
        .normalized();
        assert_eq!(settings.recent_local_dirs_max, MIN_RECENT_LOCAL_DIRS_MAX);

        let settings = AppSettings {
            recent_local_dirs_max: 999,
            ..AppSettings::default()
        }
        .normalized();
        assert_eq!(settings.recent_local_dirs_max, MAX_RECENT_LOCAL_DIRS_MAX);
    }

    #[test]
    fn settings_use_the_xdg_style_crossh_directory() {
        assert_eq!(
            settings_path_from_home(Path::new("/Users/example")),
            PathBuf::from("/Users/example/.config/crossh/settings.toml")
        );
    }
}

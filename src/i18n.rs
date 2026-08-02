//! 应用界面国际化：语言偏好、系统语言检测和 GPUI 全局状态。

use std::fs;
use std::path::{Path, PathBuf};

use gpui::{App, BorrowAppContext, Global, ReadGlobal};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locale {
    English,
    SimplifiedChinese,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct I18nState {
    pub preference: LanguagePreference,
    pub locale: Locale,
}

impl Global for I18nState {}

#[derive(Debug, Default, Deserialize, Serialize)]
struct SettingsFile {
    #[serde(default)]
    language: LanguagePreference,
}

const SETTINGS_FILE_NAME: &str = "settings.toml";

pub fn init<C: BorrowAppContext>(cx: &mut C) {
    let preference = load_preference();
    let locale = preference.resolve();
    rust_i18n::set_locale(locale.code());
    cx.set_global(I18nState { preference, locale });
}

pub fn preference(cx: &App) -> LanguagePreference {
    I18nState::global(cx).preference
}

pub fn set_preference<C: BorrowAppContext>(cx: &mut C, preference: LanguagePreference) {
    let locale = preference.resolve();
    rust_i18n::set_locale(locale.code());
    if let Err(error) = save_preference(preference) {
        log::warn!("failed to save language preference: {error}");
    }
    cx.update_global::<I18nState, _>(|state, _| {
        *state = I18nState { preference, locale };
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

pub fn language_short_label(locale: Locale) -> &'static str {
    match locale {
        Locale::English => "EN",
        Locale::SimplifiedChinese => "中",
    }
}

fn load_preference() -> LanguagePreference {
    let Some(path) = settings_path() else {
        return LanguagePreference::default();
    };
    match fs::read_to_string(path) {
        Ok(contents) => match toml::from_str::<SettingsFile>(&contents) {
            Ok(settings) => settings.language,
            Err(error) => {
                log::warn!("failed to parse language settings: {error}");
                LanguagePreference::default()
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => LanguagePreference::default(),
        Err(error) => {
            log::warn!("failed to read language settings: {error}");
            LanguagePreference::default()
        }
    }
}

fn save_preference(preference: LanguagePreference) -> std::io::Result<()> {
    let Some(path) = settings_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let contents = toml::to_string_pretty(&SettingsFile {
        language: preference,
    })
    .map_err(std::io::Error::other)?;
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
    }

    #[test]
    fn language_preference_round_trips_as_stable_toml() {
        let encoded = toml::to_string(&SettingsFile {
            language: LanguagePreference::SimplifiedChinese,
        })
        .expect("language preference should serialize");
        assert_eq!(encoded.trim(), "language = \"zh-CN\"");

        let decoded: SettingsFile = toml::from_str(&encoded).expect("language should deserialize");
        assert_eq!(decoded.language, LanguagePreference::SimplifiedChinese);
    }

    #[test]
    fn settings_use_the_xdg_style_crossh_directory() {
        assert_eq!(
            settings_path_from_home(Path::new("/Users/example")),
            PathBuf::from("/Users/example/.config/crossh/settings.toml")
        );
    }
}

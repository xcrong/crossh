//! Application locale state and translation helpers.
//!
//! `Global` is GPUI application-state storage rather than view coupling; a complete UI-dependency
//! break would require the settings feature to own and inject this state.

use gpui::{BorrowAppContext, Global};
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

#[derive(Clone, Debug, PartialEq)]
pub struct I18nState {
    pub preference: LanguagePreference,
    pub locale: Locale,
}

impl Global for I18nState {}

pub fn init<C: BorrowAppContext>(cx: &mut C, preference: LanguagePreference) {
    set_locale(preference);
    let locale = preference.resolve();
    cx.set_global(I18nState { preference, locale });
}

pub fn set_language<C: BorrowAppContext>(cx: &mut C, preference: LanguagePreference) {
    set_locale(preference);
    let locale = preference.resolve();
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

fn set_locale(preference: LanguagePreference) {
    rust_i18n::set_locale(preference.resolve().code());
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
        assert_eq!(
            text_for("settings.providers", Locale::English),
            "LLM Providers"
        );
        assert_eq!(text_for("settings.agent", Locale::English), "Agent");
        assert_eq!(
            text_for("app_menu.check_for_updates", Locale::English),
            "Check for Updates…"
        );
        assert_eq!(
            text_for("settings.providers", Locale::SimplifiedChinese),
            "模型供应商"
        );
        assert_eq!(
            text_for("settings.agent", Locale::SimplifiedChinese),
            "Agent"
        );
        assert_eq!(
            text_for("app_menu.check_for_updates", Locale::SimplifiedChinese),
            "检查更新…"
        );
    }
}

//! Application locale helpers (pure logic, zero UI dependencies).
//!
//! 语言切换由调用方直接调用 [`set_locale`] 完成；本模块只保留 locale 解析与翻译查询。

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

pub(crate) fn set_locale(preference: LanguagePreference) {
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
        assert_eq!(text_for("tab_close.title", Locale::English), "Close Tab?");
        assert_eq!(
            text_for("tab_close.title", Locale::SimplifiedChinese),
            "关闭标签页？"
        );
        assert_eq!(
            text_for("tab_close.confirm", Locale::English),
            "Close Anyway"
        );
        assert_eq!(
            text_for("tab_close.confirm", Locale::SimplifiedChinese),
            "仍然关闭"
        );
        assert_eq!(
            text_for("app_menu.check_for_updates", Locale::English),
            "Check for Updates…"
        );
        assert_eq!(
            text_for("app_menu.check_for_updates", Locale::SimplifiedChinese),
            "检查更新…"
        );
        assert_eq!(
            text_for("toast.path_copied", Locale::English),
            "Path copied"
        );
        assert_eq!(
            text_for("toast.path_copied", Locale::SimplifiedChinese),
            "路径已复制"
        );
        assert_eq!(
            rust_i18n::t!(
                "git.selection_count",
                locale = Locale::English.code(),
                count = 2
            )
            .to_string(),
            "2 selected"
        );
        assert_eq!(
            rust_i18n::t!(
                "git.selection_count",
                locale = Locale::SimplifiedChinese.code(),
                count = 2
            )
            .to_string(),
            "已选 2 项"
        );
        assert_eq!(
            text_for("git.stage_hunk", Locale::English),
            "Stage this hunk"
        );
        assert_eq!(
            text_for("git.stage_hunk", Locale::SimplifiedChinese),
            "暂存此 Hunk"
        );
        assert_eq!(text_for("git.history_tab", Locale::English), "History");
        assert_eq!(
            text_for("git.history_tab", Locale::SimplifiedChinese),
            "历史"
        );
        assert_eq!(text_for("git.branches_tab", Locale::English), "Branches");
        assert_eq!(
            text_for("git.branches_tab", Locale::SimplifiedChinese),
            "分支"
        );
        assert_eq!(text_for("git.stashes_tab", Locale::English), "Stash");
        assert_eq!(
            text_for("git.stashes_tab", Locale::SimplifiedChinese),
            "暂存"
        );
        assert_eq!(
            text_for("git.conflict_use_ours", Locale::English),
            "Use current"
        );
        assert_eq!(
            rust_i18n::t!(
                "git.files_changed",
                locale = Locale::English.code(),
                count = 3
            )
            .to_string(),
            "3 files changed"
        );
    }
}

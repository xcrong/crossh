//! Locale bootstrap shared by every binary (`crossh`, `crossh-git`, `crossh-note`).
//!
//! 主应用通过 `features/settings` 加载完整快照后再设置 locale；独立二进制不挂载
//! settings feature，此处只读 `settings.toml` 顶层 `language` 键，与
//! `SettingsFile::language` 的序列化格式保持一致（`"System"` / `"English"` /
//! `"zh-CN"`，缺失或非法时跟随系统）。
//!
//! 只做字符串级解析：即使文件其他字段损坏，语言选择依然有效；非法值走向跟随系统，
//! 与主应用全文件解析失败即回退默认的可观察结果一致。
//!
//! 中文判定规则与 `src/shared/i18n.rs::locale_from_system_tag` 同步维护。

/// rust-i18n 中文 locale 代号，与 `locales/zh-CN.yml` 对应。
pub const SIMPLIFIED_CHINESE_CODE: &str = "zh-CN";
/// rust-i18n 英文 locale 代号，与 `locales/en.yml` 对应。
pub const ENGLISH_CODE: &str = "en";

/// 当前系统语言对应的 locale 代号：中文系（`zh` / `zh-*`）取中文，其余取英文。
pub fn system_locale_code() -> &'static str {
    if is_chinese_tag(sys_locale::get_locale().as_deref().unwrap_or("")) {
        SIMPLIFIED_CHINESE_CODE
    } else {
        ENGLISH_CODE
    }
}

/// 读持久化语言偏好并折成可直接 `rust_i18n::set_locale` 的代号。
/// 文件缺失、读取失败、`language` 缺失或非法值一律跟随系统。
pub fn persisted_locale_code() -> &'static str {
    let Some(path) = settings_path() else {
        return system_locale_code();
    };
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return system_locale_code();
    };
    // 先算系统代号再解析：缺失 / "System" / 非法值都回落到它。
    let system = system_locale_code();
    resolve_code(parse_language_value(&contents), system)
}

fn settings_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|home| home.join(".config").join("crossh").join("settings.toml"))
}

fn resolve_code(language: Option<&str>, system_code: &'static str) -> &'static str {
    match language {
        Some("zh-CN") => SIMPLIFIED_CHINESE_CODE,
        Some("English") => ENGLISH_CODE,
        _ => system_code,
    }
}

/// 提取顶层 `language = "..."` 的原始值；容忍缩进、行尾注释与单引号。
fn parse_language_value(contents: &str) -> Option<&str> {
    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some(rest) = line.strip_prefix("language") else {
            continue;
        };
        // `languages` 等前缀相近的键不得误命中：键后只允许空白或 `=`。
        let rest = rest.trim_start();
        if !rest.starts_with('=') {
            continue;
        }
        let rest = rest.strip_prefix('=').unwrap_or("").trim_start();
        if let Some(quoted) = rest.strip_prefix('"') {
            return quoted.split('"').next();
        }
        if let Some(quoted) = rest.strip_prefix('\'') {
            return quoted.split('\'').next();
        }
        // 非引号裸值（如 `language = System`）：取到空白或注释为止。
        let token: &str = rest.split([' ', '\t', '#']).next().unwrap_or("").trim();
        if token.is_empty() {
            return None;
        }
        return Some(token);
    }
    None
}

fn is_chinese_tag(tag: &str) -> bool {
    tag.eq_ignore_ascii_case("zh") || tag.to_ascii_lowercase().starts_with("zh-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_system_tags_resolve_to_simplified_chinese() {
        assert!(is_chinese_tag("zh"));
        assert!(is_chinese_tag("ZH"));
        assert!(is_chinese_tag("zh-CN"));
        assert!(is_chinese_tag("zh-Hans"));
        assert!(is_chinese_tag("zh-TW"));
        assert!(!is_chinese_tag("en-US"));
        assert!(!is_chinese_tag(""));
    }

    #[test]
    fn language_values_fold_to_locale_codes() {
        assert_eq!(resolve_code(Some("zh-CN"), ENGLISH_CODE), "zh-CN");
        assert_eq!(resolve_code(Some("English"), "zh-CN"), "en");
        assert_eq!(resolve_code(Some("System"), "zh-CN"), "zh-CN");
        assert_eq!(resolve_code(Some("System"), ENGLISH_CODE), "en");
        assert_eq!(resolve_code(None, ENGLISH_CODE), "en");
        assert_eq!(resolve_code(Some("bogus"), ENGLISH_CODE), "en");
    }

    #[test]
    fn language_key_parsing_tolerates_formatting_noise() {
        assert_eq!(parse_language_value("language = \"zh-CN\""), Some("zh-CN"));
        assert_eq!(
            parse_language_value("  language=  'English'  # comment"),
            Some("English")
        );
        assert_eq!(
            parse_language_value("# language = \"zh-CN\"\nlanguage = \"English\""),
            Some("English")
        );
        assert_eq!(
            parse_language_value("terminal_font_size = 18.0\nlanguage = System"),
            Some("System")
        );
        assert_eq!(parse_language_value("terminal_font_size = 18.0"), None);
        assert_eq!(parse_language_value("language = "), None);
        // `languages` 等前缀相近的键不得误命中。
        assert_eq!(parse_language_value("languages = \"zh-CN\""), None);
    }
}

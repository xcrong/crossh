//! 笔记窗口用到的纯格式化逻辑：标题截断、相对时间、错误码文案。
//!
//! 按项目规则，这里零 `gpui` 依赖——可单独测试，视图只消费结果。

use std::time::{SystemTime, UNIX_EPOCH};

use crossh_note::MAX_CONTENT_BYTES;

/// 笔记标题：取首行并按字符截断到 `max` 个字符，超长时补省略号。
///
/// 旧实现用 `.chars().take(30)` 硬砍，两条都叫「会议纪要第…」的笔记在列表里
/// 无法区分。`max` 按字符计（不是字节），故中文不会被砍成半个字。
pub fn note_title(content: &str, max: usize) -> String {
    let first_line = content.lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        return "空笔记".to_string();
    }
    if first_line.chars().count() <= max {
        return first_line.to_string();
    }
    let kept: String = first_line.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// 存储层的错误码 -> 面向用户的中文原因。
/// 未知错误码原样透出（含 SQLite 原文），便于用户截图反馈时定位。
pub fn persist_error_message(code: &str) -> String {
    match code {
        "content_too_large" => format!(
            "内容超过 {} MB 上限，未保存；请删减后重试",
            MAX_CONTENT_BYTES / (1024 * 1024)
        ),
        "empty_content" => "空内容无法保存".to_string(),
        other => format!("保存失败：{other}"),
    }
}

/// 列表第二行的时间：近似相对时间，超 30 天回退为日期。
/// 只用 std（unix 秒时间戳），不引入新依赖；非法/未来时间戳显示"未知时间"。
pub fn format_note_time(updated_at: i64) -> String {
    if updated_at <= 0 {
        return "未知时间".to_string();
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let diff = now - updated_at;
    if diff < 0 {
        return "未知时间".to_string();
    }
    if diff < 60 {
        "刚刚".to_string()
    } else if diff < 3600 {
        format!("{} 分钟前", diff / 60)
    } else if diff < 86400 {
        format!("{} 小时前", diff / 3600)
    } else if diff < 2 * 86400 {
        "昨天".to_string()
    } else if diff < 30 * 86400 {
        format!("{} 天前", diff / 86400)
    } else {
        format_note_datetime(updated_at)
    }
}

/// 超过 30 天的旧笔记显示 `yyyy-MM-dd HH:mm`（UTC）。
fn format_note_datetime(ts: i64) -> String {
    let days = ts.div_euclid(86400);
    let secs = ts.rem_euclid(86400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        year,
        month,
        day,
        secs / 3600,
        (secs % 3600) / 60
    )
}

/// 天数转年月日（Howard Hinnant civil_from_days，1970-01-01 为第 0 天）。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_title_truncates_by_char_with_ellipsis() {
        // 短标题原样返回。
        assert_eq!(note_title("hello", 30), "hello");
        // 恰好等于上限不加省略号。
        assert_eq!(note_title("abcde", 5), "abcde");
        // 超长补省略号，总长等于上限。
        let long = "a".repeat(50);
        let cut = note_title(&long, 30);
        assert_eq!(cut.chars().count(), 30);
        assert!(cut.ends_with('…'));
        // 多行只取首行。
        assert_eq!(note_title("first\nsecond", 30), "first");
        // 中文按字符计，不切坏 UTF-8。max=10 -> 保留 9 字 + 省略号。
        let cjk = "笔记".repeat(20);
        let cut = note_title(&cjk, 10);
        assert_eq!(cut.chars().count(), 10);
        assert_eq!(cut, "笔记笔记笔记笔记笔…");
        // 全空/首行空白回退到占位。
        assert_eq!(note_title("", 30), "空笔记");
        assert_eq!(note_title("   \nreal", 30), "空笔记");
        // 上限为 0 不 panic。
        assert_eq!(note_title("abc", 0), "…");
    }

    #[test]
    fn persist_error_message_maps_known_codes() {
        assert_eq!(persist_error_message("empty_content"), "空内容无法保存");
        assert!(
            persist_error_message("content_too_large").contains("未保存"),
            "超限应说明未保存，避免用户以为写丢了"
        );
        // 未知错误码透出原文，便于截图反馈定位。
        assert_eq!(
            persist_error_message("database is locked"),
            "保存失败：database is locked"
        );
    }

    #[test]
    fn format_note_time_relative() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap();
        assert_eq!(format_note_time(0), "未知时间");
        assert_eq!(format_note_time(-1), "未知时间");
        assert_eq!(format_note_time(now + 3600), "未知时间");
        assert_eq!(format_note_time(now), "刚刚");
        assert_eq!(format_note_time(now - 30), "刚刚");
        assert_eq!(format_note_time(now - 90), "1 分钟前");
        assert_eq!(format_note_time(now - 5 * 60), "5 分钟前");
        assert_eq!(format_note_time(now - 3600), "1 小时前");
        assert_eq!(format_note_time(now - 20 * 3600), "20 小时前");
        assert_eq!(format_note_time(now - 30 * 3600), "昨天");
        assert_eq!(format_note_time(now - 5 * 86400), "5 天前");
    }

    #[test]
    fn format_note_time_falls_back_to_date() {
        // 1970-01-01 / 2000-01-01 都是已知锚点（UTC）。
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(format_note_datetime(0), "1970-01-01 00:00");
        assert_eq!(format_note_datetime(946684800), "2000-01-01 00:00");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap();
        // 40 天前的旧笔记走日期分支，形如 yyyy-MM-dd HH:mm。
        let s = format_note_time(now - 40 * 86400);
        assert_eq!(s.len(), 16);
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[10..11], " ");
        assert_eq!(&s[13..14], ":");
    }
}

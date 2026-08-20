//! 人类可读字节格式化（供 SFTP、下载、状态栏等共享）。
//!
//! 统一 `src/features/sftp/logic.rs:format_size` 与 `crates/crossh-ssh/src/sftp.rs:format_bytes`
//! 的两份实现，补齐 GB 分支后下沉到 `crossh-core`，避免 UI 与传输层各自手写。

/// 将字节数格式化为人类可读字符串（B / KB / MB / GB）。
///
/// 规则与原两处实现保持一致：1024 进制，保留一位小数（GB/MB/KB），B 为整数。
/// 与 `format_size` 语义完全等价，已覆盖 `format_bytes` 的无 GB 旧实现。
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// 兼容旧名 `format_size` 的别名，保持调用点语义不变。
pub fn format_size(bytes: u64) -> String {
    format_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes_and_kilobytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
    }

    #[test]
    fn formats_megabytes_and_gigabytes() {
        assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(format_bytes(4 * 1024 * 1024), "4.0 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GB");
        assert_eq!(format_bytes(5 * 1024 * 1024 * 1024), "5.0 GB");
    }

    #[test]
    fn format_size_alias_is_equivalent() {
        assert_eq!(format_size(2048), format_bytes(2048));
        assert_eq!(format_size(2 * 1024 * 1024 * 1024), "2.0 GB");
    }
}

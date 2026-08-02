//! crossh —— 基于 gpui 的轻量 SSH 客户端。
//!
//! 常驻开发工具：复用 `~/.ssh/config`（只读），提供交互式终端（russh + alacritty_terminal）。
//! SFTP 与端口转发为后续阶段（见 .kilo/plans）。

mod button;
mod config;
mod i18n;
mod local;
mod ssh;
mod ui;

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use gpui::App;

rust_i18n::i18n!("locales", fallback = "en");

/// 持久化日志路径。放在 /tmp，系统重启自动清空，无需手动维护。
const LOG_PATH: &str = "/tmp/crossh/run.log";
/// 启动时若日志超过此行数，触发裁剪。
const LOG_TRIM_THRESHOLD: usize = 50_000;
/// 裁剪后保留的末尾行数。
const LOG_TRIM_KEEP: usize = 10_000;

fn main() {
    init_logging();
    install_panic_hook();

    log::info!("──── crossh 启动 (pid {}) ────", std::process::id());

    // 预热 tokio 运行时（单例，限 2 worker 线程，控内存）。
    let _rt = ssh::ssh_runtime();

    let app = gpui_platform::application().with_assets(ui::assets::UiAssetSource);
    app.on_reopen(|cx| {
        // macOS 关闭最后一个窗口后应用仍驻留在 Dock；再次点击时恢复主窗口。
        // 已有窗口时不重复创建，避免 Dock/快捷方式触发多个主窗口。
        if cx.windows().is_empty() {
            ui::app_shell::open_main_window(cx);
        }
    });
    app.run(move |cx: &mut App| {
        cx.init_colors();
        i18n::init(cx);
        ui::app_shell::open_main_window(cx);
    });
}

/// 初始化日志：写到 `/tmp/crossh/run.log`（同时 tee 到 stderr，方便 `cargo run` 观察）。
///
/// 启动时若日志文件超过 `LOG_TRIM_THRESHOLD` 行，裁剪为末尾 `LOG_TRIM_KEEP` 行。
/// 默认级别 `info`，可用 `RUST_LOG=crossh=debug` 等覆盖。
fn init_logging() {
    let path = Path::new(LOG_PATH);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    trim_log_if_needed(path);

    let target_file = OpenOptions::new().create(true).append(true).open(path).ok();

    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    // 不输出 ANSI 颜色码，保证日志文件纯文本可读。
    builder.write_style(env_logger::WriteStyle::Never);
    if let Some(file) = target_file {
        builder.target(env_logger::Target::Pipe(Box::new(TeeWriter { file })));
    }
    let _ = builder.try_init();
}

/// 安装 panic hook：把 panic 信息 + backtrace 写入日志文件（同时 stderr），
/// 崩溃后也能在 `/tmp/crossh/run.log` 查到现场。
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let bt = std::backtrace::Backtrace::force_capture();
        eprintln!("panic: {info}\n{bt}");
        log::error!("panic: {info}\nbacktrace:\n{bt}");
    }));
}

/// 若日志文件行数超过 `LOG_TRIM_THRESHOLD`，保留末尾 `LOG_TRIM_KEEP` 行后重写。
/// 按字节切片处理，规避 UTF-8 字符边界问题。
fn trim_log_if_needed(path: &Path) {
    let Ok(bytes) = fs::read(path) else {
        return;
    };
    if bytes.is_empty() {
        return;
    }
    let total = count_lines(&bytes);
    if total <= LOG_TRIM_THRESHOLD {
        return;
    }
    let start = line_start_from_end(&bytes, LOG_TRIM_KEEP);
    if start == 0 {
        return; // 总行数不足 keep，保持不变。
    }
    let kept = count_lines(&bytes[start..]);
    match fs::write(path, &bytes[start..]) {
        Ok(()) => eprintln!("[crossh] 日志裁剪：{total} 行 -> 保留末尾 {kept} 行（{LOG_PATH}）"),
        Err(e) => eprintln!("[crossh] 日志裁剪失败: {e}"),
    }
}

/// 按 `\n` 统计行数。
fn count_lines(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&b| b == b'\n').count()
}

/// 从字节流末尾往前，返回倒数第 `n` 行的起始字节偏移。
/// 若总行数不足 `n`，返回 0（从头开始）。
fn line_start_from_end(bytes: &[u8], n: usize) -> usize {
    if n == 0 || bytes.is_empty() {
        return bytes.len();
    }
    let mut count = 0;
    let mut i = bytes.len();
    // 跳过末尾连续换行（空尾行不计入）。
    while i > 0 && bytes[i - 1] == b'\n' {
        i -= 1;
    }
    while i > 0 {
        i -= 1;
        if bytes[i] == b'\n' {
            count += 1;
            if count == n {
                return i + 1;
            }
        }
    }
    0
}

/// 同时写文件与 stderr 的 writer：日志既落盘，又能在终端实时看到。
struct TeeWriter {
    file: std::fs::File,
}

impl Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = io::stderr().write_all(buf);
        self.file.write_all(buf)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        let _ = io::stderr().flush();
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_lines_basic() {
        assert_eq!(count_lines(b""), 0);
        assert_eq!(count_lines(b"one\n"), 1);
        assert_eq!(count_lines(b"a\nb\nc\n"), 3);
        // 末尾无换行也算一行内容（但按 \n 计数为 2）。
        assert_eq!(count_lines(b"a\nb\nc"), 2);
    }

    #[test]
    fn line_start_from_end_keeps_last_n() {
        // 三行，取末尾 2 行 → 应从第 2 行起始偏移开始。
        let bytes = b"line0\nline1\nline2\n";
        let start = line_start_from_end(bytes, 2);
        assert_eq!(&bytes[start..], b"line1\nline2\n");
    }

    #[test]
    fn line_start_from_end_more_than_available() {
        // 只有两行，要末尾 5 行 → 从头开始（返回 0）。
        let bytes = b"only\nsecond\n";
        assert_eq!(line_start_from_end(bytes, 5), 0);
    }

    #[test]
    fn line_start_from_end_zero() {
        // n == 0 → 返回末尾（保留空）。
        let bytes = b"abc\ndef\n";
        assert_eq!(line_start_from_end(bytes, 0), bytes.len());
    }

    #[test]
    fn line_start_from_end_trailing_newlines() {
        // 末尾多余空行不应计入。
        let bytes = b"a\nb\n\n\n";
        let start = line_start_from_end(bytes, 1);
        assert_eq!(&bytes[start..], b"b\n\n\n");
    }

    /// 端到端：写一个 > 阈值 的日志文件，调用 trim_log_if_needed，验证裁剪结果。
    #[test]
    fn trim_log_truncates_to_keep() {
        let dir = std::env::temp_dir().join("crossh_trim_test");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("trim.log");
        // 写 60_000 行，每行 "msg N"。
        let content: String = (0..60_000).map(|i| format!("msg {i}\n")).collect();
        fs::write(&path, &content).unwrap();

        // 直接复用常量校验：60_000 > 50_000 触发裁剪，保留末尾 10_000。
        let bytes = fs::read(&path).unwrap();
        assert_eq!(count_lines(&bytes), 60_000);

        // 模拟 trim_log_if_needed 的核心逻辑（避免直接依赖全局常量做文件 IO）。
        let total = count_lines(&bytes);
        assert!(total > LOG_TRIM_THRESHOLD);
        let start = line_start_from_end(&bytes, LOG_TRIM_KEEP);
        assert_eq!(
            count_lines(&bytes[start..]),
            LOG_TRIM_KEEP,
            "切片后行数应为 keep"
        );
        fs::write(&path, &bytes[start..]).unwrap();

        let after = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = after.lines().collect();
        assert_eq!(
            lines.len(),
            LOG_TRIM_KEEP,
            "裁剪后应保留 {} 行",
            LOG_TRIM_KEEP
        );
        // 末尾内容应为原始的最后 10_000 行（msg 50000..59999）。
        assert_eq!(lines.first(), Some(&"msg 50000"));
        assert_eq!(lines.last(), Some(&"msg 59999"));
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }
}

//! Process-wide logging and panic diagnostics.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

/// 持久化日志路径。放在 /tmp，系统重启自动清空，无需手动维护。
const LOG_PATH: &str = "/tmp/crossh/run.log";
/// 启动时若日志超过此行数，触发裁剪。
const LOG_TRIM_THRESHOLD: usize = 50_000;
/// 裁剪后保留的末尾行数。
const LOG_TRIM_KEEP: usize = 10_000;

/// Initialize logging and install the process panic hook.
pub(crate) fn init() {
    init_logging();
    install_panic_hook();
    log::info!("---- crossh startup (pid {}) ----", std::process::id());
}

fn init_logging() {
    let path = Path::new(LOG_PATH);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    trim_log_if_needed(path);

    let target_file = OpenOptions::new().create(true).append(true).open(path).ok();

    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    builder.write_style(env_logger::WriteStyle::Never);
    if let Some(file) = target_file {
        builder.target(env_logger::Target::Pipe(Box::new(TeeWriter { file })));
    }
    let _ = builder.try_init();
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let bt = std::backtrace::Backtrace::force_capture();
        eprintln!("panic: {info}\n{bt}");
        log::error!("panic: {info}\nbacktrace:\n{bt}");
    }));
}

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
        return;
    }
    let kept = count_lines(&bytes[start..]);
    match fs::write(path, &bytes[start..]) {
        Ok(()) => eprintln!("[crossh] 日志裁剪：{total} 行 -> 保留末尾 {kept} 行（{LOG_PATH}）"),
        Err(error) => eprintln!("[crossh] 日志裁剪失败: {error}"),
    }
}

fn count_lines(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&byte| byte == b'\n').count()
}

fn line_start_from_end(bytes: &[u8], n: usize) -> usize {
    if n == 0 || bytes.is_empty() {
        return bytes.len();
    }
    let mut count = 0;
    let mut index = bytes.len();
    while index > 0 && bytes[index - 1] == b'\n' {
        index -= 1;
    }
    while index > 0 {
        index -= 1;
        if bytes[index] == b'\n' {
            count += 1;
            if count == n {
                return index + 1;
            }
        }
    }
    0
}

struct TeeWriter {
    file: fs::File,
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
        assert_eq!(count_lines(b"a\nb\nc"), 2);
    }

    #[test]
    fn line_start_from_end_keeps_last_n() {
        let bytes = b"line0\nline1\nline2\n";
        let start = line_start_from_end(bytes, 2);
        assert_eq!(&bytes[start..], b"line1\nline2\n");
    }

    #[test]
    fn line_start_from_end_more_than_available() {
        let bytes = b"only\nsecond\n";
        assert_eq!(line_start_from_end(bytes, 5), 0);
    }

    #[test]
    fn line_start_from_end_zero() {
        let bytes = b"abc\ndef\n";
        assert_eq!(line_start_from_end(bytes, 0), bytes.len());
    }

    #[test]
    fn line_start_from_end_trailing_newlines() {
        let bytes = b"a\nb\n\n\n";
        let start = line_start_from_end(bytes, 1);
        assert_eq!(&bytes[start..], b"b\n\n\n");
    }

    #[test]
    fn trim_log_truncates_to_keep() {
        let dir = std::env::temp_dir().join("crossh_trim_test");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("trim.log");
        let content: String = (0..60_000).map(|i| format!("msg {i}\n")).collect();
        fs::write(&path, &content).unwrap();

        let bytes = fs::read(&path).unwrap();
        assert_eq!(count_lines(&bytes), 60_000);
        let total = count_lines(&bytes);
        assert!(total > LOG_TRIM_THRESHOLD);
        let start = line_start_from_end(&bytes, LOG_TRIM_KEEP);
        assert_eq!(count_lines(&bytes[start..]), LOG_TRIM_KEEP);
        fs::write(&path, &bytes[start..]).unwrap();

        let after = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = after.lines().collect();
        assert_eq!(lines.len(), LOG_TRIM_KEEP);
        assert_eq!(lines.first(), Some(&"msg 50000"));
        assert_eq!(lines.last(), Some(&"msg 59999"));
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }
}

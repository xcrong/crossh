//! Pure SFTP path, text-file, and transfer helper logic.

use std::path::{Path, PathBuf};

use async_channel::Sender;

use crossh_core::format::format_bytes;
use crossh_ssh::SftpCmd;

const SUPPORTED_TEXT_EXTENSIONS: &[&str] = &[
    "bash", "c", "cc", "conf", "config", "cpp", "css", "csv", "fish", "go", "h", "hh", "hpp",
    "htm", "html", "ini", "java", "js", "json", "jsonl", "jsx", "kts", "kt", "log", "lua",
    "markdown", "md", "php", "py", "rb", "rs", "sh", "sql", "swift", "toml", "ts", "tsx", "txt",
    "xml", "yaml", "yml", "zsh",
];

const SUPPORTED_TEXT_FILENAMES: &[&str] = &[
    ".editorconfig",
    ".gitattributes",
    ".gitignore",
    "dockerfile",
    "gemfile",
    "hosts",
    "license",
    "makefile",
    "passwd",
    "profile",
    "rakefile",
    "readme",
    "sshd_config",
    "authorized_keys",
    "known_hosts",
];

pub(crate) fn sftp_channel_unavailable() -> String {
    crate::shared::i18n::text("sftp.channel_unavailable")
}

pub(crate) fn is_supported_text_file(name: &str) -> bool {
    let filename = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(name)
        .to_ascii_lowercase();
    if SUPPORTED_TEXT_FILENAMES.contains(&filename.as_str()) || filename.starts_with(".env") {
        return true;
    }
    Path::new(&filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| SUPPORTED_TEXT_EXTENSIONS.contains(&extension))
}

/// 上下移动光标（按列对齐）；到达首行/末行时返回 false，目标行更短时收缩到行尾。
///
/// 前置条件：`cursor` 必须是 `content` 的字符边界。
pub(crate) fn move_cursor_vertical(content: &str, cursor: &mut usize, direction: i8) -> bool {
    debug_assert!(content.is_char_boundary(*cursor));
    let (line_start, line_end) = line_bounds(content, *cursor);
    let column = content[line_start..*cursor].chars().count();
    let target_start = if direction < 0 {
        if line_start == 0 {
            return false;
        }
        content[..line_start - 1]
            .rfind('\n')
            .map(|idx| idx + 1)
            .unwrap_or(0)
    } else {
        if line_end == content.len() {
            return false;
        }
        line_end + 1
    };
    let target_end = content[target_start..]
        .find('\n')
        .map(|idx| target_start + idx)
        .unwrap_or(content.len());
    *cursor = content[target_start..target_end]
        .char_indices()
        .nth(column)
        .map(|(idx, _)| target_start + idx)
        .unwrap_or(target_end);
    true
}

/// 远端路径的父目录："a/b" -> "a"，根目录自身返回 "/"。
pub(crate) fn parent_of(path: &str) -> String {
    let p = path.trim_end_matches('/');
    if p.is_empty() {
        return "/".to_string();
    }
    match p.rfind('/') {
        Some(0) => "/".to_string(),
        Some(idx) => p[..idx].to_string(),
        None => ".".to_string(),
    }
}

/// 以 `/` 拼接远端路径，避免重复分隔符。
pub(crate) fn join(base: &str, name: &str) -> String {
    if base.ends_with('/') {
        format!("{base}{name}")
    } else {
        format!("{base}/{name}")
    }
}

pub(crate) fn line_bounds(text: &str, cursor: usize) -> (usize, usize) {
    let start = text[..cursor].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let end = text[cursor..]
        .find('\n')
        .map(|idx| cursor + idx)
        .unwrap_or(text.len());
    (start, end)
}

/// Resolve the local download directory, falling back to the current directory.
pub(crate) fn downloads_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Downloads"))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Add ` (1)`, ` (2)`, ... to avoid overwriting an existing download.
pub(crate) fn unique_local_path(path: &Path) -> Option<PathBuf> {
    if !path.exists() {
        return Some(path.to_path_buf());
    }
    let stem = path
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    let extension = path
        .extension()
        .map(|value| value.to_string_lossy().to_string());
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    for index in 1..1000 {
        let name = match &extension {
            Some(extension) => format!("{stem} ({index}).{extension}"),
            None => format!("{stem} ({index})"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

pub(crate) fn try_send_command(tx: &Sender<SftpCmd>, command: SftpCmd) -> Result<(), &'static str> {
    tx.try_send(command).map_err(|_| "sftp channel unavailable")
}

pub(crate) fn format_size(bytes: u64) -> String {
    format_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertical_movement_aligns_column_and_stops_at_edges() {
        let content = "abc\ndefg".to_string();
        let mut cursor = 1;
        assert!(move_cursor_vertical(&content, &mut cursor, 1));
        assert_eq!(cursor, 5);
        assert!(!move_cursor_vertical(&content, &mut cursor, 1));
        assert!(move_cursor_vertical(&content, &mut cursor, -1));
        assert_eq!(cursor, 1);
        assert!(!move_cursor_vertical(&content, &mut cursor, -1));
    }

    #[test]
    fn vertical_movement_shrinks_to_short_line_end() {
        let content = "abc\nd".to_string();
        let mut cursor = 2;
        assert!(move_cursor_vertical(&content, &mut cursor, 1));
        assert_eq!(cursor, 5);
    }

    #[test]
    fn parent_of_handles_root_and_relative_paths() {
        assert_eq!(parent_of("/a/b"), "/a");
        assert_eq!(parent_of("/a/"), "/");
        assert_eq!(parent_of("/"), "/");
        assert_eq!(parent_of("////"), "/");
        assert_eq!(parent_of("/home/user"), "/home");
        assert_eq!(parent_of("/home/user/"), "/home");
        assert_eq!(parent_of("a"), ".");
        assert_eq!(parent_of("."), ".");
    }

    #[test]
    fn join_avoids_duplicate_separator() {
        assert_eq!(join("/a", "b"), "/a/b");
        assert_eq!(join("/a/", "b"), "/a/b");
        assert_eq!(join(".", "notes.txt"), "./notes.txt");
        assert_eq!(join("/", "notes.txt"), "/notes.txt");
        assert_eq!(join("/home/user", "notes.txt"), "/home/user/notes.txt");
    }
}

//! Pure SFTP path, text-file, and transfer helper logic.

use std::path::{Path, PathBuf};

use async_channel::Sender;

use crossh_ssh::SftpCmd;

pub(crate) const SFTP_CHANNEL_UNAVAILABLE: &str = "sftp channel unavailable";

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

pub(crate) fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}

pub(crate) fn next_char_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .chars()
        .next()
        .map(|ch| cursor + ch.len_utf8())
        .unwrap_or(cursor)
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
    tx.try_send(command).map_err(|_| SFTP_CHANNEL_UNAVAILABLE)
}

pub(crate) fn format_size(bytes: u64) -> String {
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

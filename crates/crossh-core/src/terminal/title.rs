use std::{env, path::Path};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::session::TerminalProcessInfo;

/// Matches Zed's per-title-component limit for compact tab labels (display width in columns).
pub const MAX_TITLE_COMPONENT_CHARS: usize = 25;

/// Truncate at a Unicode character boundary using the same trailing ellipsis
/// convention as Zed's terminal title helper, but measured by display width
/// (`unicode-width`) so CJK and emoji (width 2) do not overflow the tab.
pub fn truncate_title(value: &str) -> String {
    if value.width() <= MAX_TITLE_COMPONENT_CHARS {
        return value.to_owned();
    }

    let ellipsis_width = "…".width();
    let content_limit = MAX_TITLE_COMPONENT_CHARS.saturating_sub(ellipsis_width);
    let mut width = 0;
    let mut end = 0;
    for (idx, ch) in value.char_indices() {
        let w = ch.width().unwrap_or(0);
        if width + w > content_limit {
            break;
        }
        width += w;
        end = idx + ch.len_utf8();
    }
    format!("{}…", &value[..end])
}

/// Build the local terminal title in the same shape as Zed:
/// working-directory — foreground-process.
pub fn local_terminal_title(
    cwd: Option<&str>,
    process: Option<&TerminalProcessInfo>,
    fallback_process: Option<&str>,
) -> String {
    let process_cwd = process
        .and_then(|info| info.cwd.as_deref())
        .filter(|cwd| !cwd.trim().is_empty());
    let directory = process_cwd
        .or_else(|| cwd.filter(|cwd| !cwd.trim().is_empty()))
        .map(|cwd| path_display_name(Path::new(cwd)))
        .unwrap_or_default();

    let process_name = process
        .map(process_display_name)
        .filter(|name| !name.is_empty())
        .or_else(|| fallback_process.map(|path| path_display_name(Path::new(path))))
        .unwrap_or_default();

    let directory = truncate_title(&directory);
    let process_name = truncate_title(&process_name);
    match (directory.is_empty(), process_name.is_empty()) {
        (false, false) => format!("{directory} — {process_name}"),
        (false, true) => directory,
        (true, false) => process_name,
        (true, true) => "Terminal".to_owned(),
    }
}

/// Prefer a shell-provided title, then use a host-independent terminal label.
pub fn remote_terminal_title(title: Option<&str>) -> String {
    title
        .filter(|title| !title.trim().is_empty())
        .map(truncate_path_title)
        .unwrap_or_else(|| "Terminal".to_owned())
}

/// Normalize a local shell path before compacting it for a tab.
///
/// Shell title hooks can abbreviate an absolute path inconsistently, so use
/// the terminal's OSC 7 cwd as the canonical path when the title is path-like.
/// This also turns the local home directory into `~`.
pub fn local_terminal_tab_title(title: &str, cwd: Option<&str>) -> String {
    let home = env::var_os("HOME")
        .map(|home| home.to_string_lossy().into_owned())
        .or_else(|| dirs::home_dir().map(|path| path.to_string_lossy().into_owned()));
    local_terminal_tab_title_for_home(title, cwd, home.as_deref())
}

/// Keep remote pane tabs scoped to the selected host in the sidebar.
pub fn remote_pane_title(pane_type: &str) -> String {
    truncate_title(pane_type)
}

/// Remove the host prefix emitted by common shell title hooks, such as
/// `user@host: ~/project`, while leaving arbitrary application titles intact.
pub fn strip_shell_host_prefix(value: &str) -> &str {
    let Some((prefix, path)) = [": ", ":~", ":/", ":\\"].iter().find_map(|separator| {
        value.find(separator).map(|index| {
            let path_start = index + separator.len();
            (&value[..index], &value[path_start..])
        })
    }) else {
        return value;
    };
    let Some((user, host)) = prefix.rsplit_once('@') else {
        return value;
    };
    if user.is_empty()
        || host.is_empty()
        || !user.chars().all(is_shell_host_component)
        || !host.chars().all(is_shell_host_component)
    {
        return value;
    }

    if path.trim().is_empty() || !is_path_like(path) {
        value
    } else {
        path
    }
}

fn is_shell_host_component(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
}

fn local_terminal_tab_title_for_home(title: &str, cwd: Option<&str>, home: Option<&str>) -> String {
    let path = if is_path_like(title) {
        cwd.filter(|cwd| is_path_like(cwd)).unwrap_or(title)
    } else {
        title
    };
    let path = home
        .map(|home| path_relative_to_home(path, home))
        .unwrap_or_else(|| path.to_owned());
    truncate_path_title(&path)
}

fn path_relative_to_home(value: &str, home: &str) -> String {
    let home = home.trim_end_matches(['/', '\\']);
    if value == home {
        return "~".to_owned();
    }
    let Some(rest) = value.strip_prefix(home) else {
        return value.to_owned();
    };
    if matches!(rest.chars().next(), Some('/') | Some('\\')) {
        format!("~{rest}")
    } else {
        value.to_owned()
    }
}

/// Compress path parents like Fish's prompt_pwd, keeping the final two
/// components intact because they carry the most useful context in a tab.
pub fn truncate_path_title(value: &str) -> String {
    if !is_path_like(value) {
        return truncate_title(value);
    }

    let shortened = shorten_path_components(value);
    if shortened.width() <= MAX_TITLE_COMPONENT_CHARS {
        return shortened;
    }

    let prefix = if shortened.contains('/') {
        "…/"
    } else {
        "…"
    };
    let prefix_width = prefix.width();
    let available = MAX_TITLE_COMPONENT_CHARS.saturating_sub(prefix_width);
    let mut width = 0;
    let mut start = shortened.len();
    for (idx, ch) in shortened.char_indices().rev() {
        let w = ch.width().unwrap_or(0);
        if width + w > available {
            break;
        }
        width += w;
        start = idx;
    }
    format!("{}{}", prefix, &shortened[start..])
}

fn is_path_like(value: &str) -> bool {
    value.starts_with("~/")
        || value.starts_with('/')
        || value.starts_with('\\')
        || value.starts_with("./")
        || value.starts_with("../")
        || value
            .as_bytes()
            .get(1..3)
            .is_some_and(|drive| drive[0] == b':' && matches!(drive[1], b'/' | b'\\'))
}

fn shorten_path_components(value: &str) -> String {
    let separator = if value.contains('\\') && !value.contains('/') {
        '\\'
    } else {
        '/'
    };
    let (prefix, rest) = if let Some(rest) = value.strip_prefix("~/") {
        ("~/", rest)
    } else if let Some(rest) = value.strip_prefix('/') {
        ("/", rest)
    } else if value.len() >= 3
        && value.as_bytes()[1] == b':'
        && value.as_bytes()[2] == separator as u8
    {
        (&value[..3], &value[3..])
    } else {
        ("", value)
    };
    let components = rest
        .split(separator)
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    if components.len() <= 2 {
        return value.to_owned();
    }

    let mut shortened = prefix.to_owned();
    for (index, component) in components.iter().enumerate() {
        if index + 2 < components.len() {
            shortened.extend(component.chars().next());
        } else {
            shortened.push_str(component);
        }
        if index + 1 < components.len() {
            shortened.push(separator);
        }
    }
    shortened
}

/// 提取路径最后一个分量作为显示名;无可用文件名(根目录、`..` 等)时回退为完整路径。
pub fn path_display_name(path: &Path) -> String {
    path.file_name()
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn process_display_name(process: &TerminalProcessInfo) -> String {
    process.name.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_title_matches_zed_shape() {
        let process = TerminalProcessInfo {
            name: "vim".into(),
            cwd: Some("/work/crossh".into()),
        };

        assert_eq!(
            local_terminal_title(Some("/tmp"), Some(&process), Some("zsh")),
            "crossh — vim"
        );
    }

    #[test]
    fn local_title_falls_back_to_shell_and_cwd() {
        assert_eq!(
            local_terminal_title(Some("/Users/me/project"), None, Some("/bin/zsh")),
            "project — zsh"
        );
    }

    #[test]
    fn title_components_are_truncated_at_unicode_boundaries() {
        let directory = "目录".repeat(20);
        let title = local_terminal_title(Some(&directory), None, Some("zsh"));
        // “目录” 每个字符宽度为 2，MAX 为列宽 25，内容可用 24 列 → 12 个字符（6 次重复）+ “…”
        let expected_directory = format!("{}…", "目录".repeat(6));

        assert_eq!(title, format!("{expected_directory} — zsh"));
        assert_eq!(truncate_title(&"x".repeat(26)), "x".repeat(24) + "…");
    }

    #[test]
    fn remote_titles_do_not_use_host_labels() {
        assert_eq!(
            remote_terminal_title(Some("very-long-remote-terminal-title-for-prod")),
            "very-long-remote-termina…"
        );
        assert_eq!(remote_terminal_title(None), "Terminal");
        assert_eq!(remote_pane_title("SFTP"), "SFTP");
        assert_eq!(remote_pane_title("Forward"), "Forward");
    }

    #[test]
    fn path_titles_keep_the_tail_visible() {
        assert_eq!(
            truncate_path_title("~/edunest/miniapp/src/static/tabbar"),
            "~/e/m/s/static/tabbar"
        );
        assert_eq!(
            truncate_path_title("~/very-long-parent/another-long-parent/this-is-a-long-folder"),
            "…/t/this-is-a-long-folder"
        );
    }

    #[test]
    fn local_path_titles_use_cwd_and_home() {
        assert_eq!(
            local_terminal_tab_title_for_home(
                "/Code/crossh",
                Some("/Users/xiancongrong/Code/crossh"),
                Some("/Users/xiancongrong"),
            ),
            "~/Code/crossh"
        );
        assert_eq!(
            local_terminal_tab_title_for_home(
                "/Users/xiancongrong/Code/crossh",
                None,
                Some("/Users/xiancongrong"),
            ),
            "~/Code/crossh"
        );
    }

    #[test]
    fn path_display_name_extracts_the_final_component() {
        assert_eq!(path_display_name(Path::new("proj.txt")), "proj.txt");
        assert_eq!(path_display_name(Path::new("/a/b.txt")), "b.txt");
        assert_eq!(path_display_name(Path::new("/a/b/")), "b");
        assert_eq!(
            path_display_name(Path::new("C:/Users/me/proj.txt")),
            "proj.txt"
        );
    }

    #[test]
    fn path_display_name_falls_back_when_there_is_no_file_name() {
        assert_eq!(path_display_name(Path::new("/")), "/");
        assert_eq!(path_display_name(Path::new(".")), ".");
        assert_eq!(path_display_name(Path::new("..")), "..");
    }

    #[test]
    fn shell_host_prefixes_are_removed_without_touching_app_titles() {
        assert_eq!(
            strip_shell_host_prefix("xiancongorng@macAir: ~/xxxx"),
            "~/xxxx"
        );
        assert_eq!(strip_shell_host_prefix("ubuntu@VM-x-x: ~/xxx"), "~/xxx");
        assert_eq!(strip_shell_host_prefix(r"alice@host: \tmp"), r"\tmp");
        assert_eq!(strip_shell_host_prefix("OpenCode"), "OpenCode");
        assert_eq!(
            strip_shell_host_prefix("alice@host: build"),
            "alice@host: build"
        );
    }
}

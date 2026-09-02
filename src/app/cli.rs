//! 顶层 CLI 解析与子进程委托。
//!
//! `main.rs` 保持装配职责（日志、运行时、GPUI 启动），可测试的业务逻辑
//! 下沉到此模块：顶层参数分发与帮助文本。

use std::path::PathBuf;

use crate::features::{
    git_launcher::{self, GitCliCommand},
    note_launcher::{self, NoteCliCommand},
};

/// 顶层 CLI 分发结果，由 `main` 负责副作用（打印、退出、启动窗口）。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CliCommand {
    Help,
    Version,
    Git(Result<GitCliCommand, String>),
    Note(Result<NoteCliCommand, String>),
    Unknown(String),
    Main,
}

/// 解析顶层参数。
///
/// `args` 为 `std::env::args().skip(1)` 的迭代器；`current_dir` 仅在
/// `crossh git` 分支需要，用于相对路径解析，与 `git_launcher::parse_cli`
/// 保持一致以便可测试。
pub(crate) fn parse_cli(
    mut args: impl Iterator<Item = String>,
    current_dir: Result<PathBuf, String>,
) -> CliCommand {
    match args.next().as_deref() {
        None => CliCommand::Main,
        Some("--help" | "-h" | "help") => CliCommand::Help,
        Some("--version" | "-V") => CliCommand::Version,
        Some("git") => {
            let result = git_launcher::parse_cli(args, current_dir);
            CliCommand::Git(result)
        }
        Some("note") => {
            let result = note_launcher::parse_cli(args);
            CliCommand::Note(result)
        }
        Some(argument) => CliCommand::Unknown(argument.to_string()),
    }
}

/// 返回与 `print_help` 打印内容一致的帮助文本，便于测试与复用。
pub(crate) fn help_text() -> String {
    format!(
        "Crossh {} — Local-first terminal workspace (GPUI)\n\nUsage: crossh [COMMAND]\n\nLocal-first terminal workspace — manage projects, terminals and notes locally.\n\nCommands:\n  git         Open the Git Viewer for a directory\n  note        Open the Note Viewer\n  help        Print help\n\nOptions:\n  -h, --help     Print help\n  -V, --version  Print version",
        env!("CARGO_PKG_VERSION")
    )
}

/// 打印顶层帮助到 stdout。
pub(crate) fn print_help() {
    println!("{}", help_text());
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{CliCommand, help_text, parse_cli};
    use crate::features::git_launcher::GitCliCommand;

    fn cwd_ok(path: &str) -> Result<PathBuf, String> {
        Ok(PathBuf::from(path))
    }

    #[test]
    fn no_arguments_maps_to_main() {
        let command = parse_cli(std::iter::empty(), cwd_ok("/repo"));
        assert_eq!(command, CliCommand::Main);
    }

    #[test]
    fn help_flags_map_to_help() {
        for flag in ["--help", "-h", "help"] {
            let command = parse_cli([flag].into_iter().map(str::to_string), cwd_ok("/repo"));
            assert_eq!(command, CliCommand::Help, "flag: {flag}");
        }
    }

    #[test]
    fn version_flags_map_to_version() {
        for flag in ["--version", "-V"] {
            let command = parse_cli([flag].into_iter().map(str::to_string), cwd_ok("/repo"));
            assert_eq!(command, CliCommand::Version, "flag: {flag}");
        }
    }

    #[test]
    fn git_delegates_to_git_launcher() {
        let command = parse_cli(["git"].into_iter().map(str::to_string), cwd_ok("/repo"));
        assert_eq!(
            command,
            CliCommand::Git(Ok(GitCliCommand::Open(PathBuf::from("/repo"))))
        );

        let command = parse_cli(
            ["git", "--help"].into_iter().map(str::to_string),
            cwd_ok("/repo"),
        );
        assert_eq!(command, CliCommand::Git(Ok(GitCliCommand::Help)));

        let command = parse_cli(
            ["git", "first", "second"].into_iter().map(str::to_string),
            cwd_ok("/repo"),
        );
        assert!(matches!(command, CliCommand::Git(Err(_))));
    }

    #[test]
    fn git_relative_path_uses_current_dir() {
        let command = parse_cli(
            ["git", "other"].into_iter().map(str::to_string),
            cwd_ok("/repo"),
        );
        assert_eq!(
            command,
            CliCommand::Git(Ok(GitCliCommand::Open(PathBuf::from("/repo/other"))))
        );
    }

    #[test]
    fn note_delegates_to_note_launcher() {
        use crate::features::note_launcher::NoteCliCommand;

        let command = parse_cli(["note"].into_iter().map(str::to_string), cwd_ok("/repo"));
        assert_eq!(command, CliCommand::Note(Ok(NoteCliCommand::Open)));

        let command = parse_cli(
            ["note", "--help"].into_iter().map(str::to_string),
            cwd_ok("/repo"),
        );
        assert_eq!(command, CliCommand::Note(Ok(NoteCliCommand::Help)));

        let command = parse_cli(
            ["note", "extra"].into_iter().map(str::to_string),
            cwd_ok("/repo"),
        );
        assert!(matches!(command, CliCommand::Note(Err(_))));
    }

    #[test]
    fn unknown_argument_is_preserved() {
        let command = parse_cli(["unknown"].into_iter().map(str::to_string), cwd_ok("/repo"));
        assert_eq!(command, CliCommand::Unknown("unknown".to_string()));
    }

    #[test]
    fn help_text_contains_version_and_commands() {
        let text = help_text();
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
        assert!(text.contains("Local-first terminal workspace"));
        assert!(text.contains("Usage: crossh [COMMAND]"));
        assert!(text.contains("git"));
        assert!(text.contains("note"));
        assert!(text.contains("--help"));
        assert!(text.contains("--version"));
    }
}

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
    /// 启动主窗口。`path` 为原始参数（相对路径不解析），由 `main`
    /// 结合进程工作目录校验为项目目录；`None` 为裸启动。
    Main {
        path: Option<PathBuf>,
    },
}

/// 解析顶层参数。
///
/// `args` 为 `std::env::args().skip(1)` 的迭代器；`current_dir` 仅在
/// `crossh git` 分支需要，用于相对路径解析，与 `git_launcher::parse_cli`
/// 保持一致以便可测试。主窗口的 `[PATH]` 参数保持原样透传，不在此
/// 触碰文件系统，存在性校验归 `main`（见 `single_instance::resolve_open_path`）。
pub(crate) fn parse_cli(
    mut args: impl Iterator<Item = String>,
    current_dir: Result<PathBuf, String>,
) -> CliCommand {
    match args.next().as_deref() {
        None => CliCommand::Main { path: None },
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
        Some("--") => match args.next() {
            None => CliCommand::Main { path: None },
            Some(path) if args.next().is_none() => CliCommand::Main {
                path: Some(PathBuf::from(path)),
            },
            Some(argument) => CliCommand::Unknown(argument),
        },
        // 首个 `-` 开头参数不在已知集合中：保持未知参数行为。
        Some(argument) if argument.starts_with('-') => CliCommand::Unknown(argument.to_string()),
        Some(argument) => match args.next() {
            None => CliCommand::Main {
                path: Some(PathBuf::from(argument)),
            },
            // 与 `git_launcher` 一致：多余参数时报错首个参数。
            Some(_) => CliCommand::Unknown(argument.to_string()),
        },
    }
}

/// 返回与 `print_help` 打印内容一致的帮助文本，便于测试与复用。
pub(crate) fn help_text() -> String {
    format!(
        "Crossh {} — Local-first terminal workspace (GPUI)\n\nUsage: crossh [COMMAND]\n       crossh [PATH]\n\nLocal-first terminal workspace — manage projects, terminals and notes locally.\n\nArguments:\n  [PATH]      Open a project directory (reuses the running instance when present,\n              otherwise starts a new instance).\n              To open a directory named \"git\"/\"note\"/\"help\", use:\n              crossh ./git  or:  crossh -- git\n\nCommands:\n  git         Open the Git Viewer for a directory\n  note        Open the Note Viewer\n  help        Print help\n\nOptions:\n  -h, --help     Print help\n  -V, --version  Print version",
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
    fn no_arguments_maps_to_main_without_path() {
        let command = parse_cli(std::iter::empty(), cwd_ok("/repo"));
        assert_eq!(command, CliCommand::Main { path: None });
    }

    #[test]
    fn single_path_argument_maps_to_main_with_raw_path() {
        // 相对路径保持原样透传，不在此 join，存在性校验归 main。
        let command = parse_cli(["other"].into_iter().map(str::to_string), cwd_ok("/repo"));
        assert_eq!(
            command,
            CliCommand::Main {
                path: Some(PathBuf::from("other"))
            }
        );

        let command = parse_cli(
            ["/repo/other"].into_iter().map(str::to_string),
            cwd_ok("/repo"),
        );
        assert_eq!(
            command,
            CliCommand::Main {
                path: Some(PathBuf::from("/repo/other"))
            }
        );
    }

    #[test]
    fn dash_separator_allows_dash_prefixed_paths() {
        let command = parse_cli(
            ["--", "-weird"].into_iter().map(str::to_string),
            cwd_ok("/repo"),
        );
        assert_eq!(
            command,
            CliCommand::Main {
                path: Some(PathBuf::from("-weird"))
            }
        );

        let command = parse_cli(["--"].into_iter().map(str::to_string), cwd_ok("/repo"));
        assert_eq!(command, CliCommand::Main { path: None });
    }

    #[test]
    fn shadowed_names_need_dot_slash_or_separator() {
        // 与子命令同名的目录必须转义，裸写永远进对应子命令。
        for name in ["git", "note", "help"] {
            let shadowed = parse_cli([name].into_iter().map(str::to_string), cwd_ok("/repo"));
            assert!(
                !matches!(shadowed, CliCommand::Main { .. }),
                "bare {name} must not open a directory"
            );

            let command = parse_cli([format!("./{name}")].into_iter(), cwd_ok("/repo"));
            assert_eq!(
                command,
                CliCommand::Main {
                    path: Some(PathBuf::from(format!("./{name}")))
                }
            );

            let command = parse_cli(
                ["--", name].into_iter().map(str::to_string),
                cwd_ok("/repo"),
            );
            assert_eq!(
                command,
                CliCommand::Main {
                    path: Some(PathBuf::from(name))
                }
            );
        }
    }

    #[test]
    fn extra_positional_argument_reports_the_first() {
        let command = parse_cli(
            ["first", "second"].into_iter().map(str::to_string),
            cwd_ok("/repo"),
        );
        assert_eq!(command, CliCommand::Unknown("first".to_string()));
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
    fn unknown_dash_argument_is_preserved() {
        let command = parse_cli(
            ["--unknown"].into_iter().map(str::to_string),
            cwd_ok("/repo"),
        );
        assert_eq!(command, CliCommand::Unknown("--unknown".to_string()));
    }

    #[test]
    fn help_text_contains_version_and_commands() {
        let text = help_text();
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
        assert!(text.contains("Local-first terminal workspace"));
        assert!(text.contains("Usage: crossh [COMMAND]"));
        assert!(text.contains("crossh [PATH]"));
        assert!(text.contains("git"));
        assert!(text.contains("note"));
        assert!(text.contains("--help"));
        assert!(text.contains("crossh ./git"));
    }
}

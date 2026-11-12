//! 顶层 CLI 解析与子进程委托。
//!
//! `main.rs` 保持装配职责（日志、运行时、GPUI 启动），可测试的业务逻辑
//! 下沉到此模块：顶层参数分发与帮助文本。

use std::path::PathBuf;

use crossh_core::{
    git_launcher::{self, GitCliCommand},
    note_launcher::{self, NoteCliCommand},
};

/// `daemon` 子命令动作。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DaemonAction {
    Status,
    Start,
    Stop,
}

/// 顶层 CLI 分发结果，由 `main` 负责副作用（打印、退出、启动窗口）。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CliCommand {
    Help,
    Version,
    Git(Result<GitCliCommand, String>),
    Note(Result<NoteCliCommand, String>),
    Unknown(String),
    /// 会话属主管理：`daemon [status|start|stop]`（缺省 `status`）。
    Daemon(DaemonAction),
    /// 列出 daemon 上的会话（含输入权角色）。
    Ls,
    /// 在新会话中运行一次性命令，转发输出并原样返回退出码。
    Run(Vec<String>),
    /// 输出指定会话的回放字节。
    Capture(String),
    /// 启动主窗口。`path` 为原始参数（相对路径不解析），由 `main`
    /// 结合进程工作目录校验为项目目录；`None` 为裸启动。
    Main {
        path: Option<PathBuf>,
    },
}

/// `args` 为 `std::env::args().skip(1)` 的迭代器；`current_dir` 仅在
/// `crossh git` 分支需要，用于相对路径解析与 `git_launcher::parse_cli`
/// 保持一致以便可测试。开目录唯一入口是 `crossh open <path>`原样透传，
/// 不在此触碰文件系统，存在性校验归 `main`（见 `single_instance::resolve_open_path`）。
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
        Some("daemon") => match args.next().as_deref() {
            None | Some("status") => CliCommand::Daemon(DaemonAction::Status),
            Some("start") => CliCommand::Daemon(DaemonAction::Start),
            Some("stop") => CliCommand::Daemon(DaemonAction::Stop),
            Some(other) => CliCommand::Unknown(other.to_string()),
        },
        Some("ls") => match args.next() {
            None => CliCommand::Ls,
            Some(extra) => CliCommand::Unknown(extra),
        },
        Some("run") => {
            let mut rest: Vec<String> = args.collect();
            if rest.first().is_some_and(|s| s == "--") {
                rest.remove(0);
            }
            CliCommand::Run(rest)
        }
        Some("capture") => match args.next() {
            Some(id) => CliCommand::Capture(id),
            None => CliCommand::Unknown("capture".to_string()),
        },
        // 开目录唯一入口：`crossh open <path>`（与子命令同名也无需转义）。
        // 缺参数/多余参数都回到未知参数，与其它子命令一致。
        Some("open") => match args.next() {
            Some(path) if args.next().is_none() => CliCommand::Main {
                path: Some(PathBuf::from(path)),
            },
            _ => CliCommand::Unknown("open".to_string()),
        },
        // 首个 `-` 开头参数不在已知集合中：保持未知参数行为。
        Some(argument) if argument.starts_with('-') => CliCommand::Unknown(argument.to_string()),
        // 裸路径已取消：开目录必须显式 `crossh open <path>`。
        Some(argument) => CliCommand::Unknown(argument.to_string()),
    }
}

/// 返回与 `print_help` 打印内容一致的帮助文本，便于测试与复用。
pub(crate) fn help_text() -> String {
    format!(
        "Crossh {} — Local-first terminal workspace (GPUI)\n\nUsage: crossh [COMMAND]\n       crossh open <path>\n\nLocal-first terminal workspace — manage projects, terminals and notes locally.\n\nCommands:\n  open        Open a project directory (reuses the running instance when present,\n              otherwise starts a new instance).\n              Directories named like subcommands need no escaping: crossh open git\n  git         Open the Git Viewer for a directory\n  note        Open the Note Viewer\n  daemon      Manage the session daemon (status|start|stop)\n  ls          List daemon sessions (second frontend needs no GUI)\n  run         Run a command in a new session, stream output, return its exit code\n  capture     Print a session's replay buffer\n  help        Print help\n\nOptions:\n  -h, --help     Print help\n  -V, --version  Print version",
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
    use crossh_core::git_launcher::GitCliCommand;

    fn cwd_ok(path: &str) -> Result<PathBuf, String> {
        Ok(PathBuf::from(path))
    }

    #[test]
    fn no_arguments_maps_to_main_without_path() {
        let command = parse_cli(std::iter::empty(), cwd_ok("/repo"));
        assert_eq!(command, CliCommand::Main { path: None });
    }

    #[test]
    fn open_takes_exactly_one_raw_path() {
        // 相对路径保持原样透传，不在此 join，存在性校验归 main。
        // `open` 后直接跟路径：`-` 开头也不再需要 `--` 转义。
        for path in ["other", "/repo/other", "-weird", "git"] {
            let command = parse_cli(
                ["open", path].into_iter().map(str::to_string),
                cwd_ok("/repo"),
            );
            assert_eq!(
                command,
                CliCommand::Main {
                    path: Some(PathBuf::from(path))
                },
                "path: {path}"
            );
        }
        // 缺参数/多余参数都回到未知参数。
        let command = parse_cli(["open"].into_iter().map(str::to_string), cwd_ok("/repo"));
        assert_eq!(command, CliCommand::Unknown("open".to_string()));
        let command = parse_cli(
            ["open", "a", "b"].into_iter().map(str::to_string),
            cwd_ok("/repo"),
        );
        assert_eq!(command, CliCommand::Unknown("open".to_string()));
    }

    #[test]
    fn bare_paths_and_separator_are_rejected() {
        // 裸路径已取消：开目录必须显式 `crossh open <path>`。
        for args in [
            vec!["other"],
            vec!["/repo/other"],
            vec!["./git"],
            vec!["--", "-weird"],
            vec!["--"],
        ] {
            let command = parse_cli(args.into_iter().map(str::to_string), cwd_ok("/repo"));
            assert!(
                matches!(command, CliCommand::Unknown(_)),
                "bare path must not open a directory"
            );
        }
    }

    #[test]
    fn shadowed_names_stay_subcommands() {
        // 与子命令同名的目录裸写永远进对应子命令（含新增的 `open` 自身）；
        // 开目录走 `crossh open <name>` 无需转义。
        for name in [
            "git", "note", "help", "daemon", "ls", "run", "capture", "open",
        ] {
            let shadowed = parse_cli([name].into_iter().map(str::to_string), cwd_ok("/repo"));
            assert!(
                !matches!(shadowed, CliCommand::Main { .. }),
                "bare {name} must not open a directory"
            );

            let command = parse_cli(
                ["open", name].into_iter().map(str::to_string),
                cwd_ok("/repo"),
            );
            assert_eq!(
                command,
                CliCommand::Main {
                    path: Some(PathBuf::from(name))
                },
                "open {name} must open a directory"
            );
        }
    }

    #[test]
    fn session_subcommands_parse() {
        use super::DaemonAction;
        // `daemon` 缺省 status。
        let command = parse_cli(["daemon"].into_iter().map(str::to_string), cwd_ok("/repo"));
        assert_eq!(command, CliCommand::Daemon(DaemonAction::Status));
        for (arg, want) in [
            ("status", DaemonAction::Status),
            ("start", DaemonAction::Start),
            ("stop", DaemonAction::Stop),
        ] {
            let command = parse_cli(
                ["daemon", arg].into_iter().map(str::to_string),
                cwd_ok("/repo"),
            );
            assert_eq!(command, CliCommand::Daemon(want), "arg: {arg}");
        }
        // `daemon bogus` 回到未知参数。
        let command = parse_cli(
            ["daemon", "bogus"].into_iter().map(str::to_string),
            cwd_ok("/repo"),
        );
        assert_eq!(command, CliCommand::Unknown("bogus".to_string()));
        // `ls` 无参数；`capture` 必须带 id。
        let command = parse_cli(["ls"].into_iter().map(str::to_string), cwd_ok("/repo"));
        assert_eq!(command, CliCommand::Ls);
        let command = parse_cli(
            ["capture", "s1"].into_iter().map(str::to_string),
            cwd_ok("/repo"),
        );
        assert_eq!(command, CliCommand::Capture("s1".to_string()));
        let command = parse_cli(["capture"].into_iter().map(str::to_string), cwd_ok("/repo"));
        assert_eq!(command, CliCommand::Unknown("capture".to_string()));
        // `run` 吃掉 `--`，空参数由执行层报用法错误。
        let command = parse_cli(
            ["run", "--", "echo", "hi"].into_iter().map(str::to_string),
            cwd_ok("/repo"),
        );
        assert_eq!(
            command,
            CliCommand::Run(vec!["echo".to_string(), "hi".to_string()])
        );
        let command = parse_cli(["run"].into_iter().map(str::to_string), cwd_ok("/repo"));
        assert_eq!(command, CliCommand::Run(vec![]));
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
        use crossh_core::note_launcher::NoteCliCommand;

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
        assert!(text.contains("crossh open <path>"));
        assert!(text.contains("crossh open git"));
        assert!(text.contains("git"));
        assert!(text.contains("note"));
        assert!(text.contains("--help"));
    }
}

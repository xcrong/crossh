//! 顶层 CLI 解析与子进程委托。
//!
//! `main.rs` 保持装配职责（日志、运行时、GPUI 启动），可测试的业务逻辑
//! 下沉到此模块：顶层参数分发、`crossh-agent` 委托命令构造与帮助文本。

use std::path::PathBuf;
use std::process::Command;

use crate::features::git_launcher::{self, GitCliCommand};

use super::LaunchTarget;

/// 顶层 CLI 分发结果，由 `main` 负责副作用（打印、退出、启动窗口）。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CliCommand {
    Help,
    Version,
    Agent(Vec<String>),
    Git(Result<GitCliCommand, String>),
    Unknown(String),
    Main(LaunchTarget),
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
        None => CliCommand::Main(LaunchTarget::Main),
        Some("--help" | "-h" | "help") => CliCommand::Help,
        Some("--version" | "-V") => CliCommand::Version,
        Some("agent") => {
            let remaining = args.collect::<Vec<_>>();
            CliCommand::Agent(remaining)
        }
        Some("git") => {
            let result = git_launcher::parse_cli(args, current_dir);
            CliCommand::Git(result)
        }
        Some(argument) => CliCommand::Unknown(argument.to_string()),
    }
}

/// 返回与 `print_help` 打印内容一致的帮助文本，便于测试与复用。
pub(crate) fn help_text() -> String {
    format!(
        "Crossh {}\n\nUsage: crossh [COMMAND]\n\nCommands:\n  agent       Start the interactive coding agent\n  git         Open the Git Viewer for a directory\n  help        Print help\n\nOptions:\n  -h, --help     Print help\n  -V, --version  Print version",
        env!("CARGO_PKG_VERSION")
    )
}

/// 打印顶层帮助到 stdout。
pub(crate) fn print_help() {
    println!("{}", help_text());
}

/// 构造 `crossh-agent` 子进程命令，可测试的纯构造逻辑。
pub(crate) fn agent_process_command(arguments: &[String]) -> Command {
    let executable = crossh_core::process::sibling_executable("crossh-agent");
    let mut command = Command::new(executable);
    command.args(arguments);
    command
}

/// 委托同目录（或 PATH）的 `crossh-agent` 二进制。继承 stdio 让 TUI 直接
/// 使用当前终端，launcher 不碰 termios；透传子进程退出码，退出路径不变。
pub(crate) fn spawn_agent_process(arguments: &[String]) -> Result<i32, String> {
    let mut command = agent_process_command(arguments);
    let executable = command.get_program().to_os_string();
    let status = command.status().map_err(|error| {
        // sibling_executable 在找不到同伴二进制时回退为纯文件名（交给
        // PATH）；此时启动失败几乎一定是二进制缺失，给出可执行的指引。
        let is_relative = std::path::Path::new(&executable).is_relative();
        if is_relative {
            format!(
                "{error}: crossh-agent not found next to crossh or on PATH; build it with `cargo build` or install crossh-agent alongside crossh"
            )
        } else {
            error.to_string()
        }
    })?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{CliCommand, agent_process_command, help_text, parse_cli};
    use crate::features::git_launcher::GitCliCommand;

    fn cwd_ok(path: &str) -> Result<PathBuf, String> {
        Ok(PathBuf::from(path))
    }

    #[test]
    fn no_arguments_maps_to_main() {
        let command = parse_cli(std::iter::empty(), cwd_ok("/repo"));
        assert_eq!(command, CliCommand::Main(super::super::LaunchTarget::Main));
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
    fn agent_collects_remaining_arguments() {
        let command = parse_cli(
            ["agent", "--help", "extra"].into_iter().map(str::to_string),
            cwd_ok("/repo"),
        );
        assert_eq!(
            command,
            CliCommand::Agent(vec!["--help".to_string(), "extra".to_string()])
        );
    }

    #[test]
    fn agent_with_no_extra_args_is_empty() {
        let command = parse_cli(["agent"].into_iter().map(str::to_string), cwd_ok("/repo"));
        assert_eq!(command, CliCommand::Agent(vec![]));
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
    fn unknown_argument_is_preserved() {
        let command = parse_cli(["unknown"].into_iter().map(str::to_string), cwd_ok("/repo"));
        assert_eq!(command, CliCommand::Unknown("unknown".to_string()));
    }

    #[test]
    fn help_text_contains_version_and_commands() {
        let text = help_text();
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
        assert!(text.contains("Usage: crossh [COMMAND]"));
        assert!(text.contains("agent"));
        assert!(text.contains("git"));
        assert!(text.contains("--help"));
        assert!(text.contains("--version"));
    }

    #[test]
    fn agent_process_command_receives_requested_arguments() {
        let command = agent_process_command(&["--help".to_string(), "extra".to_string()]);
        assert_eq!(
            command
                .get_args()
                .map(|arg| arg.to_string_lossy().to_string())
                .collect::<Vec<_>>(),
            ["--help".to_string(), "extra".to_string()]
        );
        let program = command.get_program().to_string_lossy();
        assert!(
            program.contains("crossh-agent"),
            "program should be crossh-agent, got {program}"
        );
    }
}

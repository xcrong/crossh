//! Git Viewer 的命令行解析与独立进程启动器。
//!
//! 该模块保持轻量，使主工作区只需携带启动入口，不必编译完整 Git UI。

use std::path::{Path, PathBuf};
use std::process::Command;
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GitCliCommand {
    Open(PathBuf),
    Help,
}

pub(crate) fn parse_cli(
    mut args: impl Iterator<Item = String>,
    current_dir: Result<PathBuf, String>,
) -> Result<GitCliCommand, String> {
    match args.next().as_deref() {
        None => current_dir.map(GitCliCommand::Open),
        Some("--help" | "-h" | "help") if args.next().is_none() => Ok(GitCliCommand::Help),
        Some(argument) if args.next().is_none() => {
            let path = PathBuf::from(argument);
            if path.is_absolute() {
                Ok(GitCliCommand::Open(path))
            } else {
                current_dir.map(|directory| GitCliCommand::Open(directory.join(path)))
            }
        }
        Some(argument) => Err(format!("unexpected argument: {argument}")),
    }
}

// 双 binary 复用：`print_cli_help` 仅主进程，`print_standalone_cli_help` 仅独立二进制
#[allow(dead_code)]
pub(crate) fn print_cli_help() {
    print_help_for("crossh git");
}

#[allow(dead_code)]
pub(crate) fn print_standalone_cli_help() {
    print_help_for("crossh-git");
}

fn print_help_for(command: &str) {
    println!(
        "Usage: {command} [DIRECTORY]\n\nOpen the Git Viewer for DIRECTORY, or the current directory when omitted."
    );
}

#[allow(dead_code)]
pub(crate) fn spawn_git_process(cwd: &Path) -> std::io::Result<()> {
    git_process_command(cwd)?.spawn().map(|_| ())
}

fn git_process_command(cwd: &Path) -> std::io::Result<Command> {
    let mut cmd = crossh_core::process::sibling_command("crossh-git");
    cmd.arg(cwd).current_dir(cwd);
    Ok(cmd)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{GitCliCommand, git_process_command, parse_cli};

    #[test]
    fn no_arguments_open_the_current_directory() {
        assert_eq!(
            parse_cli(std::iter::empty(), Ok(PathBuf::from("/repo"))),
            Ok(GitCliCommand::Open(PathBuf::from("/repo")))
        );
    }

    #[test]
    fn help_and_invalid_arguments_are_distinct() {
        assert_eq!(
            parse_cli(
                ["--help"].into_iter().map(str::to_string),
                Ok(PathBuf::new())
            ),
            Ok(GitCliCommand::Help)
        );
        assert_eq!(
            parse_cli(
                ["first", "second"].into_iter().map(str::to_string),
                Ok(PathBuf::new())
            ),
            Err("unexpected argument: first".to_string())
        );
    }

    #[test]
    fn one_directory_argument_is_resolved_from_the_current_directory() {
        assert_eq!(
            parse_cli(
                ["other"].into_iter().map(str::to_string),
                Ok(PathBuf::from("/repo"))
            ),
            Ok(GitCliCommand::Open(PathBuf::from("/repo/other")))
        );
    }

    #[test]
    fn absolute_directory_argument_is_preserved() {
        assert_eq!(
            parse_cli(
                ["/repo/other"].into_iter().map(str::to_string),
                Ok(PathBuf::from("/repo"))
            ),
            Ok(GitCliCommand::Open(PathBuf::from("/repo/other")))
        );
    }

    #[test]
    fn current_directory_errors_are_reported() {
        assert_eq!(
            parse_cli(std::iter::empty(), Err("cwd unavailable".to_string())),
            Err("cwd unavailable".to_string())
        );
    }

    #[test]
    fn detached_git_process_receives_requested_directory() {
        let command = git_process_command(Path::new("/repo")).expect("command should build");

        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [Path::new("/repo").as_os_str()]
        );
        assert_eq!(command.get_current_dir(), Some(Path::new("/repo")));
        assert_eq!(
            Path::new(command.get_program())
                .file_name()
                .and_then(|name| name.to_str()),
            Some("crossh-git")
        );
    }
}

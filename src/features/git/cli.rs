use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub(crate) const GIT_WINDOW_PROCESS_ENV: &str = "CROSSH_GIT_WINDOW_PROCESS";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GitCliCommand {
    Open(PathBuf),
    Help,
}

pub(crate) fn parse(
    mut args: impl Iterator<Item = String>,
    current_dir: Result<PathBuf, String>,
) -> Result<GitCliCommand, String> {
    match args.next().as_deref() {
        None => current_dir.map(GitCliCommand::Open),
        Some("--help" | "-h" | "help") if args.next().is_none() => Ok(GitCliCommand::Help),
        Some(argument) if args.next().is_none() => Ok(GitCliCommand::Open(PathBuf::from(argument))),
        Some(argument) => Err(format!("unexpected argument: {argument}")),
    }
}

pub(crate) fn print_help() {
    println!(
        "Usage: crossh git [DIRECTORY]\n\nOpen the Git Viewer for DIRECTORY, or the current directory when omitted."
    );
}

pub(crate) fn running_as_window_process() -> bool {
    std::env::var_os(GIT_WINDOW_PROCESS_ENV).is_some_and(|value| value == "1")
}

pub(crate) fn spawn_window_process(cwd: &Path) -> std::io::Result<()> {
    let executable = std::env::current_exe()?;
    window_process_command(&executable, cwd).spawn().map(|_| ())
}

fn window_process_command(executable: &Path, cwd: &Path) -> Command {
    let mut command = Command::new(executable);
    command
        .arg("git")
        .current_dir(cwd)
        .env(GIT_WINDOW_PROCESS_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }

    command
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    use super::{GIT_WINDOW_PROCESS_ENV, GitCliCommand, parse, window_process_command};

    #[test]
    fn no_arguments_open_the_current_directory() {
        assert_eq!(
            parse(std::iter::empty(), Ok(PathBuf::from("/repo"))),
            Ok(GitCliCommand::Open(PathBuf::from("/repo")))
        );
    }

    #[test]
    fn help_and_invalid_arguments_are_distinct() {
        assert_eq!(
            parse(
                ["--help"].into_iter().map(str::to_string),
                Ok(PathBuf::new())
            ),
            Ok(GitCliCommand::Help)
        );
        assert_eq!(
            parse(
                ["first", "second"].into_iter().map(str::to_string),
                Ok(PathBuf::new())
            ),
            Err("unexpected argument: first".to_string())
        );
    }

    #[test]
    fn one_directory_argument_opens_that_directory() {
        assert_eq!(
            parse(
                ["/repo/other"].into_iter().map(str::to_string),
                Ok(PathBuf::from("/repo"))
            ),
            Ok(GitCliCommand::Open(PathBuf::from("/repo/other")))
        );
    }

    #[test]
    fn current_directory_errors_are_reported() {
        assert_eq!(
            parse(std::iter::empty(), Err("cwd unavailable".to_string())),
            Err("cwd unavailable".to_string())
        );
    }

    #[test]
    fn detached_window_process_reenters_git_command_in_requested_directory() {
        let command = window_process_command(Path::new("/bin/crossh"), Path::new("/repo"));

        assert_eq!(command.get_program(), OsStr::new("/bin/crossh"));
        assert_eq!(command.get_args().collect::<Vec<_>>(), [OsStr::new("git")]);
        assert_eq!(command.get_current_dir(), Some(Path::new("/repo")));
        assert!(command.get_envs().any(|(name, value)| {
            name == OsStr::new(GIT_WINDOW_PROCESS_ENV) && value == Some(OsStr::new("1"))
        }));
    }
}

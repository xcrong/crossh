use std::path::PathBuf;

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
        Some(argument) => Err(format!("unexpected argument: {argument}")),
    }
}

pub(crate) fn print_help() {
    println!("Usage: crossh git\n\nOpen the Git Viewer for the current directory.");
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{GitCliCommand, parse};

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
                ["extra"].into_iter().map(str::to_string),
                Ok(PathBuf::new())
            ),
            Err("unexpected argument: extra".to_string())
        );
    }

    #[test]
    fn current_directory_errors_are_reported() {
        assert_eq!(
            parse(std::iter::empty(), Err("cwd unavailable".to_string())),
            Err("cwd unavailable".to_string())
        );
    }
}

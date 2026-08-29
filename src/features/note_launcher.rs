//! Note Viewer 的命令行解析与独立进程启动器。
//!
//! 该模块保持轻量，使主工作区只需携带启动入口，不必编译完整 Note UI。

use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NoteCliCommand {
    Open,
    Help,
}

pub(crate) fn parse_cli(mut args: impl Iterator<Item = String>) -> Result<NoteCliCommand, String> {
    match args.next().as_deref() {
        None => Ok(NoteCliCommand::Open),
        Some("--help" | "-h" | "help") => Ok(NoteCliCommand::Help),
        Some(other) => Err(format!("unknown argument: {other}")),
    }
}

// 双 binary 复用：`print_cli_help` 仅主进程使用，`print_standalone_cli_help` 仅独立二进制使用
#[allow(dead_code)]
pub(crate) fn print_cli_help() {
    print_help_for("crossh note");
}

#[allow(dead_code)]
pub(crate) fn print_standalone_cli_help() {
    print_help_for("crossh-note");
}

fn print_help_for(command: &str) {
    println!("Usage: {command}\n\nOpen the Note Viewer.");
}

#[allow(dead_code)]
pub(crate) fn spawn_note_process() -> std::io::Result<()> {
    note_process_command()?.spawn().map(|_| ())
}

fn note_process_command() -> std::io::Result<Command> {
    Ok(crossh_core::process::sibling_command("crossh-note"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cli_defaults_to_open() {
        assert_eq!(parse_cli([].into_iter()).unwrap(), NoteCliCommand::Open);
    }

    #[test]
    fn parse_cli_help() {
        assert_eq!(
            parse_cli(["--help".to_string()].into_iter()).unwrap(),
            NoteCliCommand::Help
        );
        assert_eq!(
            parse_cli(["-h".to_string()].into_iter()).unwrap(),
            NoteCliCommand::Help
        );
    }

    #[test]
    fn parse_cli_unknown_fails() {
        assert!(parse_cli(["--unknown".to_string()].into_iter()).is_err());
    }

    #[test]
    fn note_process_command_uses_sibling_or_path() {
        let cmd = note_process_command().unwrap();
        let prog = cmd.get_program().to_string_lossy().to_string();
        assert!(prog.contains("crossh-note"));
    }
}

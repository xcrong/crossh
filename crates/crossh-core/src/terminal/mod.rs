//! Transport-neutral terminal contracts and title helpers.

pub mod session;
pub mod shell;
pub mod title;

pub use session::{InputCmd, SessionEvent, TerminalProcessInfo};
pub use shell::{
    LocalShellEnvironment, RemoteShell, ShellCommandMarker, ShellPromptMarker,
    command_marker_from_title, prompt_marker_from_title, remote_shell_bootstrap_command,
    remote_shell_from_path, remote_shell_setup_script,
};
pub use title::{
    local_terminal_tab_title, local_terminal_title, path_display_name, remote_pane_title,
    remote_terminal_title, strip_shell_host_prefix, truncate_path_title, truncate_title,
};

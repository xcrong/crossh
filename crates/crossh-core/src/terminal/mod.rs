//! Transport-neutral terminal contracts and title helpers.

pub mod session;
pub mod shell;
pub mod title;

pub use session::{InputCmd, SessionEvent, TerminalProcessInfo};
pub use shell::{
    RemoteShell, command_status_from_title, remote_shell_from_path, remote_shell_setup_script,
    shell_setup_script_for_path,
};
pub use title::{
    local_terminal_tab_title, local_terminal_title, remote_pane_title, remote_terminal_title,
    strip_shell_host_prefix, truncate_path_title, truncate_title,
};

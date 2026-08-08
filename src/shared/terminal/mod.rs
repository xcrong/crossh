//! Terminal contracts shared by the workspace and terminal feature.

pub(crate) mod session;
#[cfg(test)]
pub(crate) mod shell;
pub(crate) mod title;

#[cfg(test)]
pub(crate) use session::{InputCmd, SessionEvent};
#[cfg(test)]
pub(crate) use shell::{RemoteShell, remote_shell_from_path, remote_shell_setup_script};
pub(crate) use title::{
    local_terminal_tab_title, local_terminal_title, remote_pane_title, remote_terminal_title,
    strip_shell_host_prefix, truncate_path_title,
};

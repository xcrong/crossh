//! Terminal contracts shared by local/SSH backends and the terminal feature.

pub(crate) mod keyboard;
pub(crate) mod protocol;
pub(crate) mod session;
#[cfg(test)]
pub(crate) mod shell;
pub(crate) mod title;

pub(crate) use keyboard::KeyboardProtocolState;
pub(crate) use protocol::{
    ImageDimension, ImagePayload, KittyGraphicsPayload, NotificationOccasion, ProtocolEvent,
    ShellEvent, TerminalProtocolParser,
};
pub(crate) use session::{InputCmd, SessionEvent, TerminalProcessInfo};
#[cfg(test)]
pub(crate) use shell::{RemoteShell, remote_shell_from_path, remote_shell_setup_script};
pub(crate) use title::{
    local_terminal_tab_title, local_terminal_title, remote_pane_title, remote_terminal_title,
    strip_shell_host_prefix, truncate_path_title,
};

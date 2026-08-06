//! Terminal contracts shared by local/SSH backends and the terminal feature.

pub(crate) mod protocol;
pub(crate) mod session;
pub(crate) mod title;

pub(crate) use protocol::{
    ImageDimension, ImagePayload, KittyGraphicsPayload, NotificationOccasion, ProtocolEvent,
    ShellEvent, TerminalProtocolParser,
};
pub(crate) use session::{InputCmd, SessionEvent, TerminalProcessInfo};
pub(crate) use title::{
    local_terminal_tab_title, local_terminal_title, remote_pane_title, remote_terminal_title,
    strip_shell_host_prefix, truncate_path_title,
};

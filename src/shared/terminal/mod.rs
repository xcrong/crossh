//! Terminal contracts shared by local/SSH backends and the terminal feature.

pub(crate) mod protocol;
pub(crate) mod session;

pub(crate) use protocol::{
    ImageDimension, ImagePayload, KittyGraphicsPayload, NotificationOccasion, ProtocolEvent,
    ShellEvent, TerminalProtocolParser,
};
pub(crate) use session::{InputCmd, SessionEvent};

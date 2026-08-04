//! Transport-neutral terminal input and output events.

/// Command sent to a local or remote terminal backend.
#[allow(dead_code)]
pub(crate) enum InputCmd {
    /// User-entered bytes.
    Write(Vec<u8>),
    /// Terminal size changed.
    Resize { cols: u16, rows: u16 },
    /// Close the terminal channel.
    Close,
}

/// Event emitted by a local or remote terminal backend.
#[derive(Debug)]
pub(crate) enum SessionEvent {
    /// TCP/KEX/auth or local PTY setup completed.
    Connected,
    /// Terminal stdout/stderr bytes.
    Output(Vec<u8>),
    /// Shell integration reported the current working directory.
    Cwd(String),
    /// Backend setup or relay failed.
    Error(String),
    /// Terminal channel or local PTY closed.
    Closed,
}

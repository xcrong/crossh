//! Terminal feature events exposed to the workspace.

/// Connection lifecycle state rendered by terminal and workspace views.
pub(crate) use crate::infrastructure::ssh::ConnectionState as ConnState;

/// Events emitted by a terminal entity to its workspace owner.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) enum TerminalEvent {
    /// The local shell reported the command that is about to execute.
    CommandStarted {
        command: String,
        cwd: Option<String>,
    },
    /// Shell integration reported the exit status of the command that just ran.
    CommandFinished { status: Option<i32> },
    /// Shell integration reported a new working directory.
    CwdChanged,
    /// The local shell returned to a prompt.
    PromptReached,
    /// The terminal title changed.
    TitleChanged,
    /// The terminal produced a user-facing notification or bell.
    Notification,
    /// The shell or SSH channel ended.
    Closed,
}

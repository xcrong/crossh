//! Transport-neutral terminal contracts and helpers.

/// Snapshot of the process currently attached to a local terminal's
/// foreground process group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalProcessInfo {
    pub name: String,
    pub cwd: Option<String>,
}

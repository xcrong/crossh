//! Transport-independent connection lifecycle state.

/// Lifecycle state shared by transport engines and UI adapters.
#[derive(Default, Clone, Debug, PartialEq)]
pub enum ConnectionState {
    #[default]
    Connecting,
    Connected,
    Error(String),
    Closed,
}

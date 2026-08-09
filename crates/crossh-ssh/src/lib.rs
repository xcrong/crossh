//! SSH infrastructure: runtime, russh channels, pooling, SFTP, forwarding, and ProxyJump.

mod connection;
mod forward;
mod pool;
mod proxyjump;
mod runtime;
mod session;
mod sftp;

pub use connection::{
    ConnEvent, ConnectionHandle, CredentialKind, ForwardKind, HostKeyDecision, RemoteCommandEvent,
    RemoteCommandStatus,
};
pub use crossh_core::connection::ConnectionState;
pub use pool::key_for as connection_key;
pub use runtime::runtime as ssh_runtime;
pub use session::{AuthChoice, default_auth_for};
pub use sftp::{MAX_EDITOR_FILE_BYTES, RemoteEntry, SftpCmd, SftpEvent};

//! SSH infrastructure: runtime, russh channels, pooling, SFTP, forwarding, and ProxyJump.

pub mod connection;
pub mod forward;
pub mod pool;
pub mod proxyjump;
pub mod runtime;
pub mod session;
pub mod sftp;

pub use connection::{
    Connection, ConnectionState, CredentialKind, ForwardKind, HostKeyDecision, PendingPrompt,
    RemoteCommandStatus,
};
pub use pool::ConnectionPool;
pub use runtime::runtime as ssh_runtime;
pub use session::AuthChoice;
pub use sftp::{MAX_EDITOR_FILE_BYTES, RemoteEntry, SftpCmd, SftpEvent};

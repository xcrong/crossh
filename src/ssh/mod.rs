//! SSH 层：tokio 运行时 + russh 连接抽象 + 终端通道桥接类型 + 连接池 + SFTP + 端口转发 + ProxyJump。

pub mod connection;
pub mod forward;
pub mod pool;
pub mod proxyjump;
pub mod runtime;
pub mod session;
pub mod sftp;

pub use connection::{Connection, CredentialKind, ForwardKind, HostKeyDecision, PendingPrompt};
pub use pool::ConnectionPool;
pub use runtime::runtime as ssh_runtime;
pub use session::{default_auth_for, InputCmd, SessionEvent};
pub use sftp::{RemoteEntry, SftpCmd, SftpEvent};

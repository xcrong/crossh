//! 会话属主进程的库入口：协议编解码、会话属主、单实例与握手。
//!
//! 零 `gpui` / `gpui_platform` / `crossh-terminal` 依赖，见 `tasks.md` 1.1。

pub mod client;
pub mod feed;
pub mod handshake;
pub mod instance;
pub mod protocol;
pub mod serve;
pub mod session;
pub mod spawn;
pub use client::{
    Attached, Client, ServerEvent, capture_session, daemon_bin_path, daemon_stop, ensure_daemon,
    role_of, run_command,
};
pub use feed::{FeedState, FeedUpdate, program_state_name};
pub use handshake::{HandshakeError, HandshakeRequest, HandshakeResult, verify_handshake};
pub use instance::{Claim, claim_instance, remove_port_file};
pub use protocol::{MAX_FRAME, MessageKind, decode_frame, encode_frame, write_frame_vectored};
pub use session::{Ownership, OwnershipEvent, ReplayRing, SessionMeta, SessionOwner};
pub use spawn::{ChildSession, OutputChunk, SpawnOptions};

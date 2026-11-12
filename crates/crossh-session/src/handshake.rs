//! 协议版本与能力协商 + 握手纯逻辑。
//!
//! - `PROTOCOL_VERSION` 单调递增；版本不一致在握手阶段拒绝，不进入半可用。
//! - 能力标志每个都写明“对不支持它的对端该怎么办”：一律忽略未知标志，
//!   用双方共同子集继续工作，旧端不因新标志断开。

use serde::{Deserialize, Serialize};

/// 当前协议版本。升级时 +1，旧 daemon 拒绝新客户端并给出恢复命令。
pub const PROTOCOL_VERSION: u32 = 1;

/// 已知能力标志。未知标志 MUST 被忽略（见 `normalize_capabilities`）。
pub mod capabilities {
    /// 服务端在回放后发送 `ReplayEnd` 边界帧。
    /// 不支持的对端：把回放与实时同等追加显示，不依赖该帧也可用。
    pub const REPLAY_MARK: &str = "replay-mark";
    /// observer 以自己尺寸本地排版，服务端不做尺寸租约。
    /// 不支持的对端：忽略显示差异，输入所有权规则不变。
    pub const OBSERVER_LOCAL_LAYOUT: &str = "observer-local-layout";
    /// 只读观察者在失去输入权时显示只读标记。
    /// 不支持的对端：不显示标记，输入仍被服务端拒绝。
    pub const READONLY_BADGE: &str = "readonly-badge";
}

/// 握手请求载荷（`MessageKind::Hello` 的 JSON）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HandshakeRequest {
    pub version: u32,
    pub capabilities: Vec<String>,
    pub token: String,
}

/// 握手成功应答（`MessageKind::Welcome` 的 JSON）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HandshakeOk {
    pub version: u32,
    /// 双方交集，作为本次连接的生效能力。
    pub capabilities: Vec<String>,
    /// 服务端分配的连接 id；会话 `controller` 与它比较得出"我的角色"。
    pub client_id: u64,
}

/// 固定错误载荷（`MessageKind::Error` 的 JSON）。不含任何会话字段。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorPayload {
    pub code: String,
    pub message: String,
}

/// 握手失败类型。错误值里不含会话字段，文案不含令牌子串。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandshakeError {
    /// 版本不一致。文案 MUST 给出 `crossh daemon stop` 恢复命令。
    VersionMismatch { want: u32, got: u32 },
    /// 令牌缺失/错误。固定文案，不泄露会话信息。
    Unauthorized,
}

impl std::fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VersionMismatch { want, got } => write!(
                f,
                "protocol version mismatch (daemon {want}, client {got}); run `crossh daemon stop` then retry"
            ),
            Self::Unauthorized => write!(f, "handshake rejected"),
        }
    }
}

impl std::error::Error for HandshakeError {}

/// 握手结果：成功时返回协商出的能力交集。
pub type HandshakeResult = Result<HandshakeOk, HandshakeError>;

/// 去掉重复、排序后返回；未知标志保留透传，由接收方忽略。
/// 调用方 MUST 对未知标志采取“忽略而不是断开”。
pub fn normalize_capabilities(caps: &[String]) -> Vec<String> {
    let mut out: Vec<String> = caps.to_vec();
    out.sort();
    out.dedup();
    out
}

/// 纯逻辑校验：版本 → 令牌。无 IO，便于单元测试。
pub fn verify_handshake(
    req: &HandshakeRequest,
    expected_version: u32,
    expected_token: &str,
    server_capabilities: &[String],
    client_id: u64,
) -> HandshakeResult {
    if req.version != expected_version {
        return Err(HandshakeError::VersionMismatch {
            want: expected_version,
            got: req.version,
        });
    }
    if req.token != expected_token {
        return Err(HandshakeError::Unauthorized);
    }
    let server = normalize_capabilities(server_capabilities);
    let client = normalize_capabilities(&req.capabilities);
    let agreed: Vec<String> = server.into_iter().filter(|c| client.contains(c)).collect();
    Ok(HandshakeOk {
        version: expected_version,
        capabilities: agreed,
        client_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(version: u32, token: &str, caps: &[&str]) -> HandshakeRequest {
        HandshakeRequest {
            version,
            capabilities: caps.iter().map(|s| s.to_string()).collect(),
            token: token.to_string(),
        }
    }

    #[test]
    fn version_mismatch_names_recovery_command() {
        let err = verify_handshake(&req(999, "t", &[]), PROTOCOL_VERSION, "t", &[], 7).unwrap_err();
        assert!(matches!(err, HandshakeError::VersionMismatch { .. }));
        assert!(err.to_string().contains("crossh daemon stop"));
    }

    #[test]
    fn bad_token_is_fixed_text_without_session_fields() {
        let err = verify_handshake(
            &req(PROTOCOL_VERSION, "wrong", &[]),
            PROTOCOL_VERSION,
            "right",
            &[],
            7,
        )
        .unwrap_err();
        assert_eq!(err, HandshakeError::Unauthorized);
        let text = err.to_string();
        assert!(!text.contains("wrong"));
        assert!(!text.contains("right"));
        assert!(!text.contains("session"));
    }

    #[test]
    fn unknown_flags_do_not_break_common_subset() {
        let ok = verify_handshake(
            &req(
                PROTOCOL_VERSION,
                "t",
                &[capabilities::REPLAY_MARK, "future-flag"],
            ),
            PROTOCOL_VERSION,
            "t",
            &[capabilities::REPLAY_MARK.to_string()],
            42,
        )
        .unwrap();
        assert_eq!(ok.capabilities, vec![capabilities::REPLAY_MARK.to_string()]);
        assert_eq!(ok.client_id, 42);
    }
}

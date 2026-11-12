//! 会话属主：回放环、输入所有权、尺寸仲裁、会话元数据。
//!
//! - 每会话 512 KiB 原始输出环（保留最近、先丢最旧），见 D9。
//! - 输入所有权：首个接入者为 controller；在接客户端发送一次输入即取得；
//!   controller 断开后角色为“无”。尺寸只接受 controller 上报，见 D8。
//! - 本模块零 IO，可单元测试；PTY 读写线程在 daemon 侧组装。

use std::collections::{HashMap, VecDeque};

use crate::feed::{FeedState, FeedUpdate};

/// 回放上限：每会话最近 512 KiB 原始输出（写入规格 R3）。
pub const REPLAY_LIMIT_BYTES: usize = 512 * 1024;

/// 环形缓冲：只保留最近 `REPLAY_LIMIT_BYTES` 字节。
#[derive(Clone, Debug, Default)]
pub struct ReplayRing {
    chunks: VecDeque<Vec<u8>>,
    len: usize,
}

impl ReplayRing {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 追加原始输出，超出上限时从头部丢弃整块（保留最近）。
    pub fn push(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        // 单次写入超过上限：只保留尾部。
        let bytes = if bytes.len() > REPLAY_LIMIT_BYTES {
            &bytes[bytes.len() - REPLAY_LIMIT_BYTES..]
        } else {
            bytes
        };
        self.chunks.push_back(bytes.to_vec());
        self.len += bytes.len();
        while self.len > REPLAY_LIMIT_BYTES {
            if let Some(front) = self.chunks.pop_front() {
                self.len -= front.len();
            } else {
                break;
            }
        }
    }

    /// 按写入顺序拼接（回放用）。
    pub fn replay(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len);
        for c in &self.chunks {
            out.extend_from_slice(c);
        }
        out
    }
}

/// 输入权持有者：`None` 为暂无持有者。
pub type ControllerId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnershipEvent {
    /// 某客户端取得输入权（含首次）。
    Takeover { new_controller: ControllerId },
    /// controller 断开，输入权空出。
    Released,
    /// observer 输入被丢弃（只观察）。
    ObserverInputDropped { from: ControllerId },
}

/// 输入所有权状态机（v1：发送一次输入即取得，无额外命令）。
#[derive(Clone, Debug, Default)]
pub struct Ownership {
    controller: Option<ControllerId>,
}

impl Ownership {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn controller(&self) -> Option<ControllerId> {
        self.controller
    }

    /// 客户端接入：首个接入者成为 controller，其余为 observer（返回 `None`）。
    pub fn on_attach(&mut self, client: ControllerId) -> Option<OwnershipEvent> {
        if self.controller.is_none() {
            self.controller = Some(client);
            Some(OwnershipEvent::Takeover {
                new_controller: client,
            })
        } else {
            None
        }
    }

    /// 客户端发送输入：在接者即取得输入权；非 controller 的输入应被丢弃。
    /// 返回 `Some(Takeover)` 表示易手，`Some(Dropped)` 表示应丢弃。
    pub fn on_input(&mut self, from: ControllerId) -> Option<OwnershipEvent> {
        if self.controller == Some(from) {
            None
        } else {
            // 发送一次输入即取得（规格 R2）。
            self.controller = Some(from);
            Some(OwnershipEvent::Takeover {
                new_controller: from,
            })
        }
    }

    /// 断开：是 controller 则释放，否则无变化。
    pub fn on_detach(&mut self, client: ControllerId) -> Option<OwnershipEvent> {
        if self.controller == Some(client) {
            self.controller = None;
            Some(OwnershipEvent::Released)
        } else {
            None
        }
    }

    /// 尺寸上报是否接受：仅 controller。
    pub fn accepts_resize(&self, from: ControllerId) -> bool {
        self.controller == Some(from)
    }
}

/// 会话元数据（列表与结束判定用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionMeta {
    pub id: String,
    pub cwd: Option<String>,
    /// 标题占位（服务端不解析标题语义）。
    pub title: Option<String>,
    /// 最近一次程序状态名（`idle`/`working`/…），`None` = 未知或已清除。
    pub program: Option<String>,
    pub controller: Option<ControllerId>,
    pub alive: bool,
    pub exit_code: Option<i32>,
}

/// 会话表：id →（元数据 + 字节馈送 + 所有权）。
#[derive(Debug, Default)]
pub struct SessionOwner {
    sessions: HashMap<String, (SessionMeta, FeedState, Ownership)>,
}

impl SessionOwner {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn insert(&mut self, meta: SessionMeta) {
        let id = meta.id.clone();
        self.sessions
            .entry(id)
            .or_insert((meta, FeedState::new(), Ownership::new()));
    }

    pub fn get(&self, id: &str) -> Option<&SessionMeta> {
        self.sessions.get(id).map(|(m, _, _)| m)
    }

    /// 推入输出字节，返回派生的会话表更新（cwd/程序状态变化时调用方同步元数据）。
    pub fn push_output(&mut self, id: &str, bytes: &[u8]) -> Option<FeedUpdate> {
        self.sessions
            .get_mut(id)
            .map(|(_, feed, _)| feed.push(bytes))
    }
    /// 子进程退出：标记结束并记录退出码，调用方随后广播并移除。
    pub fn mark_exited(&mut self, id: &str, code: i32) -> Option<SessionMeta> {
        if let Some((meta, _, _)) = self.sessions.get_mut(id) {
            meta.alive = false;
            meta.exit_code = Some(code);
            return Some(meta.clone());
        }
        None
    }

    pub fn remove(&mut self, id: &str) {
        self.sessions.remove(id);
    }

    pub fn list(&self) -> Vec<SessionMeta> {
        self.sessions.values().map(|(m, _, _)| m.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_tail_only() {
        let mut ring = ReplayRing::new();
        ring.push(b"hello ");
        ring.push(b"world");
        assert_eq!(ring.replay(), b"hello world");
        // 超出上限只保留尾部。
        let big = vec![b'x'; REPLAY_LIMIT_BYTES + 100];
        ring.push(&big);
        assert_eq!(ring.len(), REPLAY_LIMIT_BYTES);
        assert_eq!(ring.replay(), &big[big.len() - REPLAY_LIMIT_BYTES..]);
    }

    #[test]
    fn ownership_four_transitions() {
        let mut o = Ownership::new();
        // 1. 首个接入成为 controller。
        assert_eq!(
            o.on_attach(1),
            Some(OwnershipEvent::Takeover { new_controller: 1 })
        );
        // 2. 第二个接入只是观察。
        assert_eq!(o.on_attach(2), None);
        assert_eq!(o.controller(), Some(1));
        // 3. 在接者发送输入即接管。
        assert_eq!(
            o.on_input(2),
            Some(OwnershipEvent::Takeover { new_controller: 2 })
        );
        assert!(!o.accepts_resize(1));
        assert!(o.accepts_resize(2));
        // 4. controller 断开后角色为空。
        assert_eq!(o.on_detach(2), Some(OwnershipEvent::Released));
        assert_eq!(o.controller(), None);
        // 空位时发送输入即取得。
        assert_eq!(
            o.on_input(1),
            Some(OwnershipEvent::Takeover { new_controller: 1 })
        );
    }

    #[test]
    fn exit_marks_then_removes() {
        let mut owner = SessionOwner::new();
        owner.insert(SessionMeta {
            id: "s1".to_string(),
            cwd: None,
            title: None,
            program: None,
            controller: None,
            alive: true,
            exit_code: None,
        });
        let meta = owner.mark_exited("s1", 3).unwrap();
        assert!(!meta.alive);
        assert_eq!(meta.exit_code, Some(3));
        owner.remove("s1");
        assert!(owner.get("s1").is_none());
    }
}

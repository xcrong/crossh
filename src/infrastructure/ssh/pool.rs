//! 连接池：按 (user, host, port) 复用唯一已认证会话。
//!
//! - `acquire`：命中且连接仍可用（非 Closed/Error）→ 复用，在其上再开 channel；
//!   否则新建 Connection 并登记。
//! - 连接的引用计数/断开由 `Connection` 内部 channel 计数驱动：所有终端 channel
//!   关闭后 Connection 自行 disconnect；下次 acquire 见到 Closed 会重建。

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, Entity};

use super::session::AuthChoice;
use crate::infrastructure::config::{HostConfig, SshConfig};
use crate::infrastructure::ssh::connection::{Connection, ConnectionState};

pub struct ConnectionPool {
    map: HashMap<String, Entity<Connection>>,
}

impl ConnectionPool {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    /// 池键：`user@host:port`。
    pub fn key_for(host: &HostConfig) -> String {
        let user = host.user.as_deref().unwrap_or("");
        format!(
            "{}@{}:{}",
            user,
            host.effective_host(),
            host.effective_port()
        )
    }

    /// 获取/创建连接。命中且可用 → 复用；否则新建并登记。
    pub fn acquire(
        &mut self,
        host: HostConfig,
        methods: Vec<AuthChoice>,
        ssh_config: Arc<SshConfig>,
        cx: &mut App,
    ) -> Entity<Connection> {
        let key = Self::key_for(&host);
        if let Some(c) = self.map.get(&key) {
            let reusable = !matches!(
                c.read(cx).state,
                ConnectionState::Closed | ConnectionState::Error(_)
            );
            if reusable {
                return c.clone();
            }
        }
        let c = Connection::open(host, methods, ssh_config, cx);
        self.map.insert(key, c.clone());
        c
    }

    /// 查某主机当前连接状态（用于 sidebar 徽标）。
    pub fn state_for_key(&self, key: &str, cx: &App) -> Option<ConnectionState> {
        self.map.get(key).map(|c| c.read(cx).state.clone())
    }

    /// 找到第一个有待处理请求（主机密钥/凭据弹窗）的连接。
    pub fn pending_prompt_connection(&self, cx: &App) -> Option<Entity<Connection>> {
        self.map
            .values()
            .find(|c| c.read(cx).pending_prompt.is_some())
            .cloned()
    }
}

impl Default for ConnectionPool {
    fn default() -> Self {
        Self::new()
    }
}

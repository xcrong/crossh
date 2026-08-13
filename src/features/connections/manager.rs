//! Feature-level connection manager.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, Entity, WeakEntity};

use crossh_core::config::{HostConfig, SshConfig};
use crossh_ssh::{AuthChoice, ConnectionState, connection_key, default_auth_for};

use super::entity::Connection;
use super::host::{HostEntry, build_entries};

/// Owns SSH configuration, navigation entries, and reusable connections.
///
/// 连接池只持有弱引用：连接存活期与 UI 使用方（标签、转发面板、后台命令）
/// 的强引用一致。最后一个使用者释放后实体 Drop → `Connection::drop` 发送
/// shutdown，空闲的 SSH 会话随之断开，而不会残活到应用退出。
pub(crate) struct ConnectionManager {
    config: Arc<SshConfig>,
    entries: Vec<HostEntry>,
    connections: HashMap<String, WeakEntity<Connection>>,
    /// 连接键的插入顺序，让多连接同时等待弹窗时的展示顺序确定（FIFO）。
    order: Vec<String>,
}

impl ConnectionManager {
    pub(crate) fn new(config: Arc<SshConfig>) -> Self {
        let entries = build_entries(&config);
        Self {
            config,
            entries,
            connections: HashMap::new(),
            order: Vec::new(),
        }
    }

    pub(crate) fn entries(&self) -> &[HostEntry] {
        &self.entries
    }

    pub(crate) fn resolve(&self, target: &str) -> HostConfig {
        self.config.resolve(target)
    }

    pub(crate) fn auth_methods(&self, host: &HostConfig) -> Vec<AuthChoice> {
        default_auth_for(host)
    }

    pub(crate) fn pool_key(host: &HostConfig) -> String {
        connection_key(host)
    }

    pub(crate) fn acquire(
        &mut self,
        host: HostConfig,
        methods: Vec<AuthChoice>,
        cx: &mut App,
    ) -> Entity<Connection> {
        let key = connection_key(&host);
        if let Some(connection) = self.connections.get(&key).and_then(|weak| weak.upgrade()) {
            let state = connection.read(cx).state.clone();
            if !matches!(state, ConnectionState::Closed | ConnectionState::Error(_)) {
                return connection;
            }
        }

        let connection = Connection::open(host, methods, self.config.clone(), cx);
        if !self.connections.contains_key(&key) {
            self.order.push(key.clone());
        }
        self.connections.insert(key, connection.downgrade());
        connection
    }

    pub(crate) fn state_for_key(&self, key: &str, cx: &App) -> Option<ConnectionState> {
        self.connections
            .get(key)
            .and_then(|weak| weak.upgrade())
            .map(|connection| connection.read(cx).state.clone())
    }

    pub(crate) fn pending_prompt_connection(&self, cx: &App) -> Option<Entity<Connection>> {
        // 按插入顺序（FIFO）找第一个有待决弹窗的连接，避免 HashMap 迭代顺序
        // 任意导致多连接排队时展示/解析目标不稳定。
        self.order.iter().find_map(|key| {
            self.connections
                .get(key)
                .and_then(|weak| weak.upgrade())
                .filter(|connection| connection.read(cx).pending_prompt.is_some())
        })
    }
}

//! Feature-level connection manager.

use std::sync::Arc;

use gpui::{App, Entity};

use crate::infrastructure::config::{HostConfig, SshConfig};
use crate::infrastructure::ssh::{AuthChoice, Connection, ConnectionPool, ConnectionState};

use super::host::{HostEntry, build_entries};

/// Owns SSH configuration, navigation entries, and reusable connections.
pub(crate) struct ConnectionManager {
    config: Arc<SshConfig>,
    entries: Vec<HostEntry>,
    pool: ConnectionPool,
}

impl ConnectionManager {
    pub(crate) fn new(config: Arc<SshConfig>) -> Self {
        let entries = build_entries(&config);
        Self {
            config,
            entries,
            pool: ConnectionPool::new(),
        }
    }

    pub(crate) fn entries(&self) -> &[HostEntry] {
        &self.entries
    }

    pub(crate) fn resolve(&self, target: &str) -> HostConfig {
        self.config.resolve(target)
    }

    pub(crate) fn auth_methods(&self, host: &HostConfig) -> Vec<AuthChoice> {
        crate::infrastructure::ssh::session::default_auth_for(host)
    }

    pub(crate) fn pool_key(host: &HostConfig) -> String {
        ConnectionPool::key_for(host)
    }

    pub(crate) fn acquire(
        &mut self,
        host: HostConfig,
        methods: Vec<AuthChoice>,
        cx: &mut App,
    ) -> Entity<Connection> {
        self.pool.acquire(host, methods, self.config.clone(), cx)
    }

    pub(crate) fn state_for_key(&self, key: &str, cx: &App) -> Option<ConnectionState> {
        self.pool.state_for_key(key, cx)
    }

    pub(crate) fn pending_prompt_connection(&self, cx: &App) -> Option<Entity<Connection>> {
        self.pool.pending_prompt_connection(cx)
    }
}

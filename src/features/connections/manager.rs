//! Feature-level connection manager.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, Entity};

use crate::infrastructure::config::{HostConfig, SshConfig};
use crate::infrastructure::ssh::{AuthChoice, ConnectionState, connection_key};

use super::entity::Connection;
use super::host::{HostEntry, build_entries};

/// Owns SSH configuration, navigation entries, and reusable connections.
pub(crate) struct ConnectionManager {
    config: Arc<SshConfig>,
    entries: Vec<HostEntry>,
    connections: HashMap<String, Entity<Connection>>,
}

impl ConnectionManager {
    pub(crate) fn new(config: Arc<SshConfig>) -> Self {
        let entries = build_entries(&config);
        Self {
            config,
            entries,
            connections: HashMap::new(),
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
        connection_key(host)
    }

    pub(crate) fn acquire(
        &mut self,
        host: HostConfig,
        methods: Vec<AuthChoice>,
        cx: &mut App,
    ) -> Entity<Connection> {
        let key = connection_key(&host);
        if let Some(connection) = self.connections.get(&key) {
            let state = connection.read(cx).state.clone();
            if !matches!(state, ConnectionState::Closed | ConnectionState::Error(_)) {
                return connection.clone();
            }
        }

        let connection = Connection::open(host, methods, self.config.clone(), cx);
        self.connections.insert(key, connection.clone());
        connection
    }

    pub(crate) fn state_for_key(&self, key: &str, cx: &App) -> Option<ConnectionState> {
        self.connections
            .get(key)
            .map(|connection| connection.read(cx).state.clone())
    }

    pub(crate) fn pending_prompt_connection(&self, cx: &App) -> Option<Entity<Connection>> {
        self.connections
            .values()
            .find(|connection| connection.read(cx).pending_prompt.is_some())
            .cloned()
    }
}

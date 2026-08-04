//! SSH host entries used by the workspace navigation.

use crate::infrastructure::config::SshConfig;
use crate::infrastructure::ssh::ConnectionPool;

/// Host alias, resolved display details, and connection-pool key.
#[derive(Clone)]
pub(crate) struct HostEntry {
    pub(crate) alias: String,
    pub(crate) detail: String,
    pub(crate) key: String,
}

/// Build navigable host entries from the parsed SSH config.
pub(crate) fn build_entries(config: &SshConfig) -> Vec<HostEntry> {
    let mut out = Vec::new();
    for host in config.hosts() {
        let alias = host.alias().to_string();
        if alias == "*" || alias.starts_with('!') {
            continue;
        }
        let resolved = config.resolve(&alias);
        let detail = format!(
            "{}@{}:{}",
            resolved.user.as_deref().unwrap_or(""),
            resolved.effective_host(),
            resolved.effective_port()
        );
        let key = ConnectionPool::key_for(&resolved);
        out.push(HostEntry { alias, detail, key });
    }
    out
}

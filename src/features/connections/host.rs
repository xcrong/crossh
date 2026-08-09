//! SSH host entries used by the workspace navigation.

use crossh_core::config::SshConfig;
use crossh_ssh::connection_key;

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
        let key = connection_key(&resolved);
        out.push(HostEntry { alias, detail, key });
    }
    out
}

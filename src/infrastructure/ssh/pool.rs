//! Pure connection identity helpers.

use crate::infrastructure::config::HostConfig;

/// Stable reuse key for one resolved SSH endpoint.
pub fn key_for(host: &HostConfig) -> String {
    let user = host.user.as_deref().unwrap_or("");
    format!(
        "{}@{}:{}",
        user,
        host.effective_host(),
        host.effective_port()
    )
}

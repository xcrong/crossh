//! Pure connection identity helpers.

use crossh_core::config::HostConfig;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crossh_core::config::SshConfig;

    #[test]
    fn connection_key_uses_the_resolved_endpoint_identity() {
        let config = SshConfig::default();
        assert_eq!(key_for(&config.resolve("host")), "@host:22");
        assert_eq!(
            key_for(&config.resolve("alice@example.com:2200")),
            "alice@example.com:2200"
        );
        assert_eq!(
            key_for(&config.resolve("ops@[2001:db8::1]:2222")),
            "ops@2001:db8::1:2222"
        );
    }
}

//! ProxyJump（单层跳板）：经跳板机连到目标。
//!
//! 流程：解析跳板 → 直连+认证跳板 → 在跳板会话上 `direct_tcpip` 到 target:port →
//! 用 `client::connect_stream` 在该流上跑第二个 SSH client（目标主机密钥/认证
//! 仍走反应式 UI）。跳板 Handle 必须保活，否则底层 channel 流断开。
//!
//! 仅支持单层；多层或无法解析时返回错误（计划标注，不自动降级）。

use std::sync::Arc;

use async_channel::Sender;
use russh::client::{self, Handle, connect_stream};

use crossh_core::config::{HostConfig, SshConfig};

use super::connection::{ClientHandler, ConnEvent, connect_direct_and_auth};
use super::forward::RemoteForwardRegistry;
use super::session::default_auth_for;

/// 解析 ProxyJump 值：若是 config 中已知别名则 resolve；否则按 `[user@]host[:port]` 解析。
pub(crate) fn resolve_jump(config: &SshConfig, alias: &str) -> HostConfig {
    config.resolve(alias)
}

/// 经跳板打开到目标的未认证 Handle，并返回需保活的跳板 Handle。
///
/// 调用方随后对 target handle 做 `authenticate`。
pub(crate) async fn open_target_via_jump(
    config: Arc<SshConfig>,
    jump_alias: String,
    target_host: String,
    target_port: u16,
    event_tx: &Sender<ConnEvent>,
    registry: RemoteForwardRegistry,
) -> anyhow::Result<(Handle<ClientHandler>, Handle<ClientHandler>)> {
    // 1) 跳板：直连+认证（跳板自身不应再有 proxy_jump；多层不在支持范围）。
    let jump_cfg = resolve_jump(&config, &jump_alias);
    if jump_cfg.proxy_jump.is_some() {
        anyhow::bail!(
            "multi-layer ProxyJump not supported (jump {} itself has ProxyJump)",
            jump_alias
        );
    }
    let jump_methods = default_auth_for(&jump_cfg);
    if jump_methods.is_empty() {
        anyhow::bail!("no auth methods for jump host '{jump_alias}'");
    }
    let jump_handle =
        connect_direct_and_auth(&jump_cfg, jump_methods, event_tx, registry.clone()).await?;

    // 2) 在跳板会话上 direct_tcpip 到 target:port。
    let channel = jump_handle
        .channel_open_direct_tcpip(
            target_host.clone(),
            target_port as u32,
            "0.0.0.0".to_string(),
            0,
        )
        .await?;
    let stream = channel.into_stream();

    // 3) 在该流上跑第二个 SSH client（目标的 host key / 认证仍走反应式 UI）。
    let target_cfg = Arc::new(client::Config::default());
    let target_handler = ClientHandler::new(target_host, target_port, event_tx.clone(), registry);
    let target_handle = connect_stream(target_cfg, stream, target_handler).await?;
    Ok((target_handle, jump_handle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_jump_targets_share_the_main_ssh_target_parser() {
        let config = SshConfig::default();
        let jump = resolve_jump(&config, "alice@jump.example:2222");
        assert_eq!(jump.user.as_deref(), Some("alice"));
        assert_eq!(jump.effective_host(), "jump.example");
        assert_eq!(jump.effective_port(), 2222);

        let ipv6 = resolve_jump(&config, "ops@[2001:db8::7]:2200");
        assert_eq!(ipv6.user.as_deref(), Some("ops"));
        assert_eq!(ipv6.effective_host(), "2001:db8::7");
        assert_eq!(ipv6.effective_port(), 2200);
    }

    #[test]
    fn configured_jump_alias_uses_resolved_values() {
        let config = SshConfig {
            hosts: vec![HostConfig {
                aliases: vec!["jump".into()],
                host_name: Some("10.0.0.5".into()),
                user: Some("deploy".into()),
                port: Some(2022),
                identity_files: Vec::new(),
                identities_only: None,
                proxy_jump: None,
                local_forwards: Vec::new(),
                remote_forwards: Vec::new(),
                dynamic_forwards: Vec::new(),
            }],
        };
        let jump = resolve_jump(&config, "jump");
        assert_eq!(jump.effective_host(), "10.0.0.5");
        assert_eq!(jump.user.as_deref(), Some("deploy"));
        assert_eq!(jump.effective_port(), 2022);
    }
}

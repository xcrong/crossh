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
    let known = config.hosts().iter().any(|h| h.matches(alias));
    if known {
        config.resolve(alias)
    } else {
        parse_explicit(alias)
    }
}

/// 解析显式 `[user@]host[:port]`。
fn parse_explicit(s: &str) -> HostConfig {
    let (user, rest) = match s.split_once('@') {
        Some((u, r)) => (Some(u.to_string()), r),
        None => (None, s),
    };
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.parse::<u16>().ok()),
        _ => (rest.to_string(), None),
    };
    HostConfig {
        aliases: vec![s.to_string()],
        host_name: Some(host),
        user,
        port,
        identity_files: Vec::new(),
        identities_only: None,
        proxy_jump: None,
        local_forwards: Vec::new(),
        remote_forwards: Vec::new(),
        dynamic_forwards: Vec::new(),
    }
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

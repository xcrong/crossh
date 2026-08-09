//! 端口转发：-L（本地）/ -D（动态 SOCKS5）/ -R（远端）。
//!
//! - -L / -D：本地 TcpListener，每接入一条连接 → `channel_open_direct_tcpip`
//!   到目标 → 双向流式 relay。
//! - -R：`tcpip_forward` 请求服务端监听；入站 forwarded-tcpip channel 由
//!   `ClientHandler` 路由（查 `RemoteForwardRegistry`）→ relay 到本地目标。
//!
//! Handle 的 channel/direct_tcpip/disconnect 均为 `&self`，故 auth 后用 `Arc<Handle>`
//! 共享给各 listener 任务。

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::Arc;

use russh::Channel;
use russh::client::{Handle, Msg};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, oneshot};

use crossh_core::config::ForwardSpec;

use super::connection::ClientHandler;

/// 远端转发注册表：connected_port → 本地 (host, port)。
/// ClientHandler 在收到入站 forwarded-tcpip 时查表，决定 relay 到哪个本地目标。
pub(crate) type RemoteForwardRegistry = Arc<Mutex<HashMap<u32, (String, u16)>>>;

pub(crate) fn new_remote_forward_registry() -> RemoteForwardRegistry {
    Arc::new(Mutex::new(HashMap::new()))
}

/// 解析 listen 规约：`8080` → (127.0.0.1, 8080)；`host:port` → (host, port)。
pub(crate) fn parse_listen(s: &str) -> (String, u16) {
    if let Ok(port) = s.parse::<u16>() {
        return ("127.0.0.1".to_string(), port);
    }
    if let Some((h, p)) = s.rsplit_once(':')
        && let Ok(port) = p.parse::<u16>()
    {
        return (h.to_string(), port);
    }
    ("127.0.0.1".to_string(), 0)
}

/// 解析远端目标 `host:port`。
pub(crate) fn parse_remote(s: &str) -> Option<(String, u16)> {
    let (h, p) = s.rsplit_once(':')?;
    let port = p.parse::<u16>().ok()?;
    Some((h.to_string(), port))
}

/// -L：本地 listener → direct_tcpip 到 spec.remote。
pub(crate) async fn run_local_forward(
    handle: Arc<Handle<ClientHandler>>,
    spec: ForwardSpec,
    mut stop: oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    let (bind_addr, bind_port) = parse_listen(&spec.listen);
    let (target_host, target_port) = parse_remote(&spec.remote)
        .ok_or_else(|| anyhow::anyhow!("invalid remote: {}", spec.remote))?;

    let listener = TcpListener::bind((bind_addr.as_str(), bind_port)).await?;
    log::info!("local forward listening on {}:{}", bind_addr, bind_port);

    loop {
        tokio::select! {
            _ = &mut stop => break,
            res = listener.accept() => {
                let (tcp, peer) = match res {
                    Ok(v) => v,
                    Err(e) => { log::warn!("accept forward: {e}"); continue; }
                };
                let h = handle.clone();
                let th = target_host.clone();
                let tp = target_port;
                tokio::spawn(async move {
                    if let Err(e) = relay_local_to_remote(h, tcp, &th, tp, &peer.to_string()).await {
                        log::debug!("forward relay ended: {e}");
                    }
                });
            }
        }
    }
    Ok(())
}

async fn relay_local_to_remote(
    handle: Arc<Handle<ClientHandler>>,
    tcp: TcpStream,
    target_host: &str,
    target_port: u16,
    originator: &str,
) -> anyhow::Result<()> {
    let channel = handle
        .channel_open_direct_tcpip(
            target_host.to_string(),
            target_port as u32,
            originator.to_string(),
            0,
        )
        .await?;
    relay_channel_tcp(channel, tcp).await
}

/// -D：本地 SOCKS5 listener → 解析目标 → direct_tcpip。
pub(crate) async fn run_dynamic_forward(
    handle: Arc<Handle<ClientHandler>>,
    spec: ForwardSpec,
    mut stop: oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    let (bind_addr, bind_port) = parse_listen(&spec.listen);
    let listener = TcpListener::bind((bind_addr.as_str(), bind_port)).await?;
    log::info!(
        "dynamic forward (SOCKS5) listening on {}:{}",
        bind_addr,
        bind_port
    );

    loop {
        tokio::select! {
            _ = &mut stop => break,
            res = listener.accept() => {
                let (tcp, peer) = match res {
                    Ok(v) => v,
                    Err(e) => { log::warn!("accept socks: {e}"); continue; }
                };
                let h = handle.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_socks5(h, tcp).await {
                        log::debug!("socks relay ended: {e}");
                    }
                    let _ = peer;
                });
            }
        }
    }
    Ok(())
}

/// 最小 SOCKS5 握手（无认证，CONNECT，支持 IPv4/域名），随后 direct_tcpip + relay。
async fn handle_socks5(
    handle: Arc<Handle<ClientHandler>>,
    mut tcp: TcpStream,
) -> anyhow::Result<()> {
    // 方法协商。
    let mut hdr = [0u8; 2];
    tcp.read_exact(&mut hdr).await?;
    if hdr[0] != 0x05 {
        anyhow::bail!("not socks5");
    }
    let nmethods = hdr[1] as usize;
    let mut methods = vec![0u8; nmethods];
    tcp.read_exact(&mut methods).await?;
    // 回复：无需认证。
    tcp.write_all(&[0x05, 0x00]).await?;

    // 请求。
    let mut req = [0u8; 4];
    tcp.read_exact(&mut req).await?;
    if req[0] != 0x05 || req[1] != 0x01 {
        // 仅支持 CONNECT。
        tcp.write_all(&[0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
            .await?;
        anyhow::bail!("socks: only CONNECT supported");
    }
    let (host, port) = match req[3] {
        0x01 => {
            // IPv4。
            let mut ip = [0u8; 4];
            tcp.read_exact(&mut ip).await?;
            let mut p = [0u8; 2];
            tcp.read_exact(&mut p).await?;
            (Ipv4Addr::from(ip).to_string(), u16::from_be_bytes(p))
        }
        0x03 => {
            // 域名。
            let mut len = [0u8; 1];
            tcp.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            tcp.read_exact(&mut name).await?;
            let mut p = [0u8; 2];
            tcp.read_exact(&mut p).await?;
            (
                String::from_utf8_lossy(&name).to_string(),
                u16::from_be_bytes(p),
            )
        }
        _ => {
            tcp.write_all(&[0x05, 0x08, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await?;
            anyhow::bail!("socks: unsupported ATYP");
        }
    };

    let channel = match handle
        .channel_open_direct_tcpip(host.clone(), port as u32, "0.0.0.0".to_string(), 0)
        .await
    {
        Ok(c) => c,
        Err(e) => {
            tcp.write_all(&[0x05, 0x01, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await?;
            return Err(e.into());
        }
    };
    // 成功回复。
    tcp.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await?;
    relay_channel_tcp(channel, tcp).await
}

/// -R：请求服务端监听，登记注册表（供 Handler 路由入站）。
/// 返回服务端分配/确认的端口。
pub(crate) async fn start_remote_forward(
    handle: &Handle<ClientHandler>,
    registry: RemoteForwardRegistry,
    spec: ForwardSpec,
) -> anyhow::Result<u32> {
    let (bind_addr, bind_port) = parse_listen(&spec.listen);
    let (local_host, local_port) = parse_remote(&spec.remote)
        .ok_or_else(|| anyhow::anyhow!("invalid remote: {}", spec.remote))?;

    // 服务端监听 bind:bind_port；返回实际端口（若传入 0 则服务端分配）。
    let allocated = handle
        .tcpip_forward(bind_addr.clone(), bind_port as u32)
        .await?;
    registry
        .lock()
        .await
        .insert(allocated, (local_host.clone(), local_port));
    log::info!(
        "remote forward registered: {bind_addr}:{allocated} -> local {local_host}:{local_port}"
    );
    Ok(allocated)
}

/// -R 停止：取消监听 + 清注册表。
pub(crate) async fn stop_remote_forward(
    handle: &Handle<ClientHandler>,
    registry: RemoteForwardRegistry,
    spec: &ForwardSpec,
    allocated: u32,
) {
    let (bind_addr, _bind_port) = parse_listen(&spec.listen);
    let _ = handle.cancel_tcpip_forward(bind_addr, allocated).await;
    registry.lock().await.remove(&allocated);
}

/// Handler 侧：处理入站 forwarded-tcpip channel → relay 到本地目标。
pub(crate) async fn handle_forwarded_tcpip(
    channel: Channel<Msg>,
    connected_port: u32,
    registry: &RemoteForwardRegistry,
) {
    let target = registry.lock().await.get(&connected_port).cloned();
    let Some((local_host, local_port)) = target else {
        log::warn!("forwarded-tcpip on unregistered port {connected_port}; dropping");
        return;
    };
    let mut tcp = match TcpStream::connect((local_host.as_str(), local_port)).await {
        Ok(t) => t,
        Err(e) => {
            log::warn!("connect local {local_host}:{local_port}: {e}");
            return;
        }
    };
    let _ = &mut tcp;
    if let Err(e) = relay_channel_tcp(channel, tcp).await {
        log::debug!("remote-forward relay ended: {e}");
    }
}

/// 双向流式 relay：SSH channel stream ↔ TCP。
async fn relay_channel_tcp(channel: Channel<Msg>, tcp: TcpStream) -> anyhow::Result<()> {
    let stream = channel.into_stream();
    let (mut s_r, mut s_w) = tokio::io::split(stream);
    let (mut t_r, mut t_w) = tokio::io::split(tcp);

    let c2s = async {
        tokio::io::copy(&mut s_r, &mut t_w).await?;
        let _ = t_w.shutdown().await;
        Ok::<_, std::io::Error>(())
    };
    let s2c = async {
        tokio::io::copy(&mut t_r, &mut s_w).await?;
        let _ = s_w.shutdown().await;
        Ok::<_, std::io::Error>(())
    };
    let _ = tokio::try_join!(c2s, s2c);
    Ok(())
}

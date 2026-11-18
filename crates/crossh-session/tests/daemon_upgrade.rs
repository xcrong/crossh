//! 升级恢复：旧版本 daemon 仍常驻时，`ensure_daemon` 必须自动停旧启新。
//!
//! 用一个只接受 v1 握手的假 daemon 占住 `daemon.port`，再调用
//! `ensure_daemon`：客户端应按旧版本握手发 `Shutdown`，等旧端口释放后
//! 拉起当前版本的 `crossh-daemon`。测试独占 `CROSSH_CACHE_DIR` /
//! `CROSSH_CONFIG_DIR` / `CROSSH_DAEMON_BIN`，与其他测试二进制隔离。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossh_session::DaemonState;
use crossh_session::handshake::{ErrorPayload, HandshakeOk, HandshakeRequest};
use crossh_session::protocol::{MessageKind, decode_frame, encode_frame};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const OLD_VERSION: u32 = 1;

async fn read_frame(
    stream: &mut (impl tokio::io::AsyncRead + Unpin),
) -> std::io::Result<(MessageKind, Vec<u8>)> {
    let mut head = [0u8; 5];
    stream.read_exact(&mut head).await?;
    let len = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
    let mut rest = vec![0u8; len.saturating_sub(1)];
    stream.read_exact(&mut rest).await?;
    let mut full = Vec::with_capacity(4 + len);
    full.extend_from_slice(&head);
    full.extend_from_slice(&rest);
    decode_frame(&full)
}

async fn write_frame(
    stream: &mut (impl tokio::io::AsyncWrite + Unpin),
    kind: MessageKind,
    payload: &[u8],
) -> std::io::Result<()> {
    stream.write_all(&encode_frame(kind, payload)?).await
}

fn json<T: serde::Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

/// 假旧 daemon：拒绝非 v1 握手；收到 v1 `Shutdown` 后删端口文件并退出。
async fn serve_old_daemon(
    listener: tokio::net::TcpListener,
    port_file: PathBuf,
    stopped: Arc<AtomicBool>,
) {
    loop {
        let Ok((mut sock, _)) = listener.accept().await else {
            break;
        };
        let Ok((kind, payload)) = read_frame(&mut sock).await else {
            continue;
        };
        if kind != MessageKind::Hello {
            continue;
        }
        let Ok(hello) = serde_json::from_slice::<HandshakeRequest>(&payload) else {
            continue;
        };
        if hello.version != OLD_VERSION {
            let error = ErrorPayload {
                code: "protocol".to_string(),
                message: format!(
                    "protocol version mismatch (daemon {OLD_VERSION}, client {}); run `crossh daemon stop` then retry",
                    hello.version
                ),
            };
            let _ = write_frame(&mut sock, MessageKind::Error, &json(&error)).await;
            continue;
        }
        let ok = HandshakeOk {
            version: OLD_VERSION,
            capabilities: vec![],
            client_id: 1,
        };
        if write_frame(&mut sock, MessageKind::Welcome, &json(&ok))
            .await
            .is_err()
        {
            continue;
        }
        // 探测连接读完 Welcome 就断开；停止连接会继续发 Shutdown。
        if let Ok((MessageKind::Shutdown, _)) = read_frame(&mut sock).await {
            let _ = std::fs::remove_file(&port_file);
            stopped.store(true, Ordering::SeqCst);
            break;
        }
    }
}

struct Cleanup {
    cache: PathBuf,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        // 异步停下测试拉起的 daemon；Drop 在 tokio 测试运行时里，
        // 新建 runtime 必须放到独立线程。
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let _ = rt.block_on(crossh_session::daemon_stop());
        })
        .join();
        let _ = std::fs::remove_dir_all(&self.cache);
    }
}

fn read_port_file(path: &Path) -> Option<(u16, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let port = lines.next()?.trim().parse().ok()?;
    let token = lines.next()?.trim().to_string();
    Some((port, token))
}

async fn wait_for_port_file(path: &Path) -> (u16, String) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Some(entry) = read_port_file(path) {
            return entry;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("daemon.port never appeared");
}

#[tokio::test]
async fn incompatible_daemon_is_stopped_and_replaced() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let cache = std::env::temp_dir().join(format!(
        "crossh-daemon-upgrade-{}-{stamp}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&cache);
    let config = cache.join("config");
    std::fs::create_dir_all(&config).unwrap();
    // SAFETY: 本测试二进制只有这一个测试，设置后只读。
    unsafe {
        std::env::set_var("CROSSH_CACHE_DIR", &cache);
        std::env::set_var("CROSSH_CONFIG_DIR", &config);
        std::env::set_var("CROSSH_DAEMON_BIN", env!("CARGO_BIN_EXE_crossh-daemon"));
    }
    let _cleanup = Cleanup {
        cache: cache.clone(),
    };

    let daemon_dir = cache.join("daemon");
    std::fs::create_dir_all(&daemon_dir).unwrap();
    let port_file = daemon_dir.join("daemon.port");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let old_port = listener.local_addr().unwrap().port();
    let stopped = Arc::new(AtomicBool::new(false));
    let fake = tokio::spawn(serve_old_daemon(
        listener,
        port_file.clone(),
        stopped.clone(),
    ));
    std::fs::write(&port_file, format!("{old_port}\nold-token\n")).unwrap();

    let addr = crossh_session::ensure_daemon()
        .await
        .expect("ensure_daemon must replace the incompatible daemon");

    assert!(
        stopped.load(Ordering::SeqCst),
        "old daemon must receive Shutdown before the new one starts"
    );
    fake.await.unwrap();
    assert_ne!(addr.port, old_port, "must spawn a new daemon");
    assert!(
        matches!(crossh_session::probe_daemon().await, DaemonState::Ready(_)),
        "new daemon must answer the current protocol"
    );
    let (current_port, _) = wait_for_port_file(&port_file).await;
    assert_eq!(current_port, addr.port);
}

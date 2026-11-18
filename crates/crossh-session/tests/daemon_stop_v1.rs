//! `crossh daemon stop` 对 v1 daemon 的正向用例（7.5）。
//!
//! 假 daemon 只接受 v1 握手；客户端先用当前版本探测拿到版本错误，
//! 再按 v1 重试并发送 `Shutdown`。断言旧实例收到停止、端口文件被删除、
//! 不会拉起第二个实例。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossh_session::DaemonState;
use crossh_session::handshake::{ErrorPayload, HandshakeOk, HandshakeRequest};
use crossh_session::instance::daemon_port_file_path;
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
        if let Ok((MessageKind::Shutdown, _)) = read_frame(&mut sock).await {
            let _ = std::fs::remove_file(&port_file);
            stopped.store(true, Ordering::SeqCst);
            break;
        }
    }
}

fn temp_root() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("crossh-stop-v1-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn daemon_stop_retries_v1_handshake_and_removes_the_port_file() {
    let root = temp_root();
    let config = root.join("config");
    std::fs::create_dir_all(&config).unwrap();
    // SAFETY: 本测试二进制只有这一个测试，设置后只读。
    unsafe {
        std::env::set_var("CROSSH_CACHE_DIR", &root);
        std::env::set_var("CROSSH_CONFIG_DIR", &config);
    }

    let port_file = daemon_port_file_path();
    std::fs::create_dir_all(port_file.parent().unwrap()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    std::fs::write(&port_file, format!("{port}\nold-token\n")).unwrap();

    let stopped = Arc::new(AtomicBool::new(false));
    let fake = tokio::spawn(serve_old_daemon(
        listener,
        port_file.clone(),
        stopped.clone(),
    ));

    crossh_session::daemon_stop()
        .await
        .expect("daemon_stop must retry with the reported v1 handshake");
    assert!(
        stopped.load(Ordering::SeqCst),
        "v1 daemon must receive Shutdown"
    );
    tokio::time::timeout(Duration::from_secs(5), fake)
        .await
        .expect("fake daemon exits")
        .unwrap();
    assert!(
        !port_file.exists(),
        "port file belongs to the stopped daemon"
    );
    assert!(matches!(
        crossh_session::probe_daemon().await,
        DaemonState::Absent { stale: false }
    ));
    let second = crossh_session::daemon_stop().await.unwrap_err();
    assert_eq!(second.kind(), std::io::ErrorKind::NotFound);

    let _ = std::fs::remove_dir_all(root);
}

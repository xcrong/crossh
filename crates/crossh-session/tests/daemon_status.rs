//! `crossh daemon status` 的只读探测（7.6）：三态判定不产生副作用。
//!
//! 覆盖：无端口文件、陈旧端口文件、协议不兼容、当前版本四种情况；
//! 断言探测不删除端口文件、不写入 `daemon.error`、也不拉起 daemon。

use std::path::PathBuf;
use std::time::Duration;

use crossh_session::DaemonState;
use crossh_session::handshake::{ErrorPayload, HandshakeOk, HandshakeRequest, PROTOCOL_VERSION};
use crossh_session::instance::daemon_port_file_path;
use crossh_session::protocol::{MessageKind, decode_frame, encode_frame};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

/// 单连接假 daemon：按 `reply_version` 回 Welcome 或版本不匹配错误。
async fn serve_once(listener: tokio::net::TcpListener, reply_version: u32) {
    let Ok((mut sock, _)) = listener.accept().await else {
        return;
    };
    let Ok((kind, payload)) = read_frame(&mut sock).await else {
        return;
    };
    if kind != MessageKind::Hello {
        return;
    }
    let Ok(hello) = serde_json::from_slice::<HandshakeRequest>(&payload) else {
        return;
    };
    if hello.version != reply_version {
        let error = ErrorPayload {
            code: "protocol".to_string(),
            message: format!(
                "protocol version mismatch (daemon {reply_version}, client {}); run `crossh daemon stop` then retry",
                hello.version
            ),
        };
        let _ = write_frame(&mut sock, MessageKind::Error, &json(&error)).await;
        return;
    }
    let ok = HandshakeOk {
        version: reply_version,
        capabilities: vec![],
        client_id: 1,
    };
    let _ = write_frame(&mut sock, MessageKind::Welcome, &json(&ok)).await;
    // 探测连接读完 Welcome 即断开；多等一拍避免对端 RST 干扰断言。
    let _ = tokio::time::timeout(Duration::from_millis(200), sock.read(&mut [0u8; 1])).await;
}

fn temp_root() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("crossh-status-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn fake_daemon(reply_version: u32) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    (port, tokio::spawn(serve_once(listener, reply_version)))
}

// 同一二进制内用 env 隔离：两用例串行拿锁，避免 CROSSH_CACHE_DIR 竞态。
// 用 tokio::sync::Mutex，守卫可持跨 await；std MutexGuard 跨 await 会触发 clippy::await_holding_lock。
async fn env_lock() -> tokio::sync::MutexGuard<'static, ()> {
    use std::sync::OnceLock;
    static L: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    L.get_or_init(|| tokio::sync::Mutex::new(())).lock().await
}

/// 只回非版本错误帧的假 listener：覆盖握手失败分支（超时/令牌错/欢迎帧损坏同路）。
async fn serve_handshake_failure(listener: tokio::net::TcpListener) {
    loop {
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };
        let Ok((kind, _)) = read_frame(&mut sock).await else {
            continue;
        };
        if kind != MessageKind::Hello {
            continue;
        };
        let error = ErrorPayload {
            code: "auth".to_string(),
            message: "bad token".to_string(),
        };
        let _ = write_frame(&mut sock, MessageKind::Error, &json(&error)).await;
    }
}

#[tokio::test]
async fn probe_reports_three_states_without_side_effects() {
    let _env = env_lock().await;
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
    let error_file = port_file.parent().unwrap().join("daemon.error");

    // 1. 无端口文件：not running，且探测不创建任何文件。
    assert!(matches!(
        crossh_session::probe_daemon().await,
        DaemonState::Absent { stale: false }
    ));
    assert!(!port_file.exists(), "probe must not create the port file");
    assert!(!error_file.exists(), "probe must not write daemon.error");

    // 2. 陈旧端口文件：标记 stale，原文件保持不变。
    let stale = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stale_port = stale.local_addr().unwrap().port();
    drop(stale);
    let stale_bytes = format!("{stale_port}\ntoken\n").into_bytes();
    std::fs::write(&port_file, &stale_bytes).unwrap();
    assert!(matches!(
        crossh_session::probe_daemon().await,
        DaemonState::Absent { stale: true }
    ));
    assert_eq!(
        std::fs::read(&port_file).unwrap(),
        stale_bytes,
        "probe must not delete or rewrite the stale port file"
    );

    // 3. 协议不兼容：报告版本不匹配，端口文件保持不变。
    let (old_port, old_server) = fake_daemon(PROTOCOL_VERSION.saturating_sub(1)).await;
    let incompatible_bytes = format!("{old_port}\ntoken\n").into_bytes();
    std::fs::write(&port_file, &incompatible_bytes).unwrap();
    match crossh_session::probe_daemon().await {
        DaemonState::Incompatible { message } => {
            assert!(message.contains("protocol version mismatch"));
            assert!(message.contains("crossh daemon stop"));
        }
        other => panic!("expected incompatible, got {other:?}"),
    }
    old_server.await.unwrap();
    assert_eq!(
        std::fs::read(&port_file).unwrap(),
        incompatible_bytes,
        "probe must not delete or rewrite the incompatible port file"
    );

    // 4. 当前版本：Ready，端口与端口文件一致。
    let (port, server) = fake_daemon(PROTOCOL_VERSION).await;
    std::fs::write(&port_file, format!("{port}\ntoken\n")).unwrap();
    match crossh_session::probe_daemon().await {
        DaemonState::Ready(addr) => {
            assert_eq!(addr.port, port);
            assert_eq!(addr.token, "token");
        }
        other => panic!("expected ready, got {other:?}"),
    }
    server.await.unwrap();
    assert!(!error_file.exists(), "probe must not write daemon.error");

    let _ = std::fs::remove_dir_all(root);
}

/// 握手失败不断定陈旧：只回错误帧不断 Welcome，探测为 Absent{stale:false}，
/// 端口文件逐字节不变；ensure 直接报错，不删文件、不拉新 daemon。
#[tokio::test]
async fn handshake_failure_is_absent_not_stale_without_side_effects() {
    let _env = env_lock().await;
    let root = temp_root();
    let config = root.join("config");
    std::fs::create_dir_all(&config).unwrap();
    // SAFETY: 持 env 锁期间独占设置，只读使用。
    unsafe {
        std::env::set_var("CROSSH_CACHE_DIR", &root);
        std::env::set_var("CROSSH_CONFIG_DIR", &config);
    }
    let port_file = daemon_port_file_path();
    std::fs::create_dir_all(port_file.parent().unwrap()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(serve_handshake_failure(listener));
    let before = format!("{port}\ntoken\n").into_bytes();
    std::fs::write(&port_file, &before).unwrap();
    assert!(matches!(
        crossh_session::probe_daemon().await,
        DaemonState::Absent { stale: false }
    ));
    assert_eq!(std::fs::read(&port_file).unwrap(), before);
    let err = crossh_session::ensure_daemon().await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused);
    assert_eq!(std::fs::read(&port_file).unwrap(), before);
    server.abort();
    let _ = std::fs::remove_dir_all(root);
}

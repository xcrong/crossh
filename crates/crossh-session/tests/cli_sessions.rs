//! CLI 闭环集成测试（5.4）：按需拉起 → `run` 拿输出与退出码 → 双前端一致 → 停服。
//!
//! 走 `crossh_session::client` 高层 API（与根二进制 `crossh run/...` 同一份实现）。

use std::io;
use std::time::Duration;

use crossh_session::{Client, capture_session, daemon_stop, ensure_daemon, role_of, run_command};

fn cache_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("crossh-cli-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(unix)]
fn exit7() -> Vec<String> {
    vec!["sh".to_string(), "-c".to_string(), "exit 7".to_string()]
}
#[cfg(windows)]
fn exit7() -> Vec<String> {
    vec!["cmd".to_string(), "/C".to_string(), "exit 7".to_string()]
}
#[cfg(unix)]
fn echo_hi() -> Vec<String> {
    vec!["echo".to_string(), "hi-cli".to_string()]
}
#[cfg(windows)]
fn echo_hi() -> Vec<String> {
    vec![
        "cmd".to_string(),
        "/C".to_string(),
        "echo hi-cli".to_string(),
    ]
}

#[tokio::test]
async fn cli_full_cycle() {
    let dir = cache_dir();
    // SAFETY: 测试进程内最早设置，后续只读。
    unsafe {
        std::env::set_var("CROSSH_CACHE_DIR", &dir);
        std::env::set_var("CROSSH_DAEMON_BIN", env!("CARGO_BIN_EXE_crossh-daemon"));
    }

    struct Guard {
        dir: std::path::PathBuf,
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    let _guard = Guard { dir: dir.clone() };

    // 未运行时直接 `run`：按需拉起成功且退出码为 0（5.2）。
    let mut out = Vec::new();
    let code = run_command(&echo_hi(), &mut out).await.unwrap();
    assert_eq!(code, 0);
    assert!(
        String::from_utf8_lossy(&out).contains("hi-cli"),
        "got {:?}",
        String::from_utf8_lossy(&out)
    );

    // 退出码原样透出。
    let mut out = Vec::new();
    let code = run_command(&exit7(), &mut out).await.unwrap();
    assert_eq!(code, 7);

    // 双前端看到同一段输出与角色（规格“两个前端的观察一致”）。
    let mut c1 = Client::connect().await.unwrap();
    let attached = c1.create_shell(None, 80, 24).await.unwrap();
    let sid = attached.session.id.clone();
    c1.input(b"echo shared-view\n").await.unwrap();
    let mut c2 = Client::connect().await.unwrap();
    let attached2 = c2.attach(&sid).await.unwrap();
    assert_eq!(attached2.session.controller, Some(c1.client_id));
    assert_eq!(role_of(&attached2.session, c1.client_id), "controller");
    assert_eq!(role_of(&attached2.session, c2.client_id), "observer");
    // c1 读到自己的回显。
    let mut seen = attached.replay.clone();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !String::from_utf8_lossy(&seen).contains("shared-view") {
        assert!(std::time::Instant::now() < deadline, "c1 never saw output");
        match c1.next_event().await.unwrap() {
            crossh_session::ServerEvent::Output { bytes, .. } => seen.extend_from_slice(&bytes),
            crossh_session::ServerEvent::Exited(_) => panic!("session exited early"),
        }
    }
    // c2 在回放上限内看到同一段输出。
    let mut joined = attached2.replay.clone();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !String::from_utf8_lossy(&joined).contains("shared-view") {
        assert!(std::time::Instant::now() < deadline, "c2 never saw output");
        match c2.next_event().await.unwrap() {
            crossh_session::ServerEvent::Output { bytes, .. } => joined.extend_from_slice(&bytes),
            crossh_session::ServerEvent::Exited(_) => break,
        }
    }
    // capture 读到同一段输出。
    let mut captured = Vec::new();
    let info = capture_session(&sid, &mut captured).await.unwrap();
    assert_eq!(info.id, sid);
    assert!(String::from_utf8_lossy(&captured).contains("shared-view"));
    // 结束会话后列表为空。
    c1.close(&sid).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(c1.list().await.unwrap().is_empty());

    // 显式停止后会话消失，且下次请求重新拉起（规格“显式停止”）。
    daemon_stop().await.unwrap();
    let addr = ensure_daemon().await.unwrap();
    let probe = tokio::net::TcpStream::connect(("127.0.0.1", addr.port)).await;
    assert!(probe.is_ok(), "daemon relaunched on demand");
    daemon_stop().await.unwrap();
}

/// `run --` 无参数给出用法错误而不是 panic。
#[tokio::test]
async fn run_empty_is_usage_error() {
    let mut out = Vec::new();
    let err = run_command(&[], &mut out).await.unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

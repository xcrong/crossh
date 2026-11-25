//! 启动保护（7.5）：`format_version` 过新时 daemon 拒绝启动。
//!
//! 断言顺序：文件逐字节不变 → 写入 `daemon.error` → 非零退出 →
//! 不发布端口文件 → `probe_daemon` 看到 `Absent` 且能读到失败原因。

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crossh_session::DaemonState;
use crossh_session::instance::daemon_port_file_path;

fn temp_root() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("crossh-startup-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_daemon(config: &Path, cache: &Path) -> std::process::ExitStatus {
    let mut child: Child = Command::new(env!("CARGO_BIN_EXE_crossh-daemon"))
        .env("CROSSH_CONFIG_DIR", config)
        .env("CROSSH_CACHE_DIR", cache)
        .env_remove("CROSSH_DAEMON_BIN")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn crossh-daemon");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().expect("wait for crossh-daemon") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("crossh-daemon did not exit for a too-new workspace");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[tokio::test]
#[serial_test::file_serial(crossh_env)]
async fn too_new_workspace_refuses_start_without_rewriting_it() {
    let root = temp_root();
    let config = root.join("config");
    let cache = root.join("cache");
    let daemon_dir = config.join("daemon");
    std::fs::create_dir_all(&daemon_dir).unwrap();
    std::fs::create_dir_all(&cache).unwrap();
    // SAFETY: 本测试二进制只有这一个测试，设置后只读。
    unsafe {
        std::env::set_var("CROSSH_CONFIG_DIR", &config);
        std::env::set_var("CROSSH_CACHE_DIR", &cache);
    }

    let workspace_path = daemon_dir.join("workspace.json");
    let too_new = br#"{"format_version":999}"#.to_vec();
    std::fs::write(&workspace_path, &too_new).unwrap();

    let status = run_daemon(&config, &cache);
    assert!(!status.success(), "daemon must reject a too-new workspace");
    assert_eq!(
        std::fs::read(&workspace_path).unwrap(),
        too_new,
        "format guard must not rewrite the workspace file"
    );

    let error_path = daemon_dir.join("daemon.error");
    let reason = std::fs::read_to_string(&error_path).expect("daemon.error written");
    assert!(
        reason.contains("newer than supported"),
        "daemon.error must explain the format guard: {reason}"
    );
    assert!(
        !daemon_port_file_path().exists(),
        "daemon must not publish a port file when startup fails"
    );
    assert!(matches!(
        crossh_session::probe_daemon().await,
        DaemonState::Absent { stale: false }
    ));
    assert_eq!(
        crossh_session::workspace_store::read_daemon_error().as_deref(),
        Some(reason.trim())
    );

    let _ = std::fs::remove_dir_all(root);
}

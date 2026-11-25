//! `crossh-daemon` 装配入口：日志、locale、tokio 运行时、`run()`。
//!
//! 只做装配；协议/会话/单实例逻辑在库里（`crossh_session`）。

use crossh_session::Claim;
use crossh_session::claim_instance;

fn version_string() -> String {
    format!("crossh-daemon {}", env!("CARGO_PKG_VERSION"))
}

fn main() -> std::io::Result<()> {
    // `--version` 先行：不拉起日志/运行时。
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("{}", version_string());
        return Ok(());
    }
    // runtime 启动前设置进程环境：此时仍是单线程，无并发读写 env 竞态。
    set_locale();
    crossh_core::logging::init();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

fn set_locale() {
    if std::env::var_os("LANG").is_none() {
        // SAFETY: main 入口建 tokio runtime 之前调用，仍是单线程，无其他线程读写 env。
        unsafe { std::env::set_var("LANG", "en_US.UTF-8") };
    }
}

async fn run() -> std::io::Result<()> {
    let token = new_token();
    let store = crossh_session::workspace_store::WorkspaceStore::default_store()
        .ok_or_else(|| std::io::Error::other("no config root"))?;
    // 格式保护必须发生在任何写入与端口发布之前。
    let workspace = match store.load() {
        Ok(workspace) => workspace,
        Err(error) => {
            let reason = error.to_string();
            crossh_session::workspace_store::write_daemon_error(&reason);
            log::error!("{reason}");
            return Err(std::io::Error::other(reason));
        }
    };
    match claim_instance(&token)? {
        Claim::Owner { listener } => {
            log::info!("crossh-daemon owner on {}", listener.local_addr()?);
            let mut workspace = workspace;
            crossh_session::workspace_store::migrate_legacy(&mut workspace);
            crossh_session::workspace_store::clear_daemon_error();
            crossh_session::serve::run(
                tokio::net::TcpListener::from_std(listener).expect("tokio listener"),
                token,
                workspace,
                std::sync::Arc::new(store),
            )
            .await;
        }
        Claim::Existing(_addr) => {
            // 已有属主：直接退出，调用方接入既有实例（单实例语义）。
            // 不得删除属主的端口文件。
            Ok(())
        }
    }
}

fn new_token() -> String {
    // ponytail: 令牌只需同机不可猜；时间+pid+随机数拼 32 hex，不引新依赖。
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut h = DefaultHasher::new();
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut h);
    std::process::id().hash(&mut h);
    // 读一点系统随机性（失败则退化为哈希链）。
    let mut extra = [0u8; 8];
    #[cfg(unix)]
    {
        use std::io::Read;
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            let _ = f.read_exact(&mut extra);
        }
    }
    extra.hash(&mut h);
    format!(
        "{:016x}{:016x}",
        h.finish(),
        extra.iter().fold(0u64, |a, b| a * 31 + *b as u64)
    )
}

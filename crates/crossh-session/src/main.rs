//! `crossh-daemon` 装配入口：日志、locale、tokio 运行时、`run()`。
//!
//! 只做装配；协议/会话/单实例逻辑在库里（`crossh_session`）。

use crossh_session::{Claim, claim_instance, remove_port_file};

fn version_string() -> String {
    format!("crossh-daemon {}", env!("CARGO_PKG_VERSION"))
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    // `--version` 先行：不拉起日志/监听。
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("{}", version_string());
        return Ok(());
    }
    crossh_core::logging::init();
    set_locale();
    run().await
}

fn set_locale() {
    if std::env::var_os("LANG").is_none() {
        // SAFETY: 启动早期单线程设置进程环境。
        unsafe { std::env::set_var("LANG", "en_US.UTF-8") };
    }
}

async fn run() -> std::io::Result<()> {
    let token = new_token();
    match claim_instance(&token)? {
        Claim::Owner { listener } => {
            log::info!("crossh-daemon owner on {}", listener.local_addr()?);
            crossh_session::serve::run(
                tokio::net::TcpListener::from_std(listener).expect("tokio listener"),
                token,
            )
            .await;
        }
        Claim::Existing(_addr) => {
            // 已有属主：直接退出，调用方接入既有实例（单实例语义）。
        }
    }
    remove_port_file();
    Ok(())
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

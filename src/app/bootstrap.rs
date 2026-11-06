//! Application bootstrap: process-wide logging and tokio runtime.
//!
//! Extracted from `src/infrastructure/{logging,runtime}.rs` to keep `main.rs`
//! thin while co-locating startup concerns under `app::`.
//!
//! 日志本体（路径、裁剪、tee 到文件）与 panic hook 在 `crossh_core::logging`：
//! 独立子进程 `crossh-note` 也要用同一份实现。

use std::sync::LazyLock;

use tokio::runtime::Runtime;

static RT: LazyLock<Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("crossh-runtime")
        .build()
        .expect("failed to build tokio runtime")
});

/// 全局 tokio Runtime 引用。
pub(crate) fn runtime() -> &'static Runtime {
    &RT
}

/// Initialize logging and the process panic hook (both live in `crossh_core::logging`,
/// so the standalone `crossh-note` process gets them too).
pub(crate) fn init() {
    crossh_core::logging::init();
    log::info!("---- crossh startup (pid {}) ----", std::process::id());
}

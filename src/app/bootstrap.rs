//! 应用启动装配：进程级日志与 tokio 运行时。
//!
//! 从 `src/infrastructure/{logging,runtime}.rs` 抽出，保持 `main.rs`
//! 轻薄，把启动相关的装配集中在 `app::` 下。
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

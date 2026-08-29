//! 全局 tokio 运行时（单例，限 2 worker 线程，控内存）。
//!
//! GPUI 自带执行器不是 tokio，更新下载等后台 I/O 需要独立 Runtime。

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
pub fn runtime() -> &'static Runtime {
    &RT
}

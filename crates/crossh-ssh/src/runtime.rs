//! 全局 tokio 运行时（单例，限制 worker 线程以控内存）。
//!
//! russh 需要 tokio，而 gpui 自带执行器不是 tokio。我们建一个独立的、限定线程数
//! 的 tokio Runtime 常驻整个进程，所有 SSH I/O 在其上跑；通过运行时无关的
//! `async_channel` 与 gpui 主线程桥接。

use std::sync::OnceLock;
use tokio::runtime::Runtime;

static RT: OnceLock<Runtime> = OnceLock::new();

/// 全局 tokio Runtime 引用。
pub fn runtime() -> &'static Runtime {
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("crossh-ssh")
            .build()
            .expect("failed to build tokio runtime")
    })
}

//! crossh —— 基于 gpui 的轻量 SSH 客户端。
//!
//! 常驻开发工具：复用 `~/.ssh/config`（只读），提供交互式终端（russh + alacritty_terminal）。
//! SFTP 与端口转发为后续阶段（见 .kilo/plans）。

mod button;
mod config;
mod ssh;
mod ui;

use gpui::App;

fn main() {
    // 初始化 env_logger（RUST_LOG=info 可看连接日志）。
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn"),
    )
    .try_init();

    // 预热 tokio 运行时（单例，限 2 worker 线程，控内存）。
    let _rt = ssh::ssh_runtime();

    gpui_platform::application().run(move |cx: &mut App| {
        cx.init_colors();
        ui::app_shell::open_main_window(cx);
    });
}

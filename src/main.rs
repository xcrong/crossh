//! crossh —— 基于 gpui 的轻量 SSH 客户端。
//!
//! 常驻开发工具：复用 `~/.ssh/config`（只读），提供交互式终端（russh + Zed terminal core）。
//! SFTP 与端口转发为后续阶段（见 .kilo/plans）。

mod app;
mod features;
mod infrastructure;
mod shared;

use gpui::{App, KeyBinding, actions};
use theme as zed_theme;

rust_i18n::i18n!("locales", fallback = "en");

actions!(crossh, [Quit]);

fn main() {
    infrastructure::logging::init();

    // 预热 tokio 运行时（单例，限 2 worker 线程，控内存）。
    let _rt = infrastructure::ssh::ssh_runtime();

    let app = gpui_platform::application().with_assets(shared::ui::assets::UiAssetSource);
    app.on_reopen(|cx| {
        // Reuse an existing window, including a hidden one. Only create a
        // window when the application has no windows left.
        if let Some(window) = cx.windows().into_iter().next() {
            let _ = window.update(cx, |_, window, _| window.activate_window());
        } else {
            app::open_main_window(cx);
        }
    });
    app.run(move |cx: &mut App| {
        cx.set_app_identity("io.crossh.app", "Crossh");
        cx.init_colors();
        // Zed terminal reads the global theme for color queries emitted by apps
        // such as Vim while processing their output.
        zed_theme::init(zed_theme::LoadThemes::JustBase, cx);
        features::settings::init(cx);
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        app::open_main_window(cx);
    });
}

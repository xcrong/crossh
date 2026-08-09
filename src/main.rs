//! crossh —— 基于 gpui 的轻量 SSH 客户端。
//!
//! 常驻开发工具：复用 `~/.ssh/config`（只读），提供基于 Zed 官方 PTY
//! 和 Crossh 本地 renderer 的交互式终端。
//! SFTP 与端口转发为后续阶段（见 .kilo/plans）。

mod app;
mod features;
mod infrastructure;
mod shared;

use assets::Assets as ZedAssets;
use gpui::{App, KeyBinding, actions};
use release_channel as zed_release_channel;
use settings as zed_settings;
use theme::LoadThemes;
use theme_settings as zed_theme_settings;

rust_i18n::i18n!("locales", fallback = "en");

actions!(crossh, [Quit]);

fn main() {
    infrastructure::logging::init();

    // 预热 tokio 运行时（单例，限 2 worker 线程，控内存）。
    let _rt = crossh_ssh::ssh_runtime();

    let app = gpui_platform::application().with_assets(crossh_ui::assets::UiAssetSource);
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
        // Initialize the settings/theme globals consumed by Zed's terminal
        // core. Crossh's product settings remain separate and are layered on
        // top in features::settings.
        let app_version =
            zed_release_channel::AppVersion::load(env!("CARGO_PKG_VERSION"), None, None);
        zed_release_channel::init(app_version, cx);
        zed_settings::init(cx);
        zed_theme_settings::init(LoadThemes::JustBase, cx);
        ZedAssets
            .load_fonts(cx)
            .expect("Zed embedded fonts should load");
        features::settings::init(cx);
        features::terminal::init(cx);
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        app::open_main_window(cx);
    });
}

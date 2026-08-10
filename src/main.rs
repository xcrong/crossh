//! crossh —— 基于 gpui 的轻量 SSH 客户端。
//!
//! 常驻开发工具：复用 `~/.ssh/config`（只读），提供基于 Zed 官方 PTY
//! 和 Crossh 本地 renderer 的交互式终端。
//! SFTP 与端口转发为后续阶段（见 .kilo/plans）。

mod agent_cli;
mod app;
mod features;
mod infrastructure;
mod shared;

use assets::Assets as ZedAssets;
use gpui::{App, KeyBinding, Menu, MenuItem, actions};
use release_channel as zed_release_channel;
use settings as zed_settings;
use theme::LoadThemes;
use theme_settings as zed_theme_settings;

rust_i18n::i18n!("locales", fallback = "en");

actions!(crossh, [Quit]);

/// 安装应用菜单。macOS 的 Cmd+Q 通常经由菜单项的 key equivalent 由 AppKit
/// 直接截获；不装菜单时该按键只能在某个聚焦窗口内被分发，零窗口时按了无效。
fn install_app_menu(cx: &mut App) {
    cx.set_menus([
        Menu::new("Crossh").items([MenuItem::action(shared::i18n::text("quit.menu"), Quit)])
    ]);
    // 无窗口时菜单触发的 Quit 走应用级全局分发，直接退出；有窗口时仍由
    // AppShell 的窗口处理器负责风险检查与清理流程。
    cx.on_action(|_: &Quit, cx| {
        if cx.active_window().is_none() {
            cx.quit();
        }
    });
}

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--help" | "-h" | "help") => {
            print_help();
            return;
        }
        Some("--version" | "-V") => {
            println!("crossh {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Some("agent") => {
            let options = match agent_cli::parse_options(args) {
                Ok(options) => options,
                Err(error) if error == "help" => {
                    agent_cli::print_help();
                    return;
                }
                Err(error) => {
                    eprintln!("crossh agent: {error}\n");
                    agent_cli::print_help();
                    std::process::exit(2);
                }
            };
            if let Err(error) =
                agent_cli::run_with_options(features::settings::load().agent, options)
            {
                eprintln!("crossh agent: {error}");
                std::process::exit(1);
            }
            return;
        }
        Some(argument) => {
            eprintln!("unknown argument: {argument}\n");
            print_help();
            std::process::exit(2);
        }
        None => {}
    }
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
        infrastructure::theme::install_crossh_theme(cx);
        ZedAssets
            .load_fonts(cx)
            .expect("Zed embedded fonts should load");
        features::settings::init(cx);
        features::terminal::init(cx);
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        install_app_menu(cx);
        app::open_main_window(cx);
    });
}

fn print_help() {
    println!(
        "Crossh {}\n\nUsage: crossh [COMMAND]\n\nCommands:\n  agent       Start the interactive coding agent\n  help        Print help\n\nOptions:\n  -h, --help     Print help\n  -V, --version  Print version",
        env!("CARGO_PKG_VERSION")
    );
}

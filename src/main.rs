//! crossh —— 基于 gpui 的轻量 SSH 客户端。
//!
//! 常驻开发工具：复用 `~/.ssh/config`（只读），提供基于 Zed 官方 PTY
//! 和 Crossh 本地 renderer 的交互式终端。
//! SFTP 与端口转发已经作为独立 feature 接入工作区。

mod agent_cli;
mod app;
mod features;
mod infrastructure;
mod shared;

use assets::Assets as ZedAssets;
use gpui::{App, actions};
use release_channel as zed_release_channel;
use settings as zed_settings;
use theme::LoadThemes;
use theme_settings as zed_theme_settings;

rust_i18n::i18n!("locales", fallback = "en");

actions!(
    crossh,
    [
        About,
        CheckForUpdates,
        CloseActiveTab,
        CloseWindow,
        Hide,
        HideOthers,
        MinimizeWindow,
        NewTerminal,
        OpenProject,
        OpenSettings,
        Quit,
        ShowAll,
        ToggleFullScreen,
        ToggleHostSidebar,
        ToggleQuickCommands,
        ToggleTimestamps,
        ZoomWindow
    ]
);

fn main() {
    let mut args = std::env::args().skip(1);
    let launch_target = match args.next().as_deref() {
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
        Some("git") => match features::git::parse_cli(
            args,
            std::env::current_dir().map_err(|error| error.to_string()),
        ) {
            Ok(features::git::GitCliCommand::Open(cwd)) => {
                if !features::git::running_as_window_process() {
                    if let Err(error) = features::git::spawn_window_process(&cwd) {
                        eprintln!("crossh git: failed to start Git Viewer: {error}");
                        std::process::exit(1);
                    }
                    return;
                }
                app::LaunchTarget::Git(cwd)
            }
            Ok(features::git::GitCliCommand::Help) => {
                features::git::print_cli_help();
                return;
            }
            Err(error) => {
                eprintln!("crossh git: {error}\n");
                features::git::print_cli_help();
                std::process::exit(2);
            }
        },
        Some(argument) => {
            eprintln!("unknown argument: {argument}\n");
            print_help();
            std::process::exit(2);
        }
        None => app::LaunchTarget::Main,
    };
    infrastructure::logging::init();

    // 预热 tokio 运行时（单例，限 2 worker 线程，控内存）。
    let _rt = crossh_ssh::ssh_runtime();

    let app = gpui_platform::application().with_assets(crossh_ui::assets::UiAssetSource);
    let reopen_target = launch_target.clone();
    app.on_reopen(move |cx| {
        // Reuse an existing window, including a hidden one. Only create a
        // window when the application has no windows left.
        if let Some(window) = cx.windows().into_iter().next() {
            let _ = window.update(cx, |_, window, _| window.activate_window());
        } else {
            app::open_launch_target(reopen_target.clone(), cx);
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
        features::git::init(cx);
        features::terminal::init(cx);
        infrastructure::app_menu::install(cx);
        app::open_launch_target(launch_target, cx);
    });
}

fn print_help() {
    println!(
        "Crossh {}\n\nUsage: crossh [COMMAND]\n\nCommands:\n  agent       Start the interactive coding agent\n  git         Open the Git Viewer for a directory\n  help        Print help\n\nOptions:\n  -h, --help     Print help\n  -V, --version  Print version",
        env!("CARGO_PKG_VERSION")
    );
}

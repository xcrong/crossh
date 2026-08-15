//! crossh —— 基于 gpui 的轻量 SSH 客户端。
//!
//! 常驻开发工具：复用 `~/.ssh/config`（只读），提供基于 Zed 官方 PTY
//! 和 Crossh 本地 renderer 的交互式终端。
//! SFTP 与端口转发已经作为独立 feature 接入工作区。

mod app;
mod features;
mod infrastructure;
mod shared;

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
            let arguments = std::env::args().skip(2).collect::<Vec<_>>();
            let code = match spawn_agent_process(&arguments) {
                Ok(code) => code,
                Err(error) => {
                    eprintln!("crossh agent: {error}");
                    std::process::exit(1);
                }
            };
            std::process::exit(code);
        }
        Some("git") => match features::git_launcher::parse_cli(
            args,
            std::env::current_dir().map_err(|error| error.to_string()),
        ) {
            Ok(features::git_launcher::GitCliCommand::Open(cwd)) => {
                if let Err(error) = features::git_launcher::spawn_git_process(&cwd) {
                    eprintln!("crossh git: failed to start crossh-git: {error}");
                    std::process::exit(1);
                }
                return;
            }
            Ok(features::git_launcher::GitCliCommand::Help) => {
                features::git_launcher::print_cli_help();
                return;
            }
            Err(error) => {
                eprintln!("crossh git: {error}\n");
                features::git_launcher::print_cli_help();
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

    let app = gpui_platform::application().with_assets(crossh_ui::assets::UiAssetSource::default());
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
        crossh_ui::assets::load_fonts(cx).expect("Crossh fonts should load");
        features::settings::init(cx);
        features::terminal::init(cx);
        infrastructure::app_menu::install(cx);
        app::open_launch_target(launch_target, cx);
    });
}

/// 委托同目录（或 PATH）的 `crossh-agent` 二进制。继承 stdio 让 TUI 直接
/// 使用当前终端，launcher 不碰 termios；透传子进程退出码，退出路径不变。
fn spawn_agent_process(arguments: &[String]) -> Result<i32, String> {
    let executable = crossh_core::process::sibling_executable("crossh-agent");
    let status = std::process::Command::new(&executable)
        .args(arguments)
        .status()
        .map_err(|error| {
            // sibling_executable 在找不到同伴二进制时回退为纯文件名（交给
            // PATH）；此时启动失败几乎一定是二进制缺失，给出可执行的指引。
            if executable.is_relative() {
                format!("{error}: crossh-agent not found next to crossh or on PATH; build it with `cargo build` or install crossh-agent alongside crossh")
            } else {
                error.to_string()
            }
        })?;
    Ok(status.code().unwrap_or(1))
}

fn print_help() {
    println!(
        "Crossh {}\n\nUsage: crossh [COMMAND]\n\nCommands:\n  agent       Start the interactive coding agent\n  git         Open the Git Viewer for a directory\n  help        Print help\n\nOptions:\n  -h, --help     Print help\n  -V, --version  Print version",
        env!("CARGO_PKG_VERSION")
    );
}

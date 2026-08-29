//! crossh —— 基于 gpui 的本地优先终端工作环境（macOS / Linux / Windows）。
//!
//! 以项目目录组织多会话本地终端为核心，Git Viewer、Note 均为可插拔
//! WorkspacePane / 独立二进制（`crossh-git` / `crossh-note` / `crossh-updater`），
//! 通过 WorkspacePane 组合进同一工作区。
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
        ToggleScratchTerminal,
        ToggleTimestamps,
        ZoomWindow
    ]
);

fn main() {
    let cli = app::cli::parse_cli(
        std::env::args().skip(1),
        std::env::current_dir().map_err(|error| error.to_string()),
    );
    let launch_target = match cli {
        app::cli::CliCommand::Help => {
            app::cli::print_help();
            return;
        }
        app::cli::CliCommand::Version => {
            println!("crossh {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        app::cli::CliCommand::Git(result) => match result {
            Ok(features::git_launcher::GitCliCommand::Open(cwd)) => {
                if let Err(error) = features::git_launcher::spawn_git_process(&cwd) {
                    eprintln!("crossh git: failed to start crossh-git: {error}");
                    std::process::exit(1);
                }
                return;
            }
            Ok(features::git_launcher::GitCliCommand::Help) => {
                features::git_launcher::print_help("crossh git");
                return;
            }
            Err(error) => {
                eprintln!("crossh git: {error}\n");
                features::git_launcher::print_help("crossh git");
                std::process::exit(2);
            }
        },
        app::cli::CliCommand::Note(result) => match result {
            Ok(features::note_launcher::NoteCliCommand::Open) => {
                if let Err(error) = features::note_launcher::spawn_note_process() {
                    eprintln!("crossh note: failed to start crossh-note: {error}");
                    std::process::exit(1);
                }
                return;
            }
            Ok(features::note_launcher::NoteCliCommand::Help) => {
                features::note_launcher::print_help("crossh note");
                return;
            }
            Err(error) => {
                eprintln!("crossh note: {error}\n");
                features::note_launcher::print_help("crossh note");
                std::process::exit(2);
            }
        },
        app::cli::CliCommand::Unknown(argument) => {
            eprintln!("unknown argument: {argument}\n");
            app::cli::print_help();
            std::process::exit(2);
        }
        app::cli::CliCommand::Main(target) => target,
    };
    infrastructure::logging::init();

    // 预热 tokio 运行时（单例，限 2 worker 线程，控内存）。
    let _rt = infrastructure::runtime::runtime();

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
        features::settings::init();
        features::terminal::init(cx);
        cx.bind_keys([
            gpui::KeyBinding::new("cmd-`", ToggleScratchTerminal, Some("AppShell")),
            gpui::KeyBinding::new("ctrl-`", ToggleScratchTerminal, Some("AppShell")),
            gpui::KeyBinding::new("cmd-j", ToggleScratchTerminal, Some("AppShell")),
        ]);
        infrastructure::app_menu::install(cx);
        app::open_launch_target(launch_target, cx);
    });
}

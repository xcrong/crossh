//! crossh —— 基于 gpui 的本地优先终端工作环境（macOS / Linux / Windows）。
//!
//! 以项目目录组织多会话本地终端为核心，工作区内置终端与独立的
//! `crossh-git` / `crossh-note` / `crossh-updater` 二进制共同组成应用。
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod features;
mod infrastructure;

use gpui::{App, actions};

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
        ToggleScratchTerminal,
        ToggleTimestamps,
        ToggleCommandPalette,
        ZoomWindow
    ]
);

fn main() {
    // Windows GUI 子系统下无控制台：挂回父控制台，保证 --help/--version 等
    // CLI 输出在终端里仍然可见；从资源管理器启动时此调用无操作。
    crossh_core::process::attach_parent_console();
    let cli = app::cli::parse_cli(
        std::env::args().skip(1),
        std::env::current_dir().map_err(|error| error.to_string()),
    );
    // 各 CLI 分支返回 `true` 表示落到 GUI 启动；`process::exit` 分支类型为 `!`。
    // 会话执行收敛在 `app::session`，main 只做装配。
    let continue_gui = match cli {
        app::cli::CliCommand::Help => {
            app::cli::print_help();
            false
        }
        app::cli::CliCommand::Version => {
            println!("crossh {}", env!("CARGO_PKG_VERSION"));
            false
        }
        app::cli::CliCommand::Git(result) => app::session::dispatch_git(result),
        app::cli::CliCommand::Note(result) => app::session::dispatch_note(result),
        app::cli::CliCommand::Daemon(action) => {
            std::process::exit(app::session::run_daemon_command(action))
        }
        app::cli::CliCommand::Ls => std::process::exit(app::session::run_ls_command()),
        app::cli::CliCommand::Run(argv) => {
            if argv.is_empty() {
                eprintln!("usage: crossh run -- <command> [args...]");
                std::process::exit(2);
            }
            std::process::exit(app::session::run_run_command(&argv));
        }
        app::cli::CliCommand::Capture(id) => {
            std::process::exit(app::session::run_capture_command(&id))
        }
        app::cli::CliCommand::Unknown(argument) => {
            eprintln!("unknown argument: {argument}\n");
            eprintln!("try `crossh open <path>` or `crossh --help`");
            std::process::exit(2);
        }
        app::cli::CliCommand::Open(raw) => app::session::dispatch_open(raw),
        app::cli::CliCommand::Main => app::session::dispatch_main(),
    };
    if continue_gui {
        run_gui();
    }
}

/// 启动 GUI：日志、tokio 运行时、GPUI 应用装配与窗口初始化。
fn run_gui() {
    app::bootstrap::init();

    // 预热 tokio 运行时（单例，限 2 worker 线程，控内存）。
    let _rt = app::bootstrap::runtime();

    let app = gpui_platform::application().with_assets(crossh_ui::assets::UiAssetSource::default());
    app.on_reopen(move |cx| {
        // Reuse an existing window, including a hidden one. Only create a
        // window when the application has no windows left.
        if let Some(window) = cx.windows().into_iter().next() {
            let _ = window.update(cx, |_, window, _| window.activate_window());
        } else {
            crate::features::workspace::open_main_window(cx);
        }
    });
    app.run(move |cx: &mut App| {
        cx.set_app_identity("me.xcrong.crossh", "Crossh");
        cx.init_colors();
        crossh_ui::theme::install_crossh_theme(cx);
        // ponytail: 字体加载失败只记日志回退系统字体，不阻断启动。
        if let Err(error) = crossh_ui::assets::load_fonts(cx) {
            log::warn!("Crossh fonts unavailable, falling back to system fonts: {error:#}");
        }
        features::settings::init();
        crossh_terminal_view::set_text_resolver(crossh_core::i18n::text);
        crossh_terminal_view::init(cx);
        features::workspace::shell::init(cx);
        cx.bind_keys([
            gpui::KeyBinding::new("cmd-`", ToggleScratchTerminal, Some("AppShell")),
            gpui::KeyBinding::new("ctrl-`", ToggleScratchTerminal, Some("AppShell")),
            gpui::KeyBinding::new("cmd-j", ToggleScratchTerminal, Some("AppShell")),
        ]);
        infrastructure::app_menu::install(cx);
        // 菜单栏 tray（仅 macOS）：主线程任务驱动，失败只记日志。
        #[cfg(target_os = "macos")]
        crate::infrastructure::tray::spawn(cx);
        // 单实例投递桥：监听线程只做阻塞收包（serve），前台消费与项目激活
        // 收敛在 app_menu，main 只做装配。
        let (open_tx, open_rx) =
            tokio::sync::mpsc::unbounded_channel::<Option<std::path::PathBuf>>();
        if let Err(error) = app::single_instance::serve(move |request| {
            let _ = open_tx.send(request);
        }) {
            // 监听失败不阻断启动：退化为无复用，新窗口照常打开。
            log::warn!("single-instance listener unavailable: {error}");
        }
        crate::features::workspace::open_main_window(cx);
        infrastructure::app_menu::show_main_window(cx);
        infrastructure::app_menu::serve_project_requests(cx, open_rx);
    });
}

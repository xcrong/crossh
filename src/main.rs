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
    let continue_gui = match cli {
        app::cli::CliCommand::Help => {
            app::cli::print_help();
            false
        }
        app::cli::CliCommand::Version => {
            println!("crossh {}", env!("CARGO_PKG_VERSION"));
            false
        }
        app::cli::CliCommand::Git(result) => dispatch_git(result),
        app::cli::CliCommand::Note(result) => dispatch_note(result),
        app::cli::CliCommand::Daemon(action) => std::process::exit(run_daemon_command(action)),
        app::cli::CliCommand::Ls => std::process::exit(run_ls_command()),
        app::cli::CliCommand::Run(argv) => {
            if argv.is_empty() {
                eprintln!("usage: crossh run -- <command> [args...]");
                std::process::exit(2);
            }
            std::process::exit(run_run_command(&argv));
        }
        app::cli::CliCommand::Capture(id) => std::process::exit(run_capture_command(&id)),
        app::cli::CliCommand::Unknown(argument) => {
            eprintln!("unknown argument: {argument}\n");
            eprintln!("try `crossh open <path>` or `crossh --help`");
            std::process::exit(2);
        }
        app::cli::CliCommand::Open(raw) => dispatch_open(raw),
        app::cli::CliCommand::Main => dispatch_main(),
    };
    if continue_gui {
        run_gui();
    }
}

/// `crossh git`：成功后已交由子进程处理，主进程退出；返回 `false` 阻止 GUI 启动。
fn dispatch_git(result: Result<crossh_core::git_launcher::GitCliCommand, String>) -> bool {
    match result {
        Ok(crossh_core::git_launcher::GitCliCommand::Open(cwd)) => {
            if let Err(error) = crossh_core::git_launcher::spawn_git_process(&cwd) {
                eprintln!("crossh git: failed to start crossh-git: {error}");
                std::process::exit(1);
            }
            false
        }
        Ok(crossh_core::git_launcher::GitCliCommand::Help) => {
            crossh_core::git_launcher::print_help("crossh git");
            false
        }
        Err(error) => {
            eprintln!("crossh git: {error}\n");
            crossh_core::git_launcher::print_help("crossh git");
            std::process::exit(2);
        }
    }
}

/// `crossh note`：与 [`dispatch_git`] 同语义。
fn dispatch_note(result: Result<crossh_core::note_launcher::NoteCliCommand, String>) -> bool {
    match result {
        Ok(crossh_core::note_launcher::NoteCliCommand::Open) => {
            if let Err(error) = crossh_core::note_launcher::spawn_note_process() {
                eprintln!("crossh note: failed to start crossh-note: {error}");
                std::process::exit(1);
            }
            false
        }
        Ok(crossh_core::note_launcher::NoteCliCommand::Help) => {
            crossh_core::note_launcher::print_help("crossh note");
            false
        }
        Err(error) => {
            eprintln!("crossh note: {error}\n");
            crossh_core::note_launcher::print_help("crossh note");
            std::process::exit(2);
        }
    }
}

/// `crossh open <path>`：daemon 打开项目后尝试转发；`true` 表示本地启动 GUI。
fn dispatch_open(raw: std::path::PathBuf) -> bool {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
    let directory = match app::single_instance::resolve_open_path(&raw, &cwd) {
        Ok(directory) => directory,
        Err(error) => {
            eprintln!("crossh: {error}");
            std::process::exit(2);
        }
    };
    if let Err(error) = run_open_command(directory.clone()) {
        eprintln!("crossh open: {error}");
        std::process::exit(1);
    }
    match app::single_instance::try_forward(None) {
        app::single_instance::ForwardOutcome::Forwarded => {
            println!(
                "crossh: opened {} in the running instance",
                directory.display()
            );
            false
        }
        app::single_instance::ForwardOutcome::Rejected(error) => {
            eprintln!("crossh: {error}");
            std::process::exit(2);
        }
        app::single_instance::ForwardOutcome::NoInstance => true,
    }
}

/// 无子命令启动：已有实例则转发并退出，否则返回 `true` 继续 GUI。
fn dispatch_main() -> bool {
    match app::single_instance::try_forward(None) {
        app::single_instance::ForwardOutcome::Forwarded => {
            println!("crossh is already running");
            false
        }
        app::single_instance::ForwardOutcome::Rejected(error) => {
            eprintln!("crossh: {error}");
            std::process::exit(2);
        }
        app::single_instance::ForwardOutcome::NoInstance => true,
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
        crossh_ui::assets::load_fonts(cx).expect("Crossh fonts should load");
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

/// 会话类 CLI 的 tokio 执行器（一次性短进程，current-thread 即可）。
fn session_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("session cli runtime")
}

fn run_open_command(root: std::path::PathBuf) -> Result<(), String> {
    use crossh_session::protocol::WorkspaceMutateRequest;
    use crossh_session::workspace::{MutateOutcome, WorkspaceOp};

    let rt = session_runtime();
    rt.block_on(async move {
        let addr = crossh_session::ensure_daemon()
            .await
            .map_err(|error| error.to_string())?;
        let mut client = crossh_session::Client::connect_to(&addr)
            .await
            .map_err(|error| error.to_string())?;
        let reply = client
            .workspace_mutate(WorkspaceMutateRequest {
                request_id: 1,
                expected_revision: None,
                op: WorkspaceOp::OpenProject { root },
            })
            .await
            .map_err(|error| error.to_string())?;
        match reply.outcome {
            MutateOutcome::Applied | MutateOutcome::Unchanged => Ok(()),
            MutateOutcome::Rejected | MutateOutcome::Conflict => Err(reply
                .error
                .map(|error| error.message)
                .unwrap_or_else(|| format!("workspace mutation {:?}", reply.outcome))),
        }
    })
}

fn run_daemon_command(action: app::cli::DaemonAction) -> i32 {
    use app::cli::DaemonAction;
    use crossh_session::DaemonState;
    let rt = session_runtime();
    match action {
        DaemonAction::Status => match rt.block_on(crossh_session::probe_daemon()) {
            DaemonState::Ready(addr) => {
                let connected = rt.block_on(async {
                    let mut client = crossh_session::Client::connect_to(&addr).await?;
                    let sessions = client.list().await?;
                    let workspace = client.workspace_get().await?;
                    Ok::<_, std::io::Error>((sessions, workspace))
                });
                match connected {
                    Ok((sessions, workspace)) => {
                        println!("running ({} sessions, port {})", sessions.len(), addr.port);
                        let tabs = workspace
                            .projects
                            .iter()
                            .map(|project| project.tabs.len())
                            .sum::<usize>();
                        let panes = workspace
                            .projects
                            .iter()
                            .flat_map(|project| &project.tabs)
                            .map(|tab| tab.layout.leaves().count())
                            .sum::<usize>();
                        println!(
                            "workspace: {} projects, {} tabs, {} panes",
                            workspace.projects.len(),
                            tabs,
                            panes
                        );
                        0
                    }
                    Err(error) => {
                        eprintln!("crossh daemon: {error}");
                        1
                    }
                }
            }
            DaemonState::Absent { stale } => {
                let base = if stale {
                    "not running (stale port file)"
                } else {
                    "not running"
                };
                match crossh_session::workspace_store::read_daemon_error() {
                    Some(reason) => println!("{base}: {reason}"),
                    None => match crossh_session::workspace_store::workspace_format_too_new() {
                        Some(found) => println!(
                            "{base}: workspace format v{found} is newer than supported v{}; upgrade crossh",
                            crossh_session::workspace::WORKSPACE_FORMAT_VERSION
                        ),
                        None => println!("{base}"),
                    },
                }
                1
            }
            DaemonState::Incompatible { message } => {
                let found = {
                    let rest = message.split("daemon ").nth(1).unwrap_or_default();
                    rest.split(|c: char| !c.is_ascii_digit())
                        .next()
                        .filter(|digits| !digits.is_empty())
                        .map(|digits| format!("v{digits}"))
                        .unwrap_or_else(|| "unknown".to_string())
                };
                println!(
                    "incompatible: found {found}, expected v{}; run crossh daemon stop",
                    crossh_session::handshake::PROTOCOL_VERSION
                );
                1
            }
        },
        DaemonAction::Start => match rt.block_on(crossh_session::ensure_daemon()) {
            Ok(addr) => {
                println!("started (port {})", addr.port);
                0
            }
            Err(error) => {
                eprintln!("crossh daemon: {error}");
                1
            }
        },
        DaemonAction::Stop => match rt.block_on(crossh_session::daemon_stop()) {
            Ok(()) => {
                println!("stopped");
                0
            }
            Err(error) => {
                eprintln!("crossh daemon: {error}");
                1
            }
        },
    }
}

fn run_ls_command() -> i32 {
    let rt = session_runtime();
    let mut client = match rt.block_on(crossh_session::Client::connect()) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("crossh ls: {error}");
            return 1;
        }
    };
    match rt.block_on(client.list()) {
        Ok(sessions) => {
            println!(
                "{:<8} {:<10} {:<5} {:<7} {:<8} {:<8} {:<8} CWD",
                "ID", "ROLE", "ALIVE", "PROGRAM", "PROJECT", "TAB", "PANE"
            );
            for session in &sessions {
                let id_or_dash = |id: Option<u64>| {
                    id.map(|id| id.to_string())
                        .unwrap_or_else(|| "-".to_string())
                };
                println!(
                    "{:<8} {:<10} {:<5} {:<7} {:<8} {:<8} {:<8} {}",
                    session.id,
                    crossh_session::role_of(session, client.client_id),
                    session.alive,
                    session.program.as_deref().unwrap_or("-"),
                    id_or_dash(session.project_id),
                    id_or_dash(session.tab_id),
                    id_or_dash(session.pane_id),
                    session.cwd.as_deref().unwrap_or("-"),
                );
            }
            0
        }
        Err(error) => {
            eprintln!("crossh ls: {error}");
            1
        }
    }
}

fn run_run_command(argv: &[String]) -> i32 {
    let rt = session_runtime();
    let mut out = std::io::stdout().lock();
    match rt.block_on(crossh_session::run_command(argv, &mut out)) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("crossh run: {error}");
            1
        }
    }
}

fn run_capture_command(id: &str) -> i32 {
    let rt = session_runtime();
    let mut out = std::io::stdout().lock();
    match rt.block_on(crossh_session::capture_session(id, &mut out)) {
        Ok(_) => 0,
        Err(error) => {
            eprintln!("crossh capture: {error}");
            1
        }
    }
}

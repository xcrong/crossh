//! 会话类 CLI 的执行层：`main` 只做装配，分发与退出码收敛于此。
//!
//! 所有 daemon 会话调用共用一个 current-thread 运行时（一次性短进程），
//! 各分支经 [`block_on`] 收敛，不再各自重复 `session_runtime` 样板。

use super::cli::DaemonAction;

/// 会话类 CLI 的 tokio 执行器（一次性短进程，current-thread 即可）。
fn session_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("session cli runtime")
}

/// 全部会话阻塞点的唯一入口：调用方只传 future，不再各自建运行时。
fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    session_runtime().block_on(future)
}

/// `crossh git`：成功后已交由子进程处理，主进程退出；返回 `false` 阻止 GUI 启动。
pub(crate) fn dispatch_git(
    result: Result<crossh_core::git_launcher::GitCliCommand, String>,
) -> bool {
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
pub(crate) fn dispatch_note(
    result: Result<crossh_core::note_launcher::NoteCliCommand, String>,
) -> bool {
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
pub(crate) fn dispatch_open(raw: std::path::PathBuf) -> bool {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
    let directory = match super::single_instance::resolve_open_path(&raw, &cwd) {
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
    match super::single_instance::try_forward(None) {
        super::single_instance::ForwardOutcome::Forwarded => {
            println!(
                "crossh: opened {} in the running instance",
                directory.display()
            );
            false
        }
        super::single_instance::ForwardOutcome::Rejected(error) => {
            eprintln!("crossh: {error}");
            std::process::exit(2);
        }
        super::single_instance::ForwardOutcome::NoInstance => true,
    }
}

/// 无子命令启动：已有实例则转发并退出，否则返回 `true` 继续 GUI。
pub(crate) fn dispatch_main() -> bool {
    match super::single_instance::try_forward(None) {
        super::single_instance::ForwardOutcome::Forwarded => {
            println!("crossh is already running");
            false
        }
        super::single_instance::ForwardOutcome::Rejected(error) => {
            eprintln!("crossh: {error}");
            std::process::exit(2);
        }
        super::single_instance::ForwardOutcome::NoInstance => true,
    }
}

fn run_open_command(root: std::path::PathBuf) -> Result<(), String> {
    use crossh_session::protocol::WorkspaceMutateRequest;
    use crossh_session::workspace::{MutateOutcome, WorkspaceOp};

    block_on(async move {
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

pub(crate) fn run_daemon_command(action: DaemonAction) -> i32 {
    use crossh_session::DaemonState;
    match action {
        // ponytail: 探测与连接收敛在单次 block_on 内，原先 status 分支占两次。
        DaemonAction::Status => block_on(async {
            match crossh_session::probe_daemon().await {
                DaemonState::Ready(addr) => {
                    let connected = async {
                        let mut client = crossh_session::Client::connect_to(&addr).await?;
                        let sessions = client.list().await?;
                        let workspace = client.workspace_get().await?;
                        Ok::<_, std::io::Error>((sessions, workspace))
                    }
                    .await;
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
            }
        }),
        DaemonAction::Start => block_on(async {
            match crossh_session::ensure_daemon().await {
                Ok(addr) => {
                    println!("started (port {})", addr.port);
                    0
                }
                Err(error) => {
                    eprintln!("crossh daemon: {error}");
                    1
                }
            }
        }),
        DaemonAction::Stop => block_on(async {
            match crossh_session::daemon_stop().await {
                Ok(()) => {
                    println!("stopped");
                    0
                }
                Err(error) => {
                    eprintln!("crossh daemon: {error}");
                    1
                }
            }
        }),
    }
}

pub(crate) fn run_ls_command() -> i32 {
    // ponytail: 连接与列表收敛在单次 block_on 内，原先占两次。
    block_on(async {
        let mut client = match crossh_session::Client::connect().await {
            Ok(client) => client,
            Err(error) => {
                eprintln!("crossh ls: {error}");
                return 1;
            }
        };
        match client.list().await {
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
    })
}

pub(crate) fn run_run_command(argv: &[String]) -> i32 {
    let mut out = std::io::stdout().lock();
    match block_on(crossh_session::run_command(argv, &mut out)) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("crossh run: {error}");
            1
        }
    }
}

pub(crate) fn run_capture_command(id: &str) -> i32 {
    let mut out = std::io::stdout().lock();
    match block_on(crossh_session::capture_session(id, &mut out)) {
        Ok(_) => 0,
        Err(error) => {
            eprintln!("crossh capture: {error}");
            1
        }
    }
}

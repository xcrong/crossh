//! 本地终端：使用和远端终端相同的输入/事件桥接，但在本机 PTY 中启动 shell。

#[cfg(unix)]
use std::fs::{self, File};
#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::io::{ErrorKind, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::Path;
#[cfg(not(windows))]
use std::path::PathBuf;
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(unix)]
use std::sync::{Arc, Mutex};

#[cfg(unix)]
use alacritty_terminal::event::{OnResize, WindowSize};
#[cfg(unix)]
use alacritty_terminal::tty;
#[cfg(unix)]
use alacritty_terminal::tty::EventedPty;
#[cfg(not(windows))]
use async_channel::{Receiver, Sender};
#[cfg(unix)]
use tokio::io::unix::AsyncFd;

#[cfg(not(windows))]
use crate::shared::terminal::{InputCmd, SessionEvent};
#[cfg(unix)]
use crate::shared::terminal::{ProtocolEvent, TerminalProtocolParser};

#[cfg(any(windows, test))]
mod windows;

#[cfg(unix)]
mod process;

#[cfg(unix)]
use process::ProcessTracker;

#[cfg(windows)]
pub use windows::open_terminal;

#[cfg(unix)]
static NEXT_LOCAL_PTY_ID: AtomicU64 = AtomicU64::new(1);

/// 在指定工作目录创建一个本地交互式终端。
#[cfg(unix)]
pub fn open_terminal(
    cwd: PathBuf,
    cols: u16,
    rows: u16,
) -> (Sender<InputCmd>, Receiver<SessionEvent>) {
    let (input_tx, input_rx) = async_channel::bounded::<InputCmd>(1024);
    let (event_tx, event_rx) = async_channel::bounded::<SessionEvent>(256);
    let pty_id = NEXT_LOCAL_PTY_ID.fetch_add(1, Ordering::Relaxed);

    crate::infrastructure::ssh::ssh_runtime().spawn(async move {
        if let Err(error) =
            run_local_terminal(cwd, cols, rows, pty_id, input_rx, event_tx.clone()).await
        {
            let _ = event_tx.send(SessionEvent::Error(error.to_string())).await;
        }
        let _ = event_tx.send(SessionEvent::Closed).await;
    });

    (input_tx, event_rx)
}

#[cfg(not(any(unix, windows)))]
pub fn open_terminal(
    _cwd: PathBuf,
    _cols: u16,
    _rows: u16,
) -> (Sender<InputCmd>, Receiver<SessionEvent>) {
    let (input_tx, _input_rx) = async_channel::bounded::<InputCmd>(1024);
    let (event_tx, event_rx) = async_channel::bounded::<SessionEvent>(4);

    crate::infrastructure::ssh::ssh_runtime().spawn(async move {
        let _ = event_tx
            .send(SessionEvent::Error(
                "local terminals are not supported on this platform".to_string(),
            ))
            .await;
        let _ = event_tx.send(SessionEvent::Closed).await;
    });

    (input_tx, event_rx)
}

#[cfg(unix)]
async fn run_local_terminal(
    cwd: PathBuf,
    cols: u16,
    rows: u16,
    pty_id: u64,
    input_rx: Receiver<InputCmd>,
    event_tx: Sender<SessionEvent>,
) -> io::Result<()> {
    let mut options = tty::Options {
        working_directory: Some(cwd.clone()),
        ..Default::default()
    };
    if let Some(shell) = std::env::var_os("SHELL") {
        options.shell = Some(tty::Shell::new(
            shell.to_string_lossy().to_string(),
            vec!["-l".to_string()],
        ));
    }
    options
        .env
        .insert("TERM".to_string(), "xterm-256color".to_string());
    options
        .env
        .insert("COLORTERM".to_string(), "truecolor".to_string());
    options
        .env
        .insert("TERM_PROGRAM".to_string(), "crossh".to_string());
    let _shell_integration = prepare_shell_integration(&mut options, pty_id);

    let window_size = WindowSize {
        num_lines: rows,
        num_cols: cols,
        cell_width: 8,
        cell_height: 18,
    };
    let pty = tty::new(&options, window_size, pty_id)?;
    let reader = AsyncFd::new(pty.file().try_clone()?)?;
    let writer = AsyncFd::new(pty.file().try_clone()?)?;
    let process_tracker = ProcessTracker::new(&pty);
    let pty = Arc::new(Mutex::new(pty));

    let display_cwd = cwd.to_string_lossy().to_string();
    let _ = event_tx.send(SessionEvent::Cwd(display_cwd)).await;
    let _ = event_tx.send(SessionEvent::Connected).await;

    let process_cancel = Arc::new(AtomicBool::new(false));
    let process_task = process_tracker.map(|tracker| {
        let cancel = process_cancel.clone();
        let event_tx = event_tx.clone();
        tokio::task::spawn_blocking(move || tracker.run(event_tx, cancel))
    });
    let mut read_task = tokio::spawn(read_local_output(reader, event_tx.clone()));
    let mut write_task = tokio::spawn(drive_local_input(input_rx, writer, pty.clone()));

    tokio::select! {
        result = &mut read_task => {
            write_task.abort();
            let _ = write_task.await;
            if let Ok(Err(error)) = result
                && error.kind() != ErrorKind::UnexpectedEof {
                    let _ = event_tx.send(SessionEvent::Error(error.to_string())).await;
                }
        }
        result = &mut write_task => {
            read_task.abort();
            let _ = read_task.await;
            if let Ok(Err(error)) = result {
                let _ = event_tx.send(SessionEvent::Error(error.to_string())).await;
            }
        }
    }

    process_cancel.store(true, Ordering::Release);
    if let Some(process_task) = process_task {
        let _ = process_task.await;
    }

    drop(pty);
    Ok(())
}

#[cfg(unix)]
async fn read_local_output(
    reader: AsyncFd<File>,
    event_tx: Sender<SessionEvent>,
) -> io::Result<()> {
    // Keep the low-level local session contract useful to consumers that need
    // cwd updates before the output reaches a TerminalView. The view still
    // parses the complete stream for all other terminal protocol events.
    let mut protocol_parser = TerminalProtocolParser::default();
    let mut bytes_read = 0u64;
    let mut last_report = std::time::Instant::now();
    loop {
        let mut guard = reader.readable().await?;
        let mut buffer = [0u8; 32 * 1024];
        match guard.try_io(|inner| inner.get_ref().read(&mut buffer)) {
            Ok(Ok(0)) => return Ok(()),
            Ok(Ok(size)) => {
                bytes_read += size as u64;
                let now = std::time::Instant::now();
                if now.saturating_duration_since(last_report) >= std::time::Duration::from_secs(10)
                {
                    log::info!("local pty reader alive: {} bytes read", bytes_read);
                    last_report = now;
                }
                for event in protocol_parser.feed(&buffer[..size]) {
                    if let ProtocolEvent::Cwd(cwd) = event {
                        let _ = event_tx.send(SessionEvent::Cwd(cwd)).await;
                    }
                }
                let _ = event_tx
                    .send(SessionEvent::Output(buffer[..size].to_vec()))
                    .await;
            }
            Ok(Err(error)) => return Err(error),
            Err(_would_block) => continue,
        }
    }
}

#[cfg(unix)]
async fn drive_local_input(
    input_rx: Receiver<InputCmd>,
    mut writer: AsyncFd<File>,
    pty: Arc<Mutex<tty::Pty>>,
) -> io::Result<()> {
    // 每 50ms 消费一次 SIGCHLD self-pipe（alacritty 的 `Pty::next_child_event`）。
    // 不消费的话，SIGCHLD 风暴会让 self-pipe 写满，信号处理器里的 `sendto` 阻塞，
    // 主线程卡死在 signal handler 里（表现为整个 UI 冻结）。
    let mut child_poll = tokio::time::interval(std::time::Duration::from_millis(50));
    child_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            command = input_rx.recv() => {
                let Ok(command) = command else { break };
                match command {
                    InputCmd::Write(bytes) => write_all(&mut writer, &bytes).await?,
                    InputCmd::Resize { cols, rows } => {
                        if let Ok(mut pty) = pty.lock() {
                            pty.on_resize(WindowSize {
                                num_lines: rows,
                                num_cols: cols,
                                cell_width: 8,
                                cell_height: 18,
                            });
                        }
                    }
                    InputCmd::Close => break,
                }
            }
            _ = child_poll.tick() => {
                if let Ok(mut pty) = pty.lock() {
                    // 回收僵尸子进程 + 排空 SIGCHLD pipe。
                    while pty.next_child_event().is_some() {}
                }
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
async fn write_all(writer: &mut AsyncFd<File>, bytes: &[u8]) -> io::Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        let mut guard = writer.writable().await?;
        match guard.try_io(|inner| inner.get_ref().write(&bytes[offset..])) {
            Ok(Ok(0)) => {
                return Err(io::Error::new(
                    ErrorKind::WriteZero,
                    "local PTY write returned zero",
                ));
            }
            Ok(Ok(size)) => offset += size,
            Ok(Err(error)) => return Err(error),
            Err(_would_block) => continue,
        }
    }
    Ok(())
}

#[cfg(unix)]
struct ShellIntegration {
    directory: PathBuf,
}

#[cfg(unix)]
impl Drop for ShellIntegration {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

/// 将 shell 的提示符钩子接到 OSC 7/133，上报 cwd 并标记命令已经结束。
#[cfg(unix)]
fn prepare_shell_integration(options: &mut tty::Options, pty_id: u64) -> Option<ShellIntegration> {
    let shell = std::env::var_os("SHELL")?;
    let shell_name = Path::new(&shell).file_name()?.to_str()?;
    if shell_name == "bash" {
        let report = r#"__crossh_status=$?; printf '\033]133;D;%s\007\033]133;B\007\033]7;file://localhost%s\007\033]133;A\007' "$__crossh_status" "$PWD""#;
        let original = std::env::var("PROMPT_COMMAND").unwrap_or_default();
        let prompt_command = if original.is_empty() {
            report.to_string()
        } else {
            format!("{report};{original}")
        };
        let original_ps0 = std::env::var("PS0").unwrap_or_default();
        let ps0 = format!("\x1b]133;C\x07{original_ps0}");
        options
            .env
            .insert("PROMPT_COMMAND".to_string(), prompt_command);
        options.env.insert("PS0".to_string(), ps0);
        return None;
    }
    if shell_name == "fish" {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        let temp_root = std::env::temp_dir();
        let directory = create_shell_integration_dir(&temp_root, pty_id).ok()?;
        let fish_directory = directory.join("fish");
        if fs::create_dir(&fish_directory).is_err() {
            let _ = fs::remove_dir_all(&directory);
            return None;
        }

        let config_root = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        if let Err(error) = preserve_fish_config_state(&fish_directory, &config_root) {
            log::warn!("unable to preserve fish config state: {error}");
            let _ = fs::remove_dir_all(&directory);
            return None;
        }
        let original_config = config_root.join("fish").join("config.fish");
        let source_original =
            if !paths_equivalent(&original_config, &fish_directory.join("config.fish")) {
                format!(
                    "if test -r {}; source {}; end\n",
                    shell_quote(&original_config),
                    shell_quote(&original_config),
                )
            } else {
                String::new()
            };
        let config = format!(
            "{source_original}\
function __crossh_report_pwd\n\
    printf '\\033]7;file://localhost%s\\007' \"$PWD\"\n\
end\n\
function __crossh_report_prompt --on-event fish_prompt\n\
    set -l __crossh_status $status\n\
    printf '\\033]133;D;%s\\007\\033]133;B\\007' \"$__crossh_status\"\n\
    __crossh_report_pwd\n\
    printf '\\033]133;A\\007'\n\
end\n\
function __crossh_report_command --on-event fish_preexec\n\
    printf '\\033]133;C\\007'\n\
end\n\
function __crossh_report_cwd --on-variable PWD\n\
    __crossh_report_pwd\n\
end\n",
        );
        if fs::write(fish_directory.join("config.fish"), config).is_err() {
            let _ = fs::remove_dir_all(&directory);
            return None;
        }
        options.env.insert(
            "XDG_CONFIG_HOME".to_string(),
            directory.to_string_lossy().to_string(),
        );
        return Some(ShellIntegration { directory });
    }
    if shell_name != "zsh" {
        return None;
    }

    let home = PathBuf::from(std::env::var_os("HOME")?);
    let temp_root = std::env::temp_dir();
    let directory = create_shell_integration_dir(&temp_root, pty_id).ok()?;
    let original_zdotdir = original_zdotdir(
        std::env::var_os("ZDOTDIR").map(PathBuf::from),
        &home,
        &temp_root,
        &directory,
    );
    let self_source = paths_equivalent(&original_zdotdir, &directory);
    if let Err(error) = preserve_zsh_history(&directory, &original_zdotdir) {
        log::warn!("unable to preserve zsh history: {error}");
        let _ = fs::remove_dir_all(&directory);
        return None;
    }

    // ZDOTDIR 重定向会让 zsh 找不到 ~/.zprofile、~/.zshenv、~/.zlogin，
    // 而很多用户的 PATH（brew shellenv 等）都写在 .zprofile 里。
    // 为每个 dotfile 写一个 source 原文件的包装器，保留 login shell 完整语义。
    for name in [".zshenv", ".zprofile", ".zlogin"] {
        let original = original_zdotdir.join(name);
        // Keep this guard even though `create_shell_integration_dir` uses a
        // fresh directory: it prevents a future naming change or path alias
        // from producing a wrapper that sources itself recursively.
        if self_source || paths_equivalent(&original, &directory.join(name)) {
            continue;
        }
        let wrapper = format!(
            "if [[ -r {} ]]; then source {}; fi\n",
            shell_quote(&original),
            shell_quote(&original),
        );
        if fs::write(directory.join(name), wrapper).is_err() {
            let _ = fs::remove_dir_all(&directory);
            return None;
        }
    }

    let original_rc = original_zdotdir.join(".zshrc");
    let rc = if self_source || paths_equivalent(&original_rc, &directory.join(".zshrc")) {
        "autoload -Uz add-zsh-hook\n\
__crossh_report_pwd() { printf '\\033]7;file://localhost%s\\007' \"$PWD\"; }\n\
__crossh_report_prompt() { local __crossh_status=$?; printf '\\033]133;D;%s\\007\\033]133;B\\007' \"$__crossh_status\"; __crossh_report_pwd; printf '\\033]133;A\\007'; }\n\
__crossh_report_command() { printf '\\033]133;C\\007'; }\n\
add-zsh-hook precmd __crossh_report_prompt\n\
add-zsh-hook preexec __crossh_report_command\n\
add-zsh-hook chpwd __crossh_report_pwd\n"
            .to_string()
    } else {
        format!(
            "if [[ -r {original_rc} ]]; then source {original_rc}; fi\n\
autoload -Uz add-zsh-hook\n\
__crossh_report_pwd() {{ printf '\\033]7;file://localhost%s\\007' \"$PWD\"; }}\n\
__crossh_report_prompt() {{ local __crossh_status=$?; printf '\\033]133;D;%s\\007\\033]133;B\\007' \"$__crossh_status\"; __crossh_report_pwd; printf '\\033]133;A\\007'; }}\n\
__crossh_report_command() {{ printf '\\033]133;C\\007'; }}\n\
add-zsh-hook precmd __crossh_report_prompt\n\
add-zsh-hook preexec __crossh_report_command\n\
add-zsh-hook chpwd __crossh_report_pwd\n",
            original_rc = shell_quote(&original_rc),
        )
    };
    if fs::write(directory.join(".zshrc"), rc).is_err() {
        let _ = fs::remove_dir_all(&directory);
        return None;
    }

    options.env.insert(
        "ZDOTDIR".to_string(),
        directory.to_string_lossy().to_string(),
    );
    Some(ShellIntegration { directory })
}

/// Keep fish's automatically loaded config state visible through the temporary
/// XDG_CONFIG_HOME used for the prompt wrapper.
#[cfg(unix)]
fn preserve_fish_config_state(fish_directory: &Path, config_root: &Path) -> io::Result<()> {
    let original_fish_directory = config_root.join("fish");
    if !original_fish_directory.is_dir() {
        return Ok(());
    }

    for name in [
        "conf.d",
        "completions",
        "functions",
        "themes",
        "fish_variables",
    ] {
        let original = original_fish_directory.join(name);
        if original.exists() {
            symlink(&original, fish_directory.join(name))?;
        }
    }
    Ok(())
}

/// zsh derives its default HISTFILE from ZDOTDIR, so expose the user's real
/// history file at the generated directory before zsh startup reads it.
#[cfg(unix)]
fn preserve_zsh_history(directory: &Path, original_zdotdir: &Path) -> io::Result<()> {
    symlink(
        original_zdotdir.join(".zsh_history"),
        directory.join(".zsh_history"),
    )
}

#[cfg(unix)]
fn create_shell_integration_dir(temp_root: &Path, pty_id: u64) -> io::Result<PathBuf> {
    let pid = std::process::id();
    for attempt in 0..1000u16 {
        let directory = shell_integration_directory(temp_root, pid, pty_id, attempt);
        match fs::create_dir(&directory) {
            Ok(()) => return Ok(directory),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        ErrorKind::AlreadyExists,
        "unable to allocate a unique Crossh zsh integration directory",
    ))
}

#[cfg(unix)]
fn shell_integration_directory(temp_root: &Path, pid: u32, pty_id: u64, attempt: u16) -> PathBuf {
    temp_root.join(format!("crossh-zsh-{pid}-{pty_id}-{attempt}"))
}

#[cfg(unix)]
fn original_zdotdir(
    inherited: Option<PathBuf>,
    home: &Path,
    temp_root: &Path,
    generated: &Path,
) -> PathBuf {
    match inherited {
        Some(path)
            if !paths_equivalent(&path, generated)
                && !is_generated_shell_directory(&path, temp_root) =>
        {
            path
        }
        _ => home.to_path_buf(),
    }
}

#[cfg(unix)]
fn is_generated_shell_directory(path: &Path, temp_root: &Path) -> bool {
    let parent_matches = match (fs::canonicalize(path), fs::canonicalize(temp_root)) {
        (Ok(path), Ok(temp_root)) => path.parent() == Some(temp_root.as_path()),
        _ => path.parent() == Some(temp_root),
    };
    parent_matches
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_generated_shell_directory_name)
}

#[cfg(unix)]
fn is_generated_shell_directory_name(name: &str) -> bool {
    let Some(suffix) = name.strip_prefix("crossh-zsh-") else {
        return false;
    };
    let parts = suffix.split('-').collect::<Vec<_>>();
    matches!(parts.len(), 1 | 3)
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

#[cfg(unix)]
fn paths_equivalent(left: &Path, right: &Path) -> bool {
    left == right
        || fs::canonicalize(left)
            .ok()
            .zip(fs::canonicalize(right).ok())
            .is_some_and(|(left, right)| left == right)
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[cfg(unix)]
    #[test]
    fn shell_integration_ignores_stale_generated_zdotdir() {
        let temp_root = Path::new("/tmp");
        let home = Path::new("/Users/tester");
        let generated = temp_root.join("crossh-zsh-123-1-0");
        assert!(is_generated_shell_directory(&generated, temp_root));
        assert_eq!(
            original_zdotdir(Some(generated.clone()), home, temp_root, &generated),
            home
        );
        assert_eq!(original_zdotdir(None, home, temp_root, &generated), home);

        let custom = PathBuf::from("/Users/tester/custom-zsh");
        assert_eq!(
            original_zdotdir(Some(custom.clone()), home, temp_root, &generated),
            custom
        );
    }

    #[cfg(unix)]
    #[test]
    fn shell_integration_directory_names_include_process_identity() {
        let directory = shell_integration_directory(Path::new("/tmp"), 42, 7, 3);
        assert_eq!(directory, PathBuf::from("/tmp/crossh-zsh-42-7-3"));
        assert!(is_generated_shell_directory(&directory, Path::new("/tmp")));
        // Old releases used `crossh-zsh-{pty_id}`; treat those as generated
        // too so a stale inherited ZDOTDIR cannot recurse.
        assert!(is_generated_shell_directory(
            Path::new("/tmp/crossh-zsh-42"),
            Path::new("/tmp")
        ));
        assert!(!is_generated_shell_directory(
            Path::new("/tmp/crossh-user-zsh"),
            Path::new("/tmp")
        ));
        assert!(!is_generated_shell_directory(
            Path::new("/tmp/crossh-zsh-user"),
            Path::new("/tmp")
        ));
        assert!(!is_generated_shell_directory(
            Path::new("/var/tmp/crossh-zsh-42-7-3"),
            Path::new("/tmp")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn zsh_history_is_linked_to_the_original_zdotdir() {
        let root = std::env::temp_dir().join(format!(
            "crossh-history-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let original = root.join("home");
        let generated = root.join("generated");
        fs::create_dir_all(&original).unwrap();
        fs::create_dir_all(&generated).unwrap();
        fs::write(original.join(".zsh_history"), "old-command\n").unwrap();

        preserve_zsh_history(&generated, &original).unwrap();

        assert!(
            fs::symlink_metadata(generated.join(".zsh_history"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_to_string(generated.join(".zsh_history")).unwrap(),
            "old-command\n"
        );
        fs::write(generated.join(".zsh_history"), "new-command\n").unwrap();
        assert_eq!(
            fs::read_to_string(original.join(".zsh_history")).unwrap(),
            "new-command\n"
        );

        fs::remove_dir_all(&generated).unwrap();
        assert_eq!(
            fs::read_to_string(original.join(".zsh_history")).unwrap(),
            "new-command\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn fish_config_state_is_linked_through_the_temporary_root() {
        let root = std::env::temp_dir().join(format!(
            "crossh-fish-state-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config_root = root.join("config");
        let original_fish = config_root.join("fish");
        let generated_fish = root.join("generated").join("fish");
        fs::create_dir_all(original_fish.join("functions")).unwrap();
        fs::create_dir_all(original_fish.join("conf.d")).unwrap();
        fs::create_dir_all(&generated_fish).unwrap();
        fs::write(
            original_fish.join("fish_variables"),
            "set -U fish_greeting\n",
        )
        .unwrap();

        preserve_fish_config_state(&generated_fish, &config_root).unwrap();

        for name in ["functions", "conf.d", "fish_variables"] {
            assert!(
                fs::symlink_metadata(generated_fish.join(name))
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "fish state path was not linked: {name}"
            );
        }

        fs::remove_dir_all(root).unwrap();
    }

    /// 大量快速退出的子进程会触发 SIGCHLD 风暴。如果 self-pipe 不被消费，
    /// 信号处理器里的 sendto 会阻塞，导致整个进程（包括主线程）冻结。
    /// 用真实子进程（非 kill 信号，因为标准信号会被合并）制造风暴，
    /// 验证 50ms 轮询消费能扛住，终端仍能正常收发输入。
    #[cfg(unix)]
    #[test]
    fn local_terminal_survives_sigchld_storm() {
        let cwd = std::env::current_dir().unwrap();
        let (input_tx, event_rx) = open_terminal(cwd, 80, 24);
        let input_for_task = input_tx.clone();

        // 独立线程：快速 fork 大量立即退出的真子进程。
        // 每个子进程退出都触发 SIGCHLD handler（不可合并，因为每次都是新事件）。
        let flooder = std::thread::spawn(move || {
            use std::process::{Command, Stdio};
            let start = std::time::Instant::now();
            let mut count = 0u64;
            // 分批 spawn，每批 200 个，共 2000 个。
            for batch in 0..10 {
                let children = (0..200)
                    .map(|_| {
                        Command::new("/bin/sh")
                            .arg("-c")
                            .arg("exit 0")
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .spawn()
                            .expect("spawn child")
                    })
                    .collect::<Vec<_>>();
                count += children.len() as u64;
                for mut child in children {
                    let _ = child.wait();
                }
                if batch % 3 == 2 {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            eprintln!(
                "[test] spawned {count} real children in {:?}",
                start.elapsed()
            );
            count
        });

        crate::infrastructure::ssh::ssh_runtime().block_on(async move {
            let mut connected = false;
            let mut sent_echo = false;
            let mut output = Vec::new();
            let timer = tokio::time::sleep(Duration::from_secs(15));
            tokio::pin!(timer);

            loop {
                tokio::select! {
                    biased;
                    _ = &mut timer => break,
                    event = event_rx.recv() => match event {
                        Ok(SessionEvent::Connected) => {
                            connected = true;
                            // 风暴期间发出一个 echo，若终端被冻结就收不到。
                            let _ = input_for_task.send(InputCmd::Write(
                                b"echo CROSSH_ALIVE\r".to_vec()
                            )).await;
                            sent_echo = true;
                        }
                        Ok(SessionEvent::Output(bytes)) => {
                            output.extend_from_slice(&bytes);
                            let text = String::from_utf8_lossy(&output);
                            if sent_echo && text.contains("CROSSH_ALIVE") {
                                break;
                            }
                        }
                        Ok(SessionEvent::Error(message)) => {
                            panic!("local terminal error: {message}");
                        }
                        Ok(SessionEvent::Closed) | Err(_) => break,
                        Ok(SessionEvent::Cwd(_)) => {}
                        Ok(SessionEvent::ProcessInfo(_)) => {}
                    }
                }
            }

            assert!(connected, "local terminal did not connect");
            assert!(
                String::from_utf8_lossy(&output).contains("CROSSH_ALIVE"),
                "terminal froze under SIGCHLD storm: {:?}",
                String::from_utf8_lossy(&output)
            );
        });

        let spawned = flooder.join().unwrap();
        eprintln!("[test] spawned {spawned} children total");
        drop(input_tx);
    }

    #[cfg(unix)]
    #[test]
    fn local_terminal_reports_cwd_and_accepts_input() {
        let cwd = std::env::current_dir().unwrap();
        let expected_cwd = cwd.to_string_lossy().to_string();
        let (input_tx, event_rx) = open_terminal(cwd, 80, 24);
        let input_for_task = input_tx.clone();
        let expected_for_task = expected_cwd.clone();

        let (connected, reported_cwds, output, error) = crate::infrastructure::ssh::ssh_runtime()
            .block_on(async move {
                let mut connected = false;
                let mut reported_cwds = Vec::new();
                let mut output = Vec::new();
                let mut error = None;
                let mut sent_cd = false;
                let timer = tokio::time::sleep(Duration::from_secs(5));
                tokio::pin!(timer);

                loop {
                    tokio::select! {
                        biased;
                        _ = &mut timer => break,
                        event = event_rx.recv() => match event {
                            Ok(SessionEvent::Cwd(path)) => {
                                if sent_cd && path == "/tmp" {
                                    reported_cwds.push(path);
                                    break;
                                }
                                reported_cwds.push(path);
                            }
                            Ok(SessionEvent::Connected) => {
                                connected = true;
                                let _ = input_for_task
                                    .send(InputCmd::Write(b"pwd\r".to_vec()))
                                    .await;
                            }
                            Ok(SessionEvent::Output(bytes)) => {
                                output.extend_from_slice(&bytes);
                                if connected
                                    && !sent_cd
                                    && String::from_utf8_lossy(&output).contains(&expected_for_task)
                                {
                                    sent_cd = true;
                                    let _ = input_for_task
                                        .send(InputCmd::Write(b"cd /tmp\r".to_vec()))
                                        .await;
                                }
                            }
                            Ok(SessionEvent::Error(message)) => {
                                error = Some(message);
                                break;
                            }
                            Ok(SessionEvent::Closed) | Err(_) => break,
                            Ok(SessionEvent::ProcessInfo(_)) => {}
                        }
                    }
                }

                (connected, reported_cwds, output, error)
            });
        drop(input_tx);

        assert!(connected, "local PTY did not connect: {error:?}");
        assert!(reported_cwds.iter().any(|cwd| cwd == &expected_cwd));
        assert!(reported_cwds.iter().any(|cwd| cwd == "/tmp"));
        assert!(
            String::from_utf8_lossy(&output).contains(&expected_cwd),
            "local shell output did not contain pwd: {:?}",
            String::from_utf8_lossy(&output)
        );
    }

    #[cfg(unix)]
    #[test]
    fn local_shell_keeps_login_path_with_minimal_env() {
        let original_shell = std::env::var_os("SHELL");
        let original_path = std::env::var_os("PATH");
        let original_term = std::env::var_os("TERM");
        unsafe {
            // LaunchServices 注入 SHELL 但 PATH 是极简的 launchd 默认值：
            // 必须完整复现这个组合才能触发 ZDOTDIR 重定向的缺陷。
            std::env::set_var("SHELL", "/bin/zsh");
            std::env::set_var("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
        }

        let cwd = std::env::current_dir().unwrap();
        let (input_tx, event_rx) = open_terminal(cwd, 80, 24);
        let input_for_task = input_tx.clone();

        let output = crate::infrastructure::ssh::ssh_runtime().block_on(async move {
            let mut output = Vec::new();
            let timer = tokio::time::sleep(Duration::from_secs(5));
            tokio::pin!(timer);

            loop {
                tokio::select! {
                    biased;
                    _ = &mut timer => break,
                    event = event_rx.recv() => match event {
                        Ok(SessionEvent::Connected) => {
                            let _ = input_for_task
                                .send(InputCmd::Write(b"echo CROSSH_PATH=$PATH; exit\r".to_vec()))
                                .await;
                        }
                        Ok(SessionEvent::Output(bytes)) => output.extend_from_slice(&bytes),
                        Ok(SessionEvent::Error(message)) => {
                            output.extend_from_slice(format!("[error] {message}").as_bytes());
                            break;
                        }
                        Ok(SessionEvent::Closed) | Err(_) => break,
                        Ok(SessionEvent::Cwd(_)) => {}
                        Ok(SessionEvent::ProcessInfo(_)) => {}
                    }
                }
            }
            output
        });
        drop(input_tx);

        unsafe {
            match original_shell {
                Some(shell) => std::env::set_var("SHELL", shell),
                None => std::env::remove_var("SHELL"),
            }
            match original_path {
                Some(path) => std::env::set_var("PATH", path),
                None => std::env::remove_var("PATH"),
            }
            match original_term {
                Some(term) => std::env::set_var("TERM", term),
                None => std::env::remove_var("TERM"),
            }
        }

        let text = String::from_utf8_lossy(&output);
        assert!(
            text.contains("/opt/homebrew/bin") || text.contains("/usr/local/bin"),
            "local shell lost login PATH without SHELL env: {:?}",
            text
        );
    }
}

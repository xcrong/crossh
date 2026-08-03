//! 本地终端：使用和远端终端相同的输入/事件桥接，但在本机 PTY 中启动 shell。

#[cfg(unix)]
use std::fs::{self, File};
#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(unix)]
use std::sync::{Arc, Mutex};

#[cfg(unix)]
use alacritty_terminal::event::{OnResize, WindowSize};
#[cfg(unix)]
use alacritty_terminal::tty;
use async_channel::{Receiver, Sender};
#[cfg(unix)]
use tokio::io::unix::AsyncFd;

use crate::ssh::{InputCmd, SessionEvent};

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

    crate::ssh::ssh_runtime().spawn(async move {
        if let Err(error) =
            run_local_terminal(cwd, cols, rows, pty_id, input_rx, event_tx.clone()).await
        {
            let _ = event_tx.send(SessionEvent::Error(error.to_string())).await;
        }
        let _ = event_tx.send(SessionEvent::Closed).await;
    });

    (input_tx, event_rx)
}

#[cfg(not(unix))]
pub fn open_terminal(
    _cwd: PathBuf,
    _cols: u16,
    _rows: u16,
) -> (Sender<InputCmd>, Receiver<SessionEvent>) {
    let (input_tx, _input_rx) = async_channel::bounded::<InputCmd>(1024);
    let (event_tx, event_rx) = async_channel::bounded::<SessionEvent>(4);

    crate::ssh::ssh_runtime().spawn(async move {
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
    let pty = Arc::new(Mutex::new(pty));

    let display_cwd = cwd.to_string_lossy().to_string();
    let _ = event_tx.send(SessionEvent::Cwd(display_cwd)).await;
    let _ = event_tx.send(SessionEvent::Connected).await;

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

    drop(pty);
    Ok(())
}

#[cfg(unix)]
async fn read_local_output(
    reader: AsyncFd<File>,
    event_tx: Sender<SessionEvent>,
) -> io::Result<()> {
    let mut cwd_parser = CwdParser::default();
    loop {
        let mut guard = reader.readable().await?;
        let mut buffer = [0u8; 32 * 1024];
        match guard.try_io(|inner| inner.get_ref().read(&mut buffer)) {
            Ok(Ok(0)) => return Ok(()),
            Ok(Ok(size)) => {
                for cwd in cwd_parser.feed(&buffer[..size]) {
                    let _ = event_tx.send(SessionEvent::Cwd(cwd)).await;
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
    while let Ok(command) = input_rx.recv().await {
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
        let report = r#"printf '\033]133;A\007\033]7;file://localhost%s\007' "$PWD""#;
        let original = std::env::var("PROMPT_COMMAND").unwrap_or_default();
        let prompt_command = if original.is_empty() {
            report.to_string()
        } else {
            format!("{report};{original}")
        };
        options
            .env
            .insert("PROMPT_COMMAND".to_string(), prompt_command);
        return None;
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
__crossh_report_prompt() { printf '\\033]133;A\\007'; __crossh_report_pwd; }\n\
add-zsh-hook precmd __crossh_report_prompt\n\
add-zsh-hook chpwd __crossh_report_pwd\n"
            .to_string()
    } else {
        format!(
            "if [[ -r {original_rc} ]]; then source {original_rc}; fi\n\
autoload -Uz add-zsh-hook\n\
__crossh_report_pwd() {{ printf '\\033]7;file://localhost%s\\007' \"$PWD\"; }}\n\
__crossh_report_prompt() {{ printf '\\033]133;A\\007'; __crossh_report_pwd; }}\n\
add-zsh-hook precmd __crossh_report_prompt\n\
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

#[derive(Default)]
struct CwdParser {
    pending: Vec<u8>,
}

impl CwdParser {
    fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(bytes);
        let marker = b"\x1b]7;";
        let mut paths = Vec::new();

        loop {
            let Some(start) = find_bytes(&self.pending, marker) else {
                let keep = marker.len().saturating_sub(1);
                if self.pending.len() > keep {
                    let drain_to = self.pending.len() - keep;
                    self.pending.drain(..drain_to);
                }
                break;
            };
            if start > 0 {
                self.pending.drain(..start);
            }

            let payload_start = marker.len();
            let bel = self.pending[payload_start..]
                .iter()
                .position(|byte| *byte == 0x07)
                .map(|idx| (payload_start + idx, 1));
            let st = find_bytes(&self.pending[payload_start..], b"\x1b\\")
                .map(|idx| (payload_start + idx, 2));
            let Some((end, terminator_len)) = (match (bel, st) {
                (Some(bel), Some(st)) => Some(bel.min(st)),
                (Some(bel), None) => Some(bel),
                (None, Some(st)) => Some(st),
                (None, None) => None,
            }) else {
                break;
            };

            let payload = String::from_utf8_lossy(&self.pending[payload_start..end]);
            if let Some(path) = cwd_from_osc7(&payload) {
                paths.push(path);
            }
            self.pending.drain(..end + terminator_len);
        }
        paths
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn cwd_from_osc7(value: &str) -> Option<String> {
    let path = if let Some(rest) = value.strip_prefix("file://") {
        if rest.starts_with('/') {
            rest.to_string()
        } else {
            rest.find('/').map(|index| rest[index..].to_string())?
        }
    } else {
        value.to_string()
    };
    let path = percent_decode(&path)?;
    Path::new(&path).is_absolute().then_some(path)
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hi = hex_value(*bytes.get(index + 1)?)?;
            let lo = hex_value(*bytes.get(index + 2)?)?;
            decoded.push((hi << 4) | lo);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::time::Duration;

    #[test]
    fn cwd_parser_handles_split_osc7_sequences() {
        let mut parser = CwdParser::default();
        assert!(parser.feed(b"\x1b]7;file://localhost/Users/me").is_empty());
        assert_eq!(parser.feed(b"%20project\x07"), vec!["/Users/me project"]);
    }

    #[test]
    fn cwd_parser_accepts_st_terminator() {
        let mut parser = CwdParser::default();
        assert_eq!(
            parser.feed(b"\x1b]7;file:///tmp/crossh\x1b\\"),
            vec!["/tmp/crossh"]
        );
    }

    #[test]
    fn percent_decode_rejects_invalid_sequences() {
        assert_eq!(percent_decode("a%20b"), Some("a b".into()));
        assert_eq!(percent_decode("a%2"), None);
        assert_eq!(percent_decode("a%GG"), None);
    }

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
    fn local_terminal_reports_cwd_and_accepts_input() {
        let cwd = std::env::current_dir().unwrap();
        let expected_cwd = cwd.to_string_lossy().to_string();
        let (input_tx, event_rx) = open_terminal(cwd, 80, 24);
        let input_for_task = input_tx.clone();
        let expected_for_task = expected_cwd.clone();

        let (connected, reported_cwds, output, error) =
            crate::ssh::ssh_runtime().block_on(async move {
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

        let output = crate::ssh::ssh_runtime().block_on(async move {
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

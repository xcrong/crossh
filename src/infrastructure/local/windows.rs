//! Windows local terminal relay backed by alacritty's ConPTY implementation.

#[cfg(windows)]
use std::io::{self, Read, Write};
#[cfg(windows)]
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(windows)]
use std::sync::{Arc, Mutex};
#[cfg(any(windows, test))]
use std::time::{Duration, Instant};

#[cfg(windows)]
use alacritty_terminal::event::{OnResize, WindowSize};
#[cfg(windows)]
use alacritty_terminal::tty;
#[cfg(windows)]
use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite};
#[cfg(windows)]
use async_channel::{Receiver, Sender};

#[cfg(windows)]
use crate::infrastructure::ssh::ssh_runtime;
#[cfg(windows)]
use crate::shared::terminal::{InputCmd, SessionEvent};

#[cfg(windows)]
static NEXT_LOCAL_PTY_ID: AtomicU64 = AtomicU64::new(1);

#[cfg(windows)]
pub fn open_terminal(
    cwd: PathBuf,
    cols: u16,
    rows: u16,
) -> (Sender<InputCmd>, Receiver<SessionEvent>) {
    let (input_tx, input_rx) = async_channel::bounded::<InputCmd>(1024);
    let (event_tx, event_rx) = async_channel::bounded::<SessionEvent>(256);
    let pty_id = NEXT_LOCAL_PTY_ID.fetch_add(1, Ordering::Relaxed);

    ssh_runtime().spawn(async move {
        if let Err(error) =
            run_local_terminal(cwd, cols, rows, pty_id, input_rx, event_tx.clone()).await
        {
            let _ = event_tx.send(SessionEvent::Error(error.to_string())).await;
        }
        let _ = event_tx.send(SessionEvent::Closed).await;
    });

    (input_tx, event_rx)
}

#[cfg(windows)]
async fn run_local_terminal(
    cwd: PathBuf,
    cols: u16,
    rows: u16,
    pty_id: u64,
    input_rx: Receiver<InputCmd>,
    event_tx: Sender<SessionEvent>,
) -> io::Result<()> {
    let options = terminal_options(cwd.clone());
    let window_size = WindowSize {
        num_lines: rows,
        num_cols: cols,
        cell_width: 8,
        cell_height: 18,
    };
    let pty = Arc::new(Mutex::new(tty::new(&options, window_size, pty_id)?));

    // PowerShell reports Cwd and command completion through its prompt hook.
    // cmd.exe has no equivalent prompt callback, so it remains output-only.
    let display_cwd = cwd.to_string_lossy().to_string();
    let _ = event_tx.send(SessionEvent::Cwd(display_cwd)).await;
    let _ = event_tx.send(SessionEvent::Connected).await;

    let mut read_task = tokio::spawn(read_output(pty.clone(), event_tx.clone()));
    let mut write_task = tokio::spawn(drive_input(input_rx, pty.clone()));

    tokio::select! {
        result = &mut read_task => {
            write_task.abort();
            let _ = write_task.await;
            if let Ok(Err(error)) = result {
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

    // Drop the reader/writer adapters before the ConPTY backend. alacritty's
    // backend documents that closing the pseudoconsole can block until conout
    // has been drained, so the relay tasks must have stopped first.
    drop(pty);
    Ok(())
}

#[cfg(windows)]
fn terminal_options(cwd: PathBuf) -> tty::Options {
    let mut options = tty::Options {
        working_directory: Some(cwd),
        drain_on_exit: true,
        shell: Some(windows_shell()),
        ..Default::default()
    };
    options
        .env
        .insert("TERM".to_string(), "xterm-256color".to_string());
    options
        .env
        .insert("COLORTERM".to_string(), "truecolor".to_string());
    options
        .env
        .insert("TERM_PROGRAM".to_string(), "crossh".to_string());
    options
}

#[cfg(windows)]
fn windows_shell() -> tty::Shell {
    if windows_command_available("powershell.exe") {
        // Keep the user's profile semantics intact, matching the login shell
        // behavior used by the Unix backend. CI runners provide a clean
        // profile, while interactive users retain their normal setup.
        return tty::Shell::new(
            "powershell.exe".to_string(),
            vec![
                "-NoLogo".to_string(),
                "-NoExit".to_string(),
                "-Command".to_string(),
                powershell_shell_integration(),
            ],
        );
    }

    let command = std::env::var_os("COMSPEC")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "cmd.exe".into());
    tty::Shell::new(command.to_string_lossy().to_string(), Vec::new())
}

#[cfg(windows)]
fn powershell_shell_integration() -> String {
    // Use [char] escapes instead of PowerShell 7's backtick-e syntax so this
    // also works with the Windows PowerShell 5 that ships with older hosts.
    "$global:__crossh_original_prompt = $function:prompt; function global:prompt { $status = if ($?) { 0 } else { 1 }; $esc = [char]27; $bel = [char]7; $path = (Get-Location).Path.Replace('\\', '/'); [Console]::Write($esc + ']133;D;' + $status + $bel + $esc + ']133;B' + $bel + $esc + ']7;file://localhost' + $path + $bel + $esc + ']133;A' + $bel); if ($global:__crossh_original_prompt) { & $global:__crossh_original_prompt } else { 'PS ' + $path + '> ' } }".to_string()
}

#[cfg(windows)]
fn windows_command_available(command: &str) -> bool {
    let command_path = Path::new(command);
    if command_path.components().count() > 1 {
        return command_path.is_file();
    }

    if let Some(path) = std::env::var_os("PATH")
        && std::env::split_paths(&path).any(|directory| directory.join(command).is_file())
    {
        return true;
    }

    // Windows PowerShell is present on supported Windows runners even when a
    // stripped-down PATH omits its directory.
    std::env::var_os("WINDIR").is_some_and(|windir| {
        PathBuf::from(windir)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join(command)
            .is_file()
    })
}

#[cfg(any(windows, test))]
const DRAIN_QUIET_WINDOW: Duration = Duration::from_millis(50);
#[cfg(any(windows, test))]
const MIN_POLL_DELAY: Duration = Duration::from_millis(1);
#[cfg(any(windows, test))]
const MAX_POLL_DELAY: Duration = Duration::from_millis(16);

/// Tracks the post-exit drain window without tying it to the reader loop.
///
/// A ConPTY child-exit notification can arrive before the final bytes have
/// been copied out of the reader adapter. The quiet timer starts at the first
/// empty read after exit and is reset whenever data arrives.
#[cfg(any(windows, test))]
#[derive(Default)]
struct OutputDrainState {
    child_exited: bool,
    quiet_since: Option<Instant>,
}

#[cfg(any(windows, test))]
impl OutputDrainState {
    fn observe(&mut self, size: usize, exited: bool, now: Instant) -> bool {
        self.child_exited |= exited;
        if size > 0 {
            self.quiet_since = None;
            return false;
        }
        if !self.child_exited {
            return false;
        }

        let quiet_since = self.quiet_since.get_or_insert(now);
        now.saturating_duration_since(*quiet_since) >= DRAIN_QUIET_WINDOW
    }
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy)]
struct PollBackoff {
    delay: Duration,
}

#[cfg(any(windows, test))]
impl Default for PollBackoff {
    fn default() -> Self {
        Self {
            delay: MIN_POLL_DELAY,
        }
    }
}

#[cfg(any(windows, test))]
impl PollBackoff {
    fn delay(self) -> Duration {
        self.delay
    }

    fn reset(&mut self) {
        self.delay = MIN_POLL_DELAY;
    }

    fn on_empty(&mut self) {
        self.delay = self.delay.saturating_mul(2).min(MAX_POLL_DELAY);
    }
}

#[cfg(windows)]
async fn read_output(pty: Arc<Mutex<tty::Pty>>, event_tx: Sender<SessionEvent>) -> io::Result<()> {
    let mut drain = OutputDrainState::default();
    let mut poll_backoff = PollBackoff::default();
    let mut buffer = vec![0u8; 32 * 1024];
    loop {
        let (size, exited) = {
            let mut pty = pty
                .lock()
                .map_err(|_| io::Error::other("local ConPTY state lock was poisoned"))?;
            let size = pty.reader().read(&mut buffer)?;
            let exited = matches!(pty.next_child_event(), Some(ChildEvent::Exited(_)));
            (size, exited)
        };

        if size > 0 {
            drain.observe(size, exited, Instant::now());
            poll_backoff.reset();
            if event_tx
                .send(SessionEvent::Output(buffer[..size].to_vec()))
                .await
                .is_err()
            {
                return Ok(());
            }
            continue;
        }

        let now = Instant::now();
        // A child-exit notification races with the final pipe drain. Keep
        // polling until the reader has been quiet for the full drain window,
        // preserving bytes that arrive after the notification.
        if drain.observe(size, exited, now) {
            return Ok(());
        }
        tokio::time::sleep(poll_backoff.delay()).await;
        poll_backoff.on_empty();
    }
}

#[cfg(windows)]
async fn drive_input(input_rx: Receiver<InputCmd>, pty: Arc<Mutex<tty::Pty>>) -> io::Result<()> {
    while let Ok(command) = input_rx.recv().await {
        match command {
            InputCmd::Write(bytes) => write_all(&pty, &bytes).await?,
            InputCmd::Resize { cols, rows } => {
                let mut pty = pty
                    .lock()
                    .map_err(|_| io::Error::other("local ConPTY state lock was poisoned"))?;
                pty.on_resize(WindowSize {
                    num_lines: rows,
                    num_cols: cols,
                    cell_width: 8,
                    cell_height: 18,
                });
            }
            InputCmd::Close => break,
        }
    }
    Ok(())
}

#[cfg(windows)]
async fn write_all(pty: &Arc<Mutex<tty::Pty>>, bytes: &[u8]) -> io::Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        let written = {
            let mut pty = pty
                .lock()
                .map_err(|_| io::Error::other("local ConPTY state lock was poisoned"))?;
            pty.writer().write(&bytes[offset..])?
        };
        if written == 0 {
            // UnblockedWriter reports zero while its internal pipe is full.
            // Yield briefly and retry rather than dropping keyboard bytes.
            tokio::time::sleep(Duration::from_millis(2)).await;
        } else {
            offset += written;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn output_drain_waits_for_a_quiet_window() {
        let start = std::time::Instant::now();
        let mut drain = OutputDrainState::default();

        assert!(!drain.observe(0, false, start));
        assert!(!drain.observe(0, true, start));
        assert!(!drain.observe(8, false, start + Duration::from_millis(10)));
        assert!(!drain.observe(0, false, start + Duration::from_millis(59)));
        assert!(drain.observe(0, false, start + Duration::from_millis(109)));
    }

    #[test]
    fn output_polling_backoff_resets_after_data() {
        let mut backoff = PollBackoff::default();
        assert_eq!(backoff.delay(), Duration::from_millis(1));
        backoff.on_empty();
        assert_eq!(backoff.delay(), Duration::from_millis(2));
        backoff.on_empty();
        assert_eq!(backoff.delay(), Duration::from_millis(4));
        backoff.on_empty();
        backoff.on_empty();
        backoff.on_empty();
        assert_eq!(backoff.delay(), Duration::from_millis(16));
        backoff.on_empty();
        assert_eq!(backoff.delay(), Duration::from_millis(16));
        backoff.reset();
        assert_eq!(backoff.delay(), Duration::from_millis(1));
    }

    /// Keep a short, deterministic ConPTY smoke test in the Windows test
    /// suite. It exercises process creation, input, output, and child exit
    /// without depending on a user's profile or shell prompt.
    #[cfg(windows)]
    #[test]
    fn windows_conpty_smoke_round_trip() {
        const TOKEN: &str = "CROSSH_CONPTY_SMOKE";
        let cwd = std::env::current_dir().expect("current directory");
        let (input_tx, event_rx) = open_terminal(cwd, 80, 24);

        let (connected, output, error, closed) = crate::infrastructure::ssh::ssh_runtime().block_on(async move {
            let mut connected = false;
            let mut output = Vec::new();
            let mut error = None;
            let mut closed = false;
            let timer = tokio::time::sleep(Duration::from_secs(10));
            tokio::pin!(timer);

            loop {
                tokio::select! {
                    biased;
                    _ = &mut timer => break,
                    event = event_rx.recv() => match event {
                        Ok(SessionEvent::Connected) => {
                            connected = true;
                            let _ = input_tx.send(InputCmd::Write(format!("echo {TOKEN}\r").into_bytes())).await;
                            // Send exit separately and require both the PTY
                            // echo and command output below. A single token
                            // occurrence could otherwise be a local line echo.
                            let _ = input_tx.send(InputCmd::Write(b"exit\r".to_vec())).await;
                        }
                        Ok(SessionEvent::Output(bytes)) => {
                            output.extend_from_slice(&bytes);
                        }
                        Ok(SessionEvent::Error(message)) => {
                            error = Some(message);
                            break;
                        }
                        Ok(SessionEvent::Closed) | Err(_) => {
                            closed = true;
                            break;
                        }
                        Ok(SessionEvent::Cwd(_)) => {}
                    }
                }
            }

            // This is a no-op after normal child exit and guarantees cleanup on
            // timeout, so a failed smoke test cannot leak a shell process.
            let _ = input_tx.send(InputCmd::Close).await;
            (connected, output, error, closed)
        });

        let output = String::from_utf8_lossy(&output);
        assert!(connected, "ConPTY did not connect: {error:?}");
        assert!(error.is_none(), "ConPTY relay error: {error:?}");
        assert!(closed, "ConPTY did not close: {output:?}");
        assert!(
            output.matches(TOKEN).count() >= 2,
            "ConPTY output did not contain both echo and command token: {output:?}"
        );
    }
}

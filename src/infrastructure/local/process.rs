use std::os::fd::{AsRawFd, RawFd};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use alacritty_terminal::tty;
use async_channel::{Sender, TrySendError};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::shared::terminal::{SessionEvent, TerminalProcessInfo};

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(500);
const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(500);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(5);

/// Low-frequency snapshotter for the process attached to a local PTY.
///
/// The foreground process group changes when a shell launches a command such
/// as vim, git, or cargo. Reading it from the PTY lets tab titles follow the
/// same useful state that Zed exposes without parsing shell prompt text.
pub(crate) struct ProcessTracker {
    system: System,
    fallback_pid: Pid,
    last_pid: Option<Pid>,
    last_info: Option<TerminalProcessInfo>,
    retry_after: Option<Instant>,
    retry_delay: Duration,
    pty_fd: RawFd,
}

impl ProcessTracker {
    pub(crate) fn new(pty: &tty::Pty) -> Option<Self> {
        Some(Self {
            system: System::new(),
            fallback_pid: Pid::from_u32(pty.child().id()),
            last_pid: None,
            last_info: None,
            retry_after: None,
            retry_delay: INITIAL_RETRY_DELAY,
            pty_fd: pty.file().as_raw_fd(),
        })
    }

    /// Return a process snapshot that has not been acknowledged as delivered.
    pub(crate) fn changed(&mut self) -> Option<TerminalProcessInfo> {
        let now = Instant::now();
        if self
            .retry_after
            .is_some_and(|retry_after| retry_after > now)
        {
            return None;
        }

        let Some(info) = self.load() else {
            // A process can be briefly unavailable while a foreground command
            // is changing. Retry with backoff instead of permanently disabling
            // dynamic titles after one transient failure.
            self.retry_after = Some(now + self.retry_delay);
            self.retry_delay = self.retry_delay.saturating_mul(2).min(MAX_RETRY_DELAY);
            return None;
        };
        self.retry_after = None;
        self.retry_delay = INITIAL_RETRY_DELAY;
        (self.last_info.as_ref() != Some(&info)).then_some(info)
    }

    /// Mark a snapshot as delivered only after the event channel accepted it.
    pub(crate) fn acknowledge(&mut self, info: &TerminalProcessInfo) {
        self.last_info = Some(info.clone());
    }

    /// Poll from a dedicated blocking worker so sysinfo cannot stall SSH/I/O
    /// runtime workers. A full event channel retains the pending snapshot.
    pub(crate) fn run(mut self, event_tx: Sender<SessionEvent>, cancel: Arc<AtomicBool>) {
        let mut pending = None;
        while !cancel.load(Ordering::Acquire) {
            if pending.is_none() {
                pending = self.changed();
            }

            if let Some(info) = pending.take() {
                match event_tx.try_send(SessionEvent::ProcessInfo(info.clone())) {
                    Ok(()) => self.acknowledge(&info),
                    Err(TrySendError::Full(_)) => pending = Some(info),
                    Err(TrySendError::Closed(_)) => break,
                }
            }

            thread::sleep(PROCESS_POLL_INTERVAL);
        }
    }

    fn load(&mut self) -> Option<TerminalProcessInfo> {
        let foreground_pid = self.foreground_pid();
        let candidate_pids = foreground_pid
            .into_iter()
            .chain(std::iter::once(self.fallback_pid))
            .collect::<Vec<_>>();

        if self.last_pid != candidate_pids.first().copied() {
            self.system = System::new();
            self.last_pid = candidate_pids.first().copied();
        }

        let refresh_kind = ProcessRefreshKind::nothing().with_cwd(UpdateKind::Always);
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&candidate_pids),
            true,
            refresh_kind,
        );
        let pid = candidate_pids
            .into_iter()
            .find(|pid| self.system.process(*pid).is_some())?;
        let process = self.system.process(pid)?;
        let name = process.name().to_string_lossy().into_owned();
        if name.is_empty() {
            return None;
        }

        Some(TerminalProcessInfo {
            name,
            cwd: process.cwd().map(|cwd| cwd.to_string_lossy().into_owned()),
        })
    }

    fn foreground_pid(&self) -> Option<Pid> {
        let pid = unsafe { libc::tcgetpgrp(self.pty_fd) };
        (pid > 0).then(|| Pid::from_u32(pid as u32))
    }
}

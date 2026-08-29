use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use async_channel::Receiver;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackgroundTaskStatus {
    Running,
    Stopping,
    Succeeded,
    Failed,
    Terminated,
}

#[derive(Clone, Debug)]
pub struct BackgroundTask {
    pub id: u64,
    pub owner: String,
    pub scope: String,
    pub command: String,
    pub cwd: PathBuf,
    pub status: BackgroundTaskStatus,
}

#[derive(Debug)]
pub struct BackgroundTaskEvent {
    pub id: u64,
    pub status: BackgroundTaskStatus,
}

struct BackgroundControl {
    stop_requested: AtomicBool,
    pid: AtomicU32,
}

impl BackgroundControl {
    fn request_stop(&self) {
        self.stop_requested.store(true, Ordering::Release);
        let pid = self.pid.load(Ordering::Acquire);
        if pid != 0 {
            terminate_process(pid, false);
        }
    }
}

pub struct BackgroundTaskManager {
    next_id: u64,
    pub tasks: BTreeMap<u64, BackgroundTask>,
    controls: BTreeMap<u64, Arc<BackgroundControl>>,
}

impl Default for BackgroundTaskManager {
    fn default() -> Self {
        Self {
            next_id: 1,
            tasks: BTreeMap::new(),
            controls: BTreeMap::new(),
        }
    }
}

impl BackgroundTaskManager {
    pub fn start(
        &mut self,
        scope: String,
        cwd: PathBuf,
        command: String,
        owner: String,
    ) -> (u64, Receiver<BackgroundTaskEvent>) {
        let id = self.insert_task(scope, cwd.clone(), command.clone(), owner);
        let control = Arc::new(BackgroundControl {
            stop_requested: AtomicBool::new(false),
            pid: AtomicU32::new(0),
        });
        let (event_tx, event_rx) = async_channel::bounded(1);
        self.controls.insert(id, control.clone());
        thread::spawn(move || run_background_process(id, cwd, command, control, event_tx));
        (id, event_rx)
    }

    /// Reserve a task id and display entry for a command executed by another
    /// process, such as an SSH channel. Its completion event is applied by the
    /// owner of that process.
    pub fn start_remote(
        &mut self,
        scope: String,
        cwd: PathBuf,
        command: String,
        owner: String,
    ) -> u64 {
        self.insert_task(scope, cwd, command, owner)
    }

    fn insert_task(&mut self, scope: String, cwd: PathBuf, command: String, owner: String) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.tasks.insert(
            id,
            BackgroundTask {
                id,
                owner,
                scope,
                command,
                cwd,
                status: BackgroundTaskStatus::Running,
            },
        );
        id
    }

    pub fn mark_stopping(&mut self, id: u64) {
        if let Some(task) = self.tasks.get_mut(&id)
            && task.status == BackgroundTaskStatus::Running
        {
            task.status = BackgroundTaskStatus::Stopping;
        }
        if let Some(control) = self.controls.get(&id) {
            control.request_stop();
        }
    }

    pub fn apply_event(&mut self, event: BackgroundTaskEvent) {
        // Completed tasks leave the panel immediately; nothing about the
        // result is retained in the task list.
        let id = event.id;
        self.controls.remove(&id);
        self.tasks.remove(&id);
    }

    pub fn running_count(&self) -> usize {
        self.tasks
            .values()
            .filter(|task| {
                matches!(
                    task.status,
                    BackgroundTaskStatus::Running | BackgroundTaskStatus::Stopping
                )
            })
            .count()
    }

    pub fn tasks_for_scope(&self, scope: &str) -> Vec<BackgroundTask> {
        self.tasks
            .values()
            .filter(|task| task.scope == scope)
            .cloned()
            .rev()
            .collect()
    }

    pub fn active_for_command(&self, scope: &str, command: &str) -> Vec<u64> {
        self.tasks
            .values()
            .filter(|task| {
                task.scope == scope
                    && task.command == command
                    && matches!(
                        task.status,
                        BackgroundTaskStatus::Running | BackgroundTaskStatus::Stopping
                    )
            })
            .map(|task| task.id)
            .collect()
    }

    pub fn running_for_command(&self, scope: &str, command: &str) -> Vec<u64> {
        self.tasks
            .values()
            .filter(|task| {
                task.scope == scope
                    && task.command == command
                    && task.status == BackgroundTaskStatus::Running
            })
            .map(|task| task.id)
            .collect()
    }

    pub fn active_for_owner(&self, owner: &str) -> Vec<u64> {
        self.tasks
            .values()
            .filter(|task| {
                task.owner == owner
                    && matches!(
                        task.status,
                        BackgroundTaskStatus::Running | BackgroundTaskStatus::Stopping
                    )
            })
            .map(|task| task.id)
            .collect()
    }
}

fn run_background_process(
    id: u64,
    cwd: PathBuf,
    command: String,
    control: Arc<BackgroundControl>,
    event_tx: async_channel::Sender<BackgroundTaskEvent>,
) {
    let mut process = shell_command(&command, &cwd);
    let mut child = match process.spawn() {
        Ok(child) => child,
        Err(_) => {
            let _ = event_tx.send_blocking(BackgroundTaskEvent {
                id,
                status: BackgroundTaskStatus::Failed,
            });
            return;
        }
    };
    control.pid.store(child.id(), Ordering::Release);

    let started = Instant::now();
    let mut stop_sent = false;
    let status = loop {
        if control.stop_requested.load(Ordering::Acquire) {
            if !stop_sent {
                terminate_process(child.id(), false);
                stop_sent = true;
            } else if started.elapsed() >= Duration::from_millis(700) {
                terminate_process(child.id(), true);
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => thread::sleep(Duration::from_millis(40)),
            Err(_) => break None,
        }
    };
    control.pid.store(0, Ordering::Release);

    let status = if control.stop_requested.load(Ordering::Acquire) {
        BackgroundTaskStatus::Terminated
    } else if status.as_ref().is_some_and(|status| status.success()) {
        BackgroundTaskStatus::Succeeded
    } else {
        BackgroundTaskStatus::Failed
    };
    let _ = event_tx.send_blocking(BackgroundTaskEvent { id, status });
}

fn shell_command(command: &str, cwd: &Path) -> Command {
    #[cfg(windows)]
    {
        let shell = std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into());
        let mut process = Command::new(shell);
        process.args(["/D", "/S", "/C", command]);
        process.current_dir(cwd);
        crate::process::null_stdio(&mut process);
        crate::process::detach(&mut process);
        return process;
    }

    #[cfg(not(windows))]
    {
        let shell = std::env::var_os("SHELL").unwrap_or_else(|| "sh".into());
        let mut process = Command::new(shell);
        process.args(["-lc", command]);
        process.current_dir(cwd);
        crate::process::null_stdio(&mut process);
        crate::process::detach(&mut process);
        process
    }
}

fn terminate_process(pid: u32, force: bool) {
    #[cfg(unix)]
    unsafe {
        let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
        let _ = libc::killpg(pid as libc::pid_t, signal);
        let _ = libc::kill(pid as libc::pid_t, signal);
    }

    #[cfg(windows)]
    {
        let mut taskkill = Command::new("taskkill");
        taskkill.args(["/PID", &pid.to_string(), "/T"]);
        if force {
            taskkill.arg("/F");
        }
        let _ = taskkill.output();
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::commands::history::local_scope;

    #[test]
    fn remote_background_manager_tracks_completion() {
        let mut manager = BackgroundTaskManager::default();
        let owner = "remote-terminal:1";
        let id = manager.start_remote(
            "remote:example.com:22:/srv/app".into(),
            PathBuf::from("/srv/app"),
            "deploy".into(),
            owner.into(),
        );
        assert_eq!(manager.running_count(), 1);
        assert_eq!(manager.active_for_owner(owner), vec![id]);
        assert_eq!(
            manager
                .tasks_for_scope("remote:example.com:22:/srv/app")
                .len(),
            1
        );

        manager.mark_stopping(id);
        assert_eq!(manager.tasks[&id].status, BackgroundTaskStatus::Stopping);
        manager.apply_event(BackgroundTaskEvent {
            id,
            status: BackgroundTaskStatus::Terminated,
        });
        assert_eq!(manager.running_count(), 0);
        assert!(
            manager
                .tasks_for_scope("remote:example.com:22:/srv/app")
                .is_empty()
        );
        assert!(!manager.tasks.contains_key(&id));
        assert!(manager.active_for_owner(owner).is_empty());
    }

    #[test]
    fn background_manager_keeps_terminal_owners_isolated() {
        let mut manager = BackgroundTaskManager::default();
        let first = manager.start_remote(
            "local:/tmp/project".into(),
            PathBuf::from("/tmp/project"),
            "make".into(),
            "local-session:1".into(),
        );
        let second = manager.start_remote(
            "local:/tmp/project".into(),
            PathBuf::from("/tmp/project"),
            "make".into(),
            "local-session:2".into(),
        );

        assert_eq!(manager.active_for_owner("local-session:1"), vec![first]);
        assert_eq!(manager.active_for_owner("local-session:2"), vec![second]);

        manager.apply_event(BackgroundTaskEvent {
            id: first,
            status: BackgroundTaskStatus::Terminated,
        });
        assert!(manager.active_for_owner("local-session:1").is_empty());
        assert_eq!(manager.active_for_owner("local-session:2"), vec![second]);
    }

    #[test]
    fn task_listing_keeps_every_running_task_in_a_scope() {
        let mut manager = BackgroundTaskManager::default();
        let ids = (0..12)
            .map(|index| {
                manager.start_remote(
                    "local:/tmp/project".into(),
                    PathBuf::from("/tmp/project"),
                    format!("task-{index}"),
                    "local-session:1".into(),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            manager
                .tasks_for_scope("local:/tmp/project")
                .iter()
                .map(|task| task.id)
                .collect::<Vec<_>>(),
            ids.into_iter().rev().collect::<Vec<_>>()
        );
    }

    #[test]
    fn running_for_command_excludes_stopping_tasks() {
        let mut manager = BackgroundTaskManager::default();
        let id = manager.start_remote(
            "local:/tmp/project".into(),
            PathBuf::from("/tmp/project"),
            "make".into(),
            "local-session:1".into(),
        );

        assert_eq!(
            manager.running_for_command("local:/tmp/project", "make"),
            vec![id]
        );
        manager.mark_stopping(id);
        assert!(
            manager
                .running_for_command("local:/tmp/project", "make")
                .is_empty()
        );
    }

    #[cfg(unix)]
    #[test]
    fn background_manager_runs_a_command_and_reports_status() {
        let cwd = std::env::current_dir().expect("test cwd");
        let scope = local_scope(&cwd);
        let mut manager = BackgroundTaskManager::default();
        let (id, events) =
            manager.start(scope.clone(), cwd, "true".into(), "local-session:1".into());

        let event = events.recv_blocking().expect("background task event");
        assert_eq!(event.id, id);
        assert_eq!(event.status, BackgroundTaskStatus::Succeeded);

        manager.apply_event(event);
        assert!(manager.tasks_for_scope(&scope).is_empty());
        assert!(!manager.tasks.contains_key(&id));
    }
}

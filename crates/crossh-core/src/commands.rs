//! Working-directory-bound command history and background command execution.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use async_channel::Receiver;
use serde::{Deserialize, Serialize};

pub const MAX_HISTORY_ENTRIES: usize = 300;
pub const DISPLAY_LIMIT: usize = 30;
const MAX_COMMAND_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CommandRecord {
    pub command: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub last_used: u64,
}

#[derive(Default, Deserialize, Serialize)]
struct HistoryFile {
    #[serde(default = "history_file_version")]
    version: u32,
    #[serde(default)]
    scopes: BTreeMap<String, Vec<CommandRecord>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    ignored_commands: BTreeMap<String, Vec<String>>,
}

fn history_file_version() -> u32 {
    1
}

/// Cache-backed aggregate statistics. The config file is only a top-30
/// projection for the commands currently visible in the panel, plus the
/// cwd-bound commands excluded from future history.
pub struct CommandHistory {
    cache_path: PathBuf,
    config_path: Option<PathBuf>,
    scopes: BTreeMap<String, Vec<CommandRecord>>,
    ignored_commands: BTreeMap<String, Vec<String>>,
}

impl CommandHistory {
    pub fn load() -> Self {
        Self::from_paths(
            command_history_cache_path(),
            Some(quick_commands_config_path()),
        )
    }

    #[cfg(test)]
    fn from_path(path: PathBuf) -> Self {
        Self::from_paths(path, None)
    }

    fn from_paths(cache_path: PathBuf, config_path: Option<PathBuf>) -> Self {
        let cache_file = read_history_file(&cache_path);
        let config_file = config_path.as_deref().and_then(read_history_file);
        let scopes = cache_file
            .as_ref()
            .map(|file| file.scopes.clone())
            .or_else(|| config_file.as_ref().map(|file| file.scopes.clone()))
            .unwrap_or_default();
        let ignored_commands = config_file
            .as_ref()
            .map(|file| file.ignored_commands.clone())
            .unwrap_or_default();
        let mut history = Self {
            cache_path,
            config_path,
            scopes,
            ignored_commands,
        };
        normalize_scopes(&mut history.scopes);
        normalize_ignored_commands(&mut history.ignored_commands);
        remove_ignored_records(&mut history.scopes, &history.ignored_commands);
        history
    }

    pub fn top(&self, scope: &str) -> Vec<CommandRecord> {
        let mut records = self.scopes.get(scope).cloned().unwrap_or_default();
        records.retain(|record| !self.is_ignored(scope, &record.command));
        sort_records(&mut records);
        records.truncate(DISPLAY_LIMIT);
        records
    }

    pub fn total(&self, scope: &str) -> usize {
        self.scopes.get(scope).map_or(0, |records| {
            records
                .iter()
                .filter(|record| !self.is_ignored(scope, &record.command))
                .count()
        })
    }

    pub fn pinned(&self, scope: &str) -> Vec<CommandRecord> {
        self.top(scope)
            .into_iter()
            .filter(|record| record.pinned)
            .collect()
    }

    pub fn toggle_pinned(&mut self, scope: &str, command: &str) -> bool {
        let Some(record) = self
            .scopes
            .get_mut(scope)
            .and_then(|records| records.iter_mut().find(|record| record.command == command))
        else {
            return false;
        };
        record.pinned = !record.pinned;
        self.persist();
        true
    }

    pub fn record(&mut self, scope: &str, command: &str) -> bool {
        let Some(command) = normalize_command(command) else {
            return false;
        };
        if self.is_ignored(scope, &command) {
            return false;
        }
        let records = self.scopes.entry(scope.to_string()).or_default();
        let now = crate::format::unix_timestamp_secs();
        if let Some(record) = records.iter_mut().find(|record| record.command == command) {
            record.count = record.count.saturating_add(1);
            record.last_used = now;
        } else {
            records.push(CommandRecord {
                command,
                pinned: false,
                count: 1,
                last_used: now,
            });
        }
        sort_records(records);
        records.truncate(MAX_HISTORY_ENTRIES);
        self.persist();
        true
    }

    pub fn ignore(&mut self, scope: &str, command: &str) -> bool {
        let Some(command) = normalize_command(command) else {
            return false;
        };
        let added = {
            let ignored = self.ignored_commands.entry(scope.to_string()).or_default();
            if ignored.iter().any(|ignored| ignored == &command) {
                false
            } else {
                ignored.push(command.clone());
                true
            }
        };
        let removed = if let Some(records) = self.scopes.get_mut(scope) {
            let before = records.len();
            records.retain(|record| record.command != command);
            before != records.len()
        } else {
            false
        };
        if self.scopes.get(scope).is_some_and(Vec::is_empty) {
            self.scopes.remove(scope);
        }
        if added || removed {
            self.persist();
        }
        added || removed
    }

    fn is_ignored(&self, scope: &str, command: &str) -> bool {
        self.ignored_commands
            .get(scope)
            .is_some_and(|commands| commands.iter().any(|ignored| ignored == command))
    }

    pub fn edit(&mut self, scope: &str, original: &str, replacement: &str) -> bool {
        let Some(replacement) = normalize_command(replacement) else {
            return self.remove(scope, original);
        };
        if self.is_ignored(scope, &replacement) {
            return self.remove(scope, original);
        }
        let Some(records) = self.scopes.get_mut(scope) else {
            return false;
        };
        let Some(index) = records.iter().position(|record| record.command == original) else {
            return false;
        };
        if records[index].command == replacement {
            return false;
        }

        let existing_index = records
            .iter()
            .position(|record| record.command == replacement);
        if let Some(existing_index) = existing_index {
            let count = records[index].count;
            let last_used = records[index].last_used;
            let existing = &mut records[existing_index];
            existing.count = existing.count.saturating_add(count);
            existing.last_used = existing.last_used.max(last_used);
            records.remove(index);
        } else {
            records[index].command = replacement;
            records[index].last_used = crate::format::unix_timestamp_secs();
        }
        sort_records(records);
        records.truncate(MAX_HISTORY_ENTRIES);
        self.persist();
        true
    }

    pub fn remove(&mut self, scope: &str, command: &str) -> bool {
        let Some(records) = self.scopes.get_mut(scope) else {
            return false;
        };
        let before = records.len();
        records.retain(|record| record.command != command);
        let changed = records.len() != before;
        if records.is_empty() {
            self.scopes.remove(scope);
        }
        if changed {
            self.persist();
        }
        changed
    }

    fn persist(&self) {
        let cache_file = HistoryFile {
            version: history_file_version(),
            scopes: self.scopes.clone(),
            ignored_commands: BTreeMap::new(),
        };
        if let Err(error) = write_history_file(&self.cache_path, &cache_file) {
            log::warn!("failed to persist command history: {error}");
        }
        let Some(config_path) = &self.config_path else {
            return;
        };
        let mut config_scopes = self.scopes.clone();
        remove_ignored_records(&mut config_scopes, &self.ignored_commands);
        for records in config_scopes.values_mut() {
            sort_records(records);
            records.truncate(DISPLAY_LIMIT);
        }
        let config_file = HistoryFile {
            version: history_file_version(),
            scopes: config_scopes,
            ignored_commands: self.ignored_commands.clone(),
        };
        if let Err(error) = write_history_file(config_path, &config_file) {
            log::warn!("failed to persist quick command configuration: {error}");
        }
    }
}

fn read_history_file(path: &Path) -> Option<HistoryFile> {
    match fs::read_to_string(path) {
        Ok(contents) => match toml::from_str::<HistoryFile>(&contents) {
            Ok(file) => Some(file),
            Err(error) => {
                log::warn!("failed to parse command history: {error}");
                None
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            log::warn!("failed to read command history: {error}");
            None
        }
    }
}

#[cfg(test)]
fn read_scopes(path: &Path) -> Option<BTreeMap<String, Vec<CommandRecord>>> {
    read_history_file(path).map(|file| file.scopes)
}

fn normalize_scopes(scopes: &mut BTreeMap<String, Vec<CommandRecord>>) {
    for records in scopes.values_mut() {
        records.retain(|record| normalize_command(&record.command).is_some());
        sort_records(records);
        records.truncate(MAX_HISTORY_ENTRIES);
    }
    scopes.retain(|_, records| !records.is_empty());
}

fn normalize_ignored_commands(ignored: &mut BTreeMap<String, Vec<String>>) {
    for commands in ignored.values_mut() {
        let mut normalized = commands
            .drain(..)
            .filter_map(|command| normalize_command(&command))
            .collect::<Vec<_>>();
        normalized.sort();
        normalized.dedup();
        *commands = normalized;
    }
    ignored.retain(|_, commands| !commands.is_empty());
}

fn remove_ignored_records(
    scopes: &mut BTreeMap<String, Vec<CommandRecord>>,
    ignored: &BTreeMap<String, Vec<String>>,
) {
    for (scope, records) in scopes.iter_mut() {
        let Some(ignored_commands) = ignored.get(scope) else {
            continue;
        };
        records.retain(|record| {
            !ignored_commands
                .iter()
                .any(|ignored| ignored == &record.command)
        });
    }
    scopes.retain(|_, records| !records.is_empty());
}

fn write_history_file(path: &Path, file: &HistoryFile) -> io::Result<()> {
    let contents = toml::to_string_pretty(file)
        .map_err(|error| io::Error::other(format!("serialize history: {error}")))?;
    let Some(parent) = path.parent() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "history path has no parent",
        ));
    };
    fs::create_dir_all(parent).and_then(|_| atomic_write(path, &contents))
}

fn atomic_write(path: &Path, contents: &str) -> io::Result<()> {
    let temporary = path.with_extension("toml.tmp");
    fs::write(&temporary, contents)?;
    // Windows does not replace an existing destination during rename.
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)
}

fn sort_records(records: &mut [CommandRecord]) {
    records.sort_by(|left, right| {
        right.pinned.cmp(&left.pinned).then_with(|| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| right.last_used.cmp(&left.last_used))
                .then_with(|| left.command.cmp(&right.command))
        })
    });
}

fn normalize_command(command: &str) -> Option<String> {
    let command = command.trim();
    if command.is_empty() || command.len() > MAX_COMMAND_BYTES || command.contains('\0') {
        return None;
    }
    Some(command.to_string())
}

pub fn command_history_cache_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cache")
        .join("crossh")
        .join("command-history.toml")
}

pub fn quick_commands_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("crossh")
        .join("quick-commands.toml")
}

pub fn local_scope(cwd: &Path) -> String {
    format!("local:{}", cwd.to_string_lossy())
}

pub fn remote_scope(host_key: &str, cwd: &str) -> String {
    format!("remote:{host_key}:{cwd}")
}

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
        process
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        return process;
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::process::CommandExt;

        let shell = std::env::var_os("SHELL").unwrap_or_else(|| "sh".into());
        let mut process = Command::new(shell);
        process.args(["-lc", command]);
        process.current_dir(cwd);
        // stdio 全部置空：后台任务不与前台终端抢输入，输出不捕获。
        process
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            process.pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            });
        }
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
    use super::*;

    #[test]
    fn aggregates_commands_and_returns_top_thirty() {
        let path = std::env::temp_dir().join(format!(
            "crossh-command-history-{}.toml",
            std::process::id()
        ));
        let mut history = CommandHistory::from_path(path.clone());
        for index in 1..MAX_HISTORY_ENTRIES + 21 {
            history.record("local:/tmp/project", &format!("command-{index}"));
        }
        history.record("local:/tmp/project", "command-0");
        history.record("local:/tmp/project", "command-0");
        history.record("local:/tmp/project", "command-0");

        assert_eq!(history.total("local:/tmp/project"), MAX_HISTORY_ENTRIES);
        assert_eq!(history.top("local:/tmp/project")[0].command, "command-0");
        assert_eq!(history.top("local:/tmp/project")[0].count, 3);
        assert_eq!(history.top("local:/tmp/project").len(), DISPLAY_LIMIT);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn cache_keeps_three_hundred_records_and_config_keeps_thirty() {
        let cache_path = std::env::temp_dir().join(format!(
            "crossh-command-history-cache-{}.toml",
            std::process::id()
        ));
        let config_path = std::env::temp_dir().join(format!(
            "crossh-command-history-config-{}.toml",
            std::process::id()
        ));
        let mut history = CommandHistory::from_paths(cache_path.clone(), Some(config_path.clone()));
        for index in 0..MAX_HISTORY_ENTRIES + 20 {
            history.record("local:/tmp/project", &format!("command-{index}"));
        }

        let cache_records = read_scopes(&cache_path)
            .unwrap()
            .remove("local:/tmp/project")
            .unwrap();
        let config_records = read_scopes(&config_path)
            .unwrap()
            .remove("local:/tmp/project")
            .unwrap();
        assert_eq!(cache_records.len(), MAX_HISTORY_ENTRIES);
        assert_eq!(config_records.len(), DISPLAY_LIMIT);

        let _ = fs::remove_file(cache_path);
        let _ = fs::remove_file(config_path);
    }

    #[test]
    fn edit_preserves_frequency_and_remove_deletes_a_record() {
        let path = std::env::temp_dir().join(format!(
            "crossh-command-history-edit-{}.toml",
            std::process::id()
        ));
        let mut history = CommandHistory::from_path(path.clone());
        history.record("local:/tmp/project", "git status");
        history.record("local:/tmp/project", "git status");
        assert!(history.edit("local:/tmp/project", "git status", "git status --short"));
        assert_eq!(
            history.top("local:/tmp/project")[0].command,
            "git status --short"
        );
        assert_eq!(history.top("local:/tmp/project")[0].count, 2);
        assert!(history.remove("local:/tmp/project", "git status --short"));
        assert_eq!(history.total("local:/tmp/project"), 0);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn pinned_commands_are_scoped_and_sorted_first() {
        let path = std::env::temp_dir().join(format!(
            "crossh-command-history-pin-{}.toml",
            std::process::id()
        ));
        let mut history = CommandHistory::from_path(path.clone());
        history.record("local:/tmp/project", "git status");
        history.record("local:/tmp/project", "cargo test");
        history.record("local:/tmp/other", "cargo test");

        assert!(history.toggle_pinned("local:/tmp/project", "cargo test"));
        assert_eq!(history.top("local:/tmp/project")[0].command, "cargo test");
        assert_eq!(history.pinned("local:/tmp/project").len(), 1);
        assert!(history.pinned("local:/tmp/other").is_empty());
        assert!(history.toggle_pinned("local:/tmp/project", "cargo test"));
        assert!(history.pinned("local:/tmp/project").is_empty());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn ignored_commands_are_scoped_filtered_and_persisted() {
        let cache_path = std::env::temp_dir().join(format!(
            "crossh-command-history-ignore-cache-{}.toml",
            std::process::id()
        ));
        let config_path = std::env::temp_dir().join(format!(
            "crossh-command-history-ignore-config-{}.toml",
            std::process::id()
        ));
        let scope = "local:/tmp/project";
        let other_scope = "local:/tmp/other";
        let mut history = CommandHistory::from_paths(cache_path.clone(), Some(config_path.clone()));
        history.record(scope, "ls");
        history.record(scope, "git status");

        assert!(history.ignore(scope, "ls"));
        assert_eq!(history.total(scope), 1);
        assert_eq!(history.top(scope)[0].command, "git status");
        assert!(!history.record(scope, "ls"));
        history.record(scope, "git diff");
        assert!(history.edit(scope, "git diff", "ls"));
        assert_eq!(history.total(scope), 1);
        assert!(history.record(other_scope, "ls"));

        let mut restored =
            CommandHistory::from_paths(cache_path.clone(), Some(config_path.clone()));
        assert_eq!(restored.total(scope), 1);
        assert_eq!(restored.top(scope)[0].command, "git status");
        assert!(!restored.record(scope, "ls"));
        assert!(
            restored
                .top(other_scope)
                .iter()
                .any(|record| record.command == "ls")
        );
        assert!(
            read_history_file(&cache_path)
                .unwrap()
                .ignored_commands
                .is_empty()
        );
        let ignored = read_history_file(&config_path)
            .unwrap()
            .ignored_commands
            .remove(scope)
            .unwrap();
        assert_eq!(ignored, vec!["ls"]);

        let _ = fs::remove_file(cache_path);
        let _ = fs::remove_file(config_path);
    }

    #[test]
    fn scope_keys_keep_directory_identity() {
        assert_eq!(local_scope(Path::new("/tmp/project")), "local:/tmp/project");
    }

    #[test]
    fn remote_scope_keeps_host_and_directory_identity() {
        assert_eq!(
            remote_scope("deploy@example.com:22", "/srv/app"),
            "remote:deploy@example.com:22:/srv/app"
        );
        assert_ne!(
            remote_scope("deploy@example.com:22", "/srv/app"),
            remote_scope("deploy@example.com:22", "/srv/other")
        );
    }

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

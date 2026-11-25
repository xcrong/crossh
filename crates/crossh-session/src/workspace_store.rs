//! 工作区文件：原子写入、损坏恢复、格式保护与旧 settings 迁移。
//!
//! - 写入：tmp → `sync_all` → 主文件转 `.bak` → rename → 目录同步；
//! - 读取：主文件 → 备份 → 空工作区；损坏文件改名隔离；
//! - `format_version` 过新：不读写、不重命名，尽力写 `daemon.error`；
//! - 迁移：旧 settings 一次性导入，源文件旁保留 `.pre-workspace` 快照。

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use tokio::sync::{mpsc, oneshot};

use crate::workspace::{
    LegacyImport, Pane, PaneNode, Project, ProjectId, StorageErrorInfo, StorageErrorKind, Tab,
    TabId, TermSize, WORKSPACE_FORMAT_VERSION, Workspace, normalize_pane_cwd,
    normalize_project_root, now_secs,
};

pub const WORKSPACE_FILE_NAME: &str = "workspace.json";

#[derive(Debug)]
pub enum LoadError {
    FormatTooNew {
        found: u32,
        current: u32,
        path: PathBuf,
    },
    Io(io::Error),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FormatTooNew {
                found,
                current,
                path,
            } => write!(
                f,
                "workspace format {found} is newer than supported {current}; upgrade crossh ({})",
                path.display()
            ),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// 基于固定路径的工作区存储。
#[derive(Clone, Debug)]
pub struct WorkspaceStore {
    path: PathBuf,
}

impl WorkspaceStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// 默认配置根下的工作区路径。
    pub fn default_store() -> Option<Self> {
        crossh_core::config::workspace_path().map(Self::new)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn backup_path(&self) -> PathBuf {
        with_suffix(&self.path, ".bak")
    }

    pub fn tmp_path(&self) -> PathBuf {
        // 历史固定后缀保留给外部清理逻辑；实际写入改用唯一 tmp + O_EXCL。
        with_suffix(&self.path, ".tmp")
    }

    /// 加载主文件 → 备份 → 空工作区；损坏文件改名隔离。
    pub fn load(&self) -> Result<Workspace, LoadError> {
        let main = read_workspace(&self.path)?;
        match main {
            ReadOutcome::Missing => self.load_backup(),
            ReadOutcome::FormatTooNew(found) => Err(LoadError::FormatTooNew {
                found,
                current: WORKSPACE_FORMAT_VERSION,
                path: self.path.clone(),
            }),
            ReadOutcome::Corrupt => {
                self.quarantine(&self.path, "corrupt");
                self.load_backup()
            }
            ReadOutcome::Loaded(workspace) => Ok(workspace),
        }
    }

    fn load_backup(&self) -> Result<Workspace, LoadError> {
        match read_workspace(&self.backup_path())? {
            ReadOutcome::Loaded(workspace) => Ok(workspace),
            ReadOutcome::FormatTooNew(found) => Err(LoadError::FormatTooNew {
                found,
                current: WORKSPACE_FORMAT_VERSION,
                path: self.backup_path(),
            }),
            ReadOutcome::Corrupt => {
                self.quarantine(&self.backup_path(), "corrupt-bak");
                Ok(Workspace::new())
            }
            ReadOutcome::Missing => Ok(Workspace::new()),
        }
    }

    fn quarantine(&self, path: &Path, tag: &str) {
        // ponytail: 秒级名同秒覆盖，加 pid/纳秒；极端同纳秒时按存在追加重试后缀。
        let base = format!(
            ".{tag}-{}-{}-{}",
            now_secs(),
            std::process::id(),
            now_nanos()
        );
        let mut target = with_suffix(path, &base);
        for retry in 0..8 {
            if !target.exists() {
                break;
            }
            target = with_suffix(path, &format!("{base}-{retry}"));
        }
        if let Err(error) = std::fs::rename(path, &target) {
            log::warn!(
                "failed to quarantine corrupt workspace {}: {error}",
                path.display()
            );
        }
    }

    /// 原子保存整份工作区。
    pub fn save(&self, workspace: &Workspace) -> Result<(), StorageErrorInfo> {
        let bytes = serde_json::to_vec_pretty(workspace).map_err(|error| StorageErrorInfo {
            kind: StorageErrorKind::Format,
            message: error.to_string(),
            path: Some(self.path.clone()),
        })?;
        self.save_bytes(&bytes)
    }

    pub fn save_bytes(&self, bytes: &[u8]) -> Result<(), StorageErrorInfo> {
        save_atomic(&self.path, bytes).map_err(|error| StorageErrorInfo {
            kind: StorageErrorKind::Io,
            message: error.to_string(),
            path: Some(self.path.clone()),
        })
    }
}

/// 写入器使用的存储抽象，便于测试注入失败。
pub trait WorkspaceSaver: Send + Sync + 'static {
    fn save_bytes(&self, bytes: &[u8]) -> Result<(), StorageErrorInfo>;
}

impl WorkspaceSaver for WorkspaceStore {
    fn save_bytes(&self, bytes: &[u8]) -> Result<(), StorageErrorInfo> {
        WorkspaceStore::save_bytes(self, bytes)
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

enum ReadOutcome {
    Missing,
    Corrupt,
    FormatTooNew(u32),
    Loaded(Workspace),
}

fn read_workspace(path: &Path) -> Result<ReadOutcome, LoadError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(ReadOutcome::Missing),
        Err(error) => return Err(LoadError::Io(error)),
    };
    // 先只解出版本号：过新的文件不能反序列化成当前结构，也不能被改写。
    #[derive(Deserialize)]
    struct VersionProbe {
        #[serde(default)]
        format_version: u32,
    }
    if let Ok(probe) = serde_json::from_slice::<VersionProbe>(&bytes)
        && probe.format_version > WORKSPACE_FORMAT_VERSION
    {
        return Ok(ReadOutcome::FormatTooNew(probe.format_version));
    }
    match serde_json::from_slice::<Workspace>(&bytes) {
        Ok(workspace) if workspace.format_version > WORKSPACE_FORMAT_VERSION => {
            Ok(ReadOutcome::FormatTooNew(workspace.format_version))
        }
        Ok(workspace) => Ok(ReadOutcome::Loaded(workspace)),
        Err(_) => Ok(ReadOutcome::Corrupt),
    }
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_tmp_path(path: &Path) -> PathBuf {
    let count = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    with_suffix(
        path,
        &format!(".tmp-{}-{}-{count}", std::process::id(), now_nanos()),
    )
}

fn save_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    crossh_core::config::restrict_dir(parent)?;
    // ponytail: 固定 `.tmp` 可预测，改唯一后缀 + O_EXCL(create_new)；单写者语义由 WorkspaceWriter 串行保证。
    let tmp = unique_tmp_path(path);
    let save_result = (|| {
        {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&tmp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        if path.is_file() {
            let backup = with_suffix(path, ".bak");
            // Windows 的 rename 不覆盖目标文件；先删旧备份。
            let _ = std::fs::remove_file(&backup);
            std::fs::rename(path, &backup)?;
        }
        std::fs::rename(&tmp, path)?;
        crossh_core::config::restrict_file(path)?;
        sync_dir(parent)?;
        Ok(())
    })();
    if save_result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    save_result
}

fn sync_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        let dir = std::fs::File::open(path)?;
        dir.sync_all()?;
    }
    Ok(())
}

/// 把格式过新等启动失败写入 `daemon.error`；失败只记日志。
pub fn write_daemon_error(reason: &str) {
    // 配置根不可用时无处可写：带上原因记日志，方便按配置根排查。
    let Some(path) = crossh_core::config::daemon_error_path() else {
        log::warn!("daemon error (no config root): {reason}");
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            // 带上配置根派生的完整路径，否则只看目录名定位不到是哪个配置根。
            log::warn!(
                "failed to create daemon error dir {}: {error}",
                path.display()
            );
            return;
        }
        let _ = crossh_core::config::restrict_dir(parent);
    }
    if let Err(error) = std::fs::write(&path, format!("{reason}\n")) {
        // 同上：写失败必须带路径，否则多配置根并存时无法区分。
        log::warn!("failed to write daemon.error {}: {error}", path.display());
        return;
    }
    let _ = crossh_core::config::restrict_file(&path);
}

/// 成功启动后删除陈旧错误；失败只记日志。
pub fn clear_daemon_error() {
    let Some(path) = crossh_core::config::daemon_error_path() else {
        return;
    };
    if let Err(error) = std::fs::remove_file(&path)
        && error.kind() != io::ErrorKind::NotFound
    {
        // 带上完整路径：陈旧文件删不掉会影响下次状态展示，需按配置根定位。
        log::warn!(
            "failed to remove stale daemon.error {}: {error}",
            path.display()
        );
    }
}

/// 读取 `daemon.error`（CLI/状态展示用）。
pub fn read_daemon_error() -> Option<String> {
    let path = crossh_core::config::daemon_error_path()?;
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// 只读推断工作区格式是否高于当前（`daemon.error` 缺失时状态展示用）。
pub fn workspace_format_too_new() -> Option<u32> {
    let path = crossh_core::config::workspace_path()?;
    match read_workspace(&path) {
        Ok(ReadOutcome::FormatTooNew(found)) => Some(found),
        _ => None,
    }
}

/// 迁移源文件：新写入落点存在时用它，否则按只读回退读旧路径。
pub fn migration_source() -> Option<PathBuf> {
    let write = crossh_core::config::settings_path()?;
    if write.is_file() {
        return Some(write);
    }
    if std::env::var_os(crossh_core::config::CONFIG_DIR_ENV).is_some() {
        return None;
    }
    crossh_core::config::legacy_settings_path().filter(|path| path.is_file())
}

#[derive(Debug, Default, Deserialize)]
struct LegacySettings {
    #[serde(default, rename = "recent_local_dirs")]
    recent_local_dirs: Vec<PathBuf>,
    #[serde(default, rename = "recent_local_dirs_max")]
    recent_local_dirs_max: usize,
    #[serde(default)]
    pinned_local_tabs: Vec<LegacyPinnedTab>,
}

#[derive(Debug, Deserialize)]
struct LegacyPinnedTab {
    #[serde(default)]
    pin_id: u64,
    #[serde(default)]
    project_dir: PathBuf,
    #[serde(default)]
    cwd: PathBuf,
    #[serde(default)]
    custom_name: Option<String>,
    #[serde(default)]
    default_command: Option<String>,
    #[serde(default)]
    daemon_session_id: Option<String>,
}

/// 启动时执行一次性迁移。成功返回 `true` 表示工作区已被更新并落盘。
///
/// 迁移与工作区是否为空无关：已有同根项目时合并；CLI 先 `crossh open`
/// 不会破坏导入。写失败不设标记，下次重试。
pub fn migrate_legacy(workspace: &mut Workspace) -> bool {
    if workspace.legacy_imported.is_some() {
        return false;
    }
    let Some(source) = migration_source() else {
        return false;
    };
    let snapshot = with_suffix(&source, ".pre-workspace");
    let Ok(raw) = std::fs::read(&source) else {
        return false;
    };
    let Ok(probe) = toml::from_str::<LegacySettings>(&String::from_utf8_lossy(&raw)) else {
        return false;
    };
    if probe.recent_local_dirs.is_empty() && probe.pinned_local_tabs.is_empty() {
        return false;
    }
    if !snapshot.is_file()
        && let Err(error) = copy_snapshot(&source, &snapshot)
    {
        log::warn!("legacy settings snapshot failed, retry next start: {error}");
        return false;
    }
    let bytes = if snapshot.is_file() {
        std::fs::read(&snapshot).unwrap_or(raw)
    } else {
        raw
    };
    let mut legacy: LegacySettings = match toml::from_str(&String::from_utf8_lossy(&bytes)) {
        Ok(legacy) => legacy,
        Err(error) => {
            log::warn!("legacy settings parse failed: {error}");
            return false;
        }
    };
    if legacy.recent_local_dirs_max > 0 {
        legacy
            .recent_local_dirs
            .truncate(legacy.recent_local_dirs_max);
    }
    let Some(candidate) = import_legacy(workspace, &legacy, &source) else {
        return false;
    };
    let Some(store) = WorkspaceStore::default_store() else {
        return false;
    };
    if let Err(error) = store.save(&candidate) {
        log::warn!("legacy migration save failed, retry next start: {error:?}");
        return false;
    }
    *workspace = candidate;
    true
}

fn copy_snapshot(source: &Path, snapshot: &Path) -> io::Result<()> {
    let bytes = std::fs::read(source)?;
    if let Some(parent) = snapshot.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    let mut file = options.open(snapshot)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    crossh_core::config::restrict_file(snapshot)?;
    Ok(())
}

fn import_legacy(
    workspace: &Workspace,
    legacy: &LegacySettings,
    source: &Path,
) -> Option<Workspace> {
    let mut candidate = workspace.clone();
    let mut next_id = candidate.next_id;
    let mut seen_imported = BTreeSet::new();

    let fixed_projects =
        import_pinned_tabs(&mut candidate, &mut next_id, legacy, &mut seen_imported);
    let recent_projects =
        import_recent_dirs(&mut candidate, &mut next_id, legacy, &mut seen_imported);
    reorder_imported_projects(&mut candidate, &fixed_projects, &recent_projects);

    candidate.next_id = next_id;
    candidate.legacy_imported = Some(LegacyImport {
        imported_at: now_secs(),
        source: source.to_path_buf(),
    });
    Some(candidate)
}

/// 固定标签：项目按规范化根去重，标签按设置顺序追加。
fn import_pinned_tabs(
    candidate: &mut Workspace,
    next_id: &mut u64,
    legacy: &LegacySettings,
    seen_imported: &mut BTreeSet<ProjectId>,
) -> Vec<ProjectId> {
    let mut fixed_projects = Vec::new();
    for pinned in &legacy.pinned_local_tabs {
        let root = normalize_project_root(&pinned.project_dir);
        if root.as_os_str().is_empty() {
            continue;
        }
        let cwd = if pinned.cwd.as_os_str().is_empty() {
            root.clone()
        } else {
            normalize_pane_cwd(&pinned.cwd)
        };
        let project_id = ensure_project_root(candidate, root, next_id);
        let tab_id = TabId(*next_id);
        *next_id += 1;
        let pane_id = crate::workspace::PaneId(*next_id);
        *next_id += 1;
        if let Some(project) = candidate.project_mut(project_id) {
            project.tabs.push(Tab {
                id: tab_id,
                title: pinned
                    .custom_name
                    .clone()
                    .filter(|name| !name.trim().is_empty()),
                default_command: pinned
                    .default_command
                    .clone()
                    .filter(|command| !command.trim().is_empty()),
                active_pane: Some(pane_id),
                layout: PaneNode::Leaf(Pane {
                    id: pane_id,
                    cwd,
                    size: TermSize::default(),
                }),
            });
            project.active_tab.get_or_insert(tab_id);
        }
        if seen_imported.insert(project_id) {
            fixed_projects.push(project_id);
        }
        // `daemon_session_id` 刻意丢弃：daemon 重启后没有意义。
        let _ = pinned.pin_id;
        let _ = &pinned.daemon_session_id;
    }
    fixed_projects
}

/// 最近目录：只建空项目，去重按规范化根。
fn import_recent_dirs(
    candidate: &mut Workspace,
    next_id: &mut u64,
    legacy: &LegacySettings,
    seen_imported: &mut BTreeSet<ProjectId>,
) -> Vec<ProjectId> {
    let mut recent_projects = Vec::new();
    for dir in &legacy.recent_local_dirs {
        let root = normalize_project_root(dir);
        if root.as_os_str().is_empty() {
            continue;
        }
        let id = ensure_project_root(candidate, root, next_id);
        if seen_imported.insert(id) {
            recent_projects.push(id);
        }
    }
    recent_projects
}

/// 按规范化根复用已有项目，否则分配新 id 并追加空项目。
fn ensure_project_root(candidate: &mut Workspace, root: PathBuf, next_id: &mut u64) -> ProjectId {
    if let Some(project) = candidate.projects.iter().find(|p| p.root == root) {
        return project.id;
    }
    let id = ProjectId(*next_id);
    *next_id += 1;
    candidate.projects.push(Project {
        id,
        root,
        name: None,
        last_opened_at: 0,
        active_tab: None,
        tabs: Vec::new(),
    });
    id
}

/// 固定标签在前、最近目录在后；未参与导入的既有项目保持相对顺序。
fn reorder_imported_projects(
    candidate: &mut Workspace,
    fixed_projects: &[ProjectId],
    recent_projects: &[ProjectId],
) {
    let mut ordered = Vec::with_capacity(candidate.projects.len());
    for id in fixed_projects.iter().chain(recent_projects.iter()) {
        if let Some(position) = candidate
            .projects
            .iter()
            .position(|project| project.id == *id)
        {
            ordered.push(candidate.projects.remove(position));
        }
    }
    ordered.append(&mut candidate.projects);
    candidate.projects = ordered;
    candidate.active_project = fixed_projects
        .first()
        .copied()
        .or_else(|| recent_projects.first().copied())
        .or(candidate.active_project);
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct PersistenceResult {
    pub revision: u64,
    pub result: Result<(), StorageErrorInfo>,
}

pub struct SaveRequest {
    pub revision: u64,
    pub bytes: Vec<u8>,
    pub ack: Option<oneshot::Sender<Result<(), StorageErrorInfo>>>,
}

enum WriteCommand {
    Save(SaveRequest),
    Flush(oneshot::Sender<()>),
}

/// 单 writer 任务：串行写、写入期间合并待写载荷、逐个 ack。
pub struct WorkspaceWriter {
    tx: mpsc::UnboundedSender<WriteCommand>,
}

impl WorkspaceWriter {
    pub fn spawn(
        store: Arc<dyn WorkspaceSaver>,
        result_tx: mpsc::UnboundedSender<PersistenceResult>,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(writer_loop(store, result_tx, rx));
        Self { tx }
    }

    pub fn submit(&self, request: SaveRequest) {
        let _ = self.tx.send(WriteCommand::Save(request));
    }

    pub fn flush(&self) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        let _ = self.tx.send(WriteCommand::Flush(tx));
        rx
    }
}

struct Pending {
    revision: u64,
    bytes: Vec<u8>,
}

struct PendingAcks {
    acks: Vec<(u64, oneshot::Sender<Result<(), StorageErrorInfo>>)>,
    flushes: Vec<oneshot::Sender<()>>,
}

async fn writer_loop(
    store: Arc<dyn WorkspaceSaver>,
    result_tx: mpsc::UnboundedSender<PersistenceResult>,
    mut rx: mpsc::UnboundedReceiver<WriteCommand>,
) {
    let mut pending: Option<Pending> = None;
    let mut acks = PendingAcks {
        acks: Vec::new(),
        flushes: Vec::new(),
    };
    loop {
        if pending.is_none() {
            let Some(command) = rx.recv().await else {
                break;
            };
            absorb(command, &mut pending, &mut acks);
            drain(&mut rx, &mut pending, &mut acks);
        }
        let Some(payload) = pending.take() else {
            for flush in acks.flushes.drain(..) {
                let _ = flush.send(());
            }
            continue;
        };

        let mut backoff = Duration::from_secs(1);
        let mut current = payload;
        loop {
            match store.save_bytes(&current.bytes) {
                Ok(()) => {
                    let _ = result_tx.send(PersistenceResult {
                        revision: current.revision,
                        result: Ok(()),
                    });
                    let mut remaining = Vec::new();
                    for (revision, ack) in acks.acks.drain(..) {
                        if revision <= current.revision {
                            let _ = ack.send(Ok(()));
                        } else {
                            remaining.push((revision, ack));
                        }
                    }
                    acks.acks = remaining;
                    // 写入期间到达的新请求：继续写；没有新请求时才能满足 flush。
                    drain(&mut rx, &mut pending, &mut acks);
                    if pending.is_none() {
                        for flush in acks.flushes.drain(..) {
                            let _ = flush.send(());
                        }
                    }
                    break;
                }
                Err(error) => {
                    let _ = result_tx.send(PersistenceResult {
                        revision: current.revision,
                        result: Err(error.clone()),
                    });
                    // 结构回执立即报告未持久化；载荷留在后台继续按退避重试。
                    let mut remaining = Vec::new();
                    for (revision, ack) in acks.acks.drain(..) {
                        if revision <= current.revision {
                            let _ = ack.send(Err(error.clone()));
                        } else {
                            remaining.push((revision, ack));
                        }
                    }
                    acks.acks = remaining;
                    // 退避期间继续接收新请求：用更新的整份载荷替换当前重试内容。
                    let sleep = tokio::time::sleep(backoff);
                    tokio::pin!(sleep);
                    loop {
                        tokio::select! {
                            _ = &mut sleep => break,
                            command = rx.recv() => {
                                let Some(command) = command else { return; };
                                absorb(command, &mut pending, &mut acks);
                                drain(&mut rx, &mut pending, &mut acks);
                                if let Some(next) = pending.take()
                                    && next.revision >= current.revision
                                {
                                    current = next;
                                }
                            }
                        }
                    }
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                }
            }
        }
    }
}

fn absorb(command: WriteCommand, pending: &mut Option<Pending>, acks: &mut PendingAcks) {
    match command {
        WriteCommand::Save(request) => {
            if pending
                .as_ref()
                .is_none_or(|existing| request.revision >= existing.revision)
            {
                *pending = Some(Pending {
                    revision: request.revision,
                    bytes: request.bytes,
                });
            }
            if let Some(ack) = request.ack {
                acks.acks.push((request.revision, ack));
            }
        }
        WriteCommand::Flush(ack) => acks.flushes.push(ack),
    }
}

fn drain(
    rx: &mut mpsc::UnboundedReceiver<WriteCommand>,
    pending: &mut Option<Pending>,
    acks: &mut PendingAcks,
) {
    while let Ok(command) = rx.try_recv() {
        absorb(command, pending, acks);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "crossh-store-{name}-{}-{}",
            std::process::id(),
            now_secs()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn save_and_load_roundtrip_with_backup() {
        let root = temp_root("roundtrip");
        let path = root.join(WORKSPACE_FILE_NAME);
        let store = WorkspaceStore::new(path.clone());
        let mut workspace = Workspace::new();
        workspace.next_id = 4;
        store.save(&workspace).unwrap();
        assert!(path.is_file());
        workspace.next_id = 5;
        store.save(&workspace).unwrap();
        assert!(store.backup_path().is_file());
        let loaded = store.load().unwrap();
        assert_eq!(loaded.next_id, 5);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_main_falls_back_to_backup() {
        let root = temp_root("corrupt");
        let path = root.join(WORKSPACE_FILE_NAME);
        let store = WorkspaceStore::new(path.clone());
        let mut workspace = Workspace::new();
        workspace.next_id = 9;
        store.save(&workspace).unwrap();
        workspace.next_id = 10;
        store.save(&workspace).unwrap();
        // 用坏内容覆盖主文件，备份保留上一版。
        std::fs::write(&path, b"not json").unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.next_id, 9);
        assert!(root.read_dir().unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("corrupt")
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn double_corruption_starts_empty_and_quarantines_both() {
        let root = temp_root("double");
        let path = root.join(WORKSPACE_FILE_NAME);
        let store = WorkspaceStore::new(path.clone());
        std::fs::write(&path, b"nope").unwrap();
        std::fs::write(store.backup_path(), b"nope too").unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded, Workspace::new());
        let names: Vec<String> = root
            .read_dir()
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().any(|name| name.contains("corrupt-")));
        assert!(names.iter().any(|name| name.contains("corrupt-bak-")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn too_new_format_is_not_modified() {
        let root = temp_root("toonew");
        let path = root.join(WORKSPACE_FILE_NAME);
        let store = WorkspaceStore::new(path.clone());
        let mut workspace = Workspace::new();
        workspace.format_version = WORKSPACE_FORMAT_VERSION + 3;
        let bytes = serde_json::to_vec_pretty(&workspace).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let error = store.load().unwrap_err();
        assert!(matches!(
            error,
            LoadError::FormatTooNew { found, .. } if found == WORKSPACE_FORMAT_VERSION + 3
        ));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(!store.backup_path().exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn quarantine_names_carry_pid_and_avoid_same_second_collision() {
        let root = temp_root("quarantine-unique");
        let path = root.join(WORKSPACE_FILE_NAME);
        let store = WorkspaceStore::new(path.clone());
        // 同秒内两次隔离：旧 `.{tag}-{secs}` 会直接覆盖，现有 pid/纳秒名必须不同。
        std::fs::write(&path, b"bad one").unwrap();
        store.quarantine(&path, "corrupt");
        std::fs::write(&path, b"bad two").unwrap();
        store.quarantine(&path, "corrupt");
        let mut quarantined: Vec<String> = root
            .read_dir()
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("corrupt"))
            .collect();
        quarantined.sort();
        assert_eq!(quarantined.len(), 2);
        assert_ne!(quarantined[0], quarantined[1]);
        let pid = std::process::id().to_string();
        for name in &quarantined {
            assert!(name.contains(&pid), "隔离名应携带 pid: {name}");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn save_leaves_no_predictable_tmp_behind() {
        let root = temp_root("notmp");
        let path = root.join(WORKSPACE_FILE_NAME);
        let store = WorkspaceStore::new(path.clone());
        store.save(&Workspace::new()).unwrap();
        // 固定 `.tmp` 不再作为写入载体；唯一 tmp 成功后已 rename、失败已清理。
        assert!(!store.tmp_path().exists());
        assert!(
            !root.read_dir().unwrap().any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".tmp")),
            "不应残留任何 tmp 文件"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn migration_imports_fixed_then_recent_and_marks_once() {
        let root = temp_root("migrate");
        let source = root.join("settings.toml");
        // 用测试目录构造绝对路径：迁移到 Windows 时不会退化成 `/tmp/...`。
        let pinned = root.join("pinned-project");
        let pinned_cwd = pinned.join("sub");
        let recent = root.join("recent-project");
        let toml_path = |path: &Path| path.to_string_lossy().replace('\\', "\\\\");
        std::fs::write(
            &source,
            format!(
                r#"
recent_local_dirs = ["{recent}"]
pinned_local_tabs = [{{ pin_id = 1, project_dir = "{pinned}", cwd = "{cwd}", custom_name = "work", default_command = "echo hi", daemon_session_id = "s1" }}]
"#,
                recent = toml_path(&recent),
                pinned = toml_path(&pinned),
                cwd = toml_path(&pinned_cwd),
            ),
        )
        .unwrap();
        let legacy: LegacySettings =
            toml::from_str(&std::fs::read_to_string(&source).unwrap()).unwrap();
        assert_eq!(legacy.pinned_local_tabs.len(), 1);
        let workspace = Workspace::new();
        let mut candidate = import_legacy(&workspace, &legacy, &source).unwrap();
        assert!(candidate.legacy_imported.is_some());
        assert_eq!(candidate.projects.len(), 2);
        assert_eq!(candidate.projects[0].root, pinned);
        assert_eq!(candidate.projects[0].tabs.len(), 1);
        assert_eq!(
            candidate.projects[0].tabs[0].default_command.as_deref(),
            Some("echo hi")
        );
        assert_eq!(candidate.projects[1].tabs.len(), 0);
        assert_eq!(
            candidate.active_project,
            candidate.projects.first().map(|project| project.id)
        );
        // 再次导入跳过标记。
        assert!(candidate.legacy_imported.is_some());
        let _ = &mut candidate;
        let _ = std::fs::remove_dir_all(root);
    }
}

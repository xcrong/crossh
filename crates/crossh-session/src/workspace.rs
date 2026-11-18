//! 工作区持久模型：项目、tab、递归分屏树与纯函数操作。
//!
//! 本模块零 `gpui` 依赖。持久身份（`ProjectId`/`TabId`/`PaneId`/`SplitId`）
//! 与运行时会话身份（`SessionId`）严格分离：会话只存在于 [`Runtime`]，不落盘。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub type SessionId = String;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl std::str::FromStr for $name {
            type Err = std::num::ParseIntError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                s.parse::<u64>().map(Self)
            }
        }
    };
}

id_type!(ProjectId);
id_type!(TabId);
id_type!(PaneId);
id_type!(SplitId);

/// 工作区文件格式版本；高于此值的文件由 daemon 拒绝而不是覆盖。
pub const WORKSPACE_FORMAT_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    Row,
    Column,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermSize {
    pub cols: u16,
    pub rows: u16,
}

impl Default for TermSize {
    fn default() -> Self {
        Self { cols: 80, rows: 24 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub format_version: u32,
    pub revision: u64,
    pub next_id: u64,
    pub active_project: Option<ProjectId>,
    pub projects: Vec<Project>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_imported: Option<LegacyImport>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegacyImport {
    pub imported_at: u64,
    pub source: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub last_opened_at: u64,
    pub active_tab: Option<TabId>,
    pub tabs: Vec<Tab>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    pub id: TabId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_command: Option<String>,
    pub active_pane: Option<PaneId>,
    pub layout: PaneNode,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PaneNode {
    Leaf(Pane),
    Split {
        id: SplitId,
        axis: Axis,
        ratio: f32,
        first: Box<PaneNode>,
        second: Box<PaneNode>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pane {
    pub id: PaneId,
    pub cwd: PathBuf,
    #[serde(default)]
    pub size: TermSize,
}

/// 运行时状态；不进入持久模型。
#[derive(Clone, Debug, Default)]
pub struct Runtime {
    pub pane_session: BTreeMap<PaneId, SessionId>,
    pub session_pane: BTreeMap<SessionId, PaneId>,
    pub starting: BTreeSet<PaneId>,
    /// 上次物化失败、尚未重试的 pane。
    pub spawn_failed: BTreeSet<PaneId>,
    pub attached_clients: usize,
    pub storage_error: Option<StorageErrorInfo>,
}

/// 快照与协议共用的存储错误。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageErrorInfo {
    pub kind: StorageErrorKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageErrorKind {
    Io,
    Format,
    Corpus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneStatusReason {
    NotStarted,
    SpawnFailed,
    ProjectRootMissing,
    CwdMissing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PaneStatus {
    Starting,
    Live,
    Dormant { reason: PaneStatusReason },
    Unavailable { reason: PaneStatusReason },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    pub revision: u64,
    pub active_project: Option<ProjectId>,
    pub projects: Vec<ProjectSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_error: Option<StorageErrorInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectSnapshot {
    pub id: ProjectId,
    pub root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub available: bool,
    pub active_tab: Option<TabId>,
    pub tabs: Vec<TabSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabSnapshot {
    pub id: TabId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_command: Option<String>,
    pub active_pane: Option<PaneId>,
    pub layout: PaneNodeSnapshot,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PaneNodeSnapshot {
    Leaf(PaneSnapshot),
    Split {
        id: SplitId,
        axis: Axis,
        ratio: f32,
        first: Box<PaneNodeSnapshot>,
        second: Box<PaneNodeSnapshot>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaneSnapshot {
    pub id: PaneId,
    pub cwd: PathBuf,
    pub size: TermSize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
    pub cwd_available: bool,
    pub status: PaneStatus,
}

impl PaneNodeSnapshot {
    pub fn leaves(&self) -> impl Iterator<Item = &PaneSnapshot> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out.into_iter()
    }

    fn collect_leaves<'a>(&'a self, out: &mut Vec<&'a PaneSnapshot>) {
        match self {
            PaneNodeSnapshot::Leaf(pane) => out.push(pane),
            PaneNodeSnapshot::Split { first, second, .. } => {
                first.collect_leaves(out);
                second.collect_leaves(out);
            }
        }
    }

    pub fn find_pane(&self, pane_id: PaneId) -> Option<&PaneSnapshot> {
        match self {
            PaneNodeSnapshot::Leaf(pane) => (pane.id == pane_id).then_some(pane),
            PaneNodeSnapshot::Split { first, second, .. } => first
                .find_pane(pane_id)
                .or_else(|| second.find_pane(pane_id)),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutateOutcome {
    Applied,
    #[default]
    Unchanged,
    Conflict,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpError {
    pub code: String,
    pub message: String,
}

impl OpError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }

    pub fn not_found(target: &str) -> Self {
        Self::new("not_found", format!("{target} not found"))
    }

    pub fn invalid_payload(message: impl Into<String>) -> Self {
        Self::new("invalid_payload", message)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedIds {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<TabId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<PaneId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<SplitId>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApplyResult {
    pub outcome: MutateOutcome,
    pub created: CreatedIds,
    pub error: Option<OpError>,
}

impl ApplyResult {
    fn applied(created: CreatedIds) -> Self {
        Self {
            outcome: MutateOutcome::Applied,
            created,
            error: None,
        }
    }

    fn unchanged() -> Self {
        Self {
            outcome: MutateOutcome::Unchanged,
            ..Self::default()
        }
    }

    fn rejected(error: OpError) -> Self {
        Self {
            outcome: MutateOutcome::Rejected,
            error: Some(error),
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum WorkspaceOp {
    OpenProject {
        root: PathBuf,
    },
    StopProject {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project_id: Option<ProjectId>,
    },
    ForgetProject {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project_id: Option<ProjectId>,
    },
    ActivateProject {
        project_id: ProjectId,
    },
    CreateTab {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project_id: Option<ProjectId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<PathBuf>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default_command: Option<String>,
    },
    CloseTab {
        tab_id: TabId,
    },
    ActivateTab {
        tab_id: TabId,
    },
    ReorderTabs {
        project_id: ProjectId,
        tab_id: TabId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_tab_id: Option<TabId>,
    },
    RenameTab {
        tab_id: TabId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    SetTabCommand {
        tab_id: TabId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
    },
    SplitPane {
        pane_id: PaneId,
        axis: Axis,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<PathBuf>,
    },
    ClosePane {
        pane_id: PaneId,
    },
    ActivatePane {
        pane_id: PaneId,
    },
    SetSplitRatio {
        split_id: SplitId,
        ratio: f32,
    },
    SetPaneCwd {
        pane_id: PaneId,
        cwd: PathBuf,
    },
    EnsurePaneSession {
        pane_id: PaneId,
    },
    RefreshAvailability,
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            format_version: WORKSPACE_FORMAT_VERSION,
            revision: 0,
            next_id: 1,
            active_project: None,
            projects: Vec::new(),
            legacy_imported: None,
        }
    }

    fn allocate<T>(&mut self, make: impl FnOnce(u64) -> T) -> T {
        let id = self.next_id;
        self.next_id += 1;
        make(id)
    }

    pub fn project(&self, id: ProjectId) -> Option<&Project> {
        self.projects.iter().find(|project| project.id == id)
    }

    pub fn project_mut(&mut self, id: ProjectId) -> Option<&mut Project> {
        self.projects.iter_mut().find(|project| project.id == id)
    }

    pub fn active_project_ref(&self) -> Option<&Project> {
        self.active_project.and_then(|id| self.project(id))
    }

    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.projects
            .iter()
            .flat_map(|project| &project.tabs)
            .find(|tab| tab.id == id)
    }

    pub fn tab_project(&self, id: TabId) -> Option<ProjectId> {
        self.projects
            .iter()
            .find(|project| project.tabs.iter().any(|tab| tab.id == id))
            .map(|project| project.id)
    }

    /// 查找 pane 及其所在 tab/project。
    pub fn locate_pane(&self, pane_id: PaneId) -> Option<(ProjectId, TabId, &Pane)> {
        for project in &self.projects {
            for tab in &project.tabs {
                if let Some(pane) = tab.layout.find_pane(pane_id) {
                    return Some((project.id, tab.id, pane));
                }
            }
        }
        None
    }

    pub fn pane(&self, pane_id: PaneId) -> Option<&Pane> {
        self.locate_pane(pane_id).map(|(_, _, pane)| pane)
    }

    pub fn pane_mut(&mut self, pane_id: PaneId) -> Option<&mut Pane> {
        for project in &mut self.projects {
            for tab in &mut project.tabs {
                if let Some(pane) = tab.layout.find_pane_mut(pane_id) {
                    return Some(pane);
                }
            }
        }
        None
    }

    pub fn project_of_pane(&self, pane_id: PaneId) -> Option<ProjectId> {
        self.locate_pane(pane_id).map(|(project, _, _)| project)
    }

    pub fn all_panes(&self) -> Vec<&Pane> {
        self.projects
            .iter()
            .flat_map(|project| &project.tabs)
            .flat_map(|tab| tab.layout.leaves())
            .collect()
    }

    pub fn pane_count(&self) -> usize {
        self.all_panes().len()
    }

    pub fn tab_count(&self) -> usize {
        self.projects.iter().map(|project| project.tabs.len()).sum()
    }

    /// 某项目的全部分屏 pane（用于 StopProject 等收集会话）。
    pub fn project_panes(&self, project_id: ProjectId) -> Vec<PaneId> {
        self.project(project_id)
            .map(|project| {
                project
                    .tabs
                    .iter()
                    .flat_map(|tab| tab.layout.leaves().map(|pane| pane.id))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn tab_panes(&self, tab_id: TabId) -> Vec<PaneId> {
        self.tab(tab_id)
            .map(|tab| tab.layout.leaves().map(|pane| pane.id).collect())
            .unwrap_or_default()
    }

    /// 持久模型 → 快照。`available`、`cwd_available`、`session` 与
    /// `PaneStatus` 都在这里计算，不落盘。
    pub fn snapshot(&self, runtime: &Runtime) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            revision: self.revision,
            active_project: self.active_project,
            projects: self
                .projects
                .iter()
                .map(|project| {
                    let available = project.root.is_dir();
                    ProjectSnapshot {
                        id: project.id,
                        root: project.root.clone(),
                        name: project.name.clone(),
                        available,
                        active_tab: project.active_tab,
                        tabs: project
                            .tabs
                            .iter()
                            .map(|tab| TabSnapshot {
                                id: tab.id,
                                title: tab.title.clone(),
                                default_command: tab.default_command.clone(),
                                active_pane: tab.active_pane,
                                layout: tab.layout.snapshot(available, runtime),
                            })
                            .collect(),
                    }
                })
                .collect(),
            storage_error: runtime.storage_error.clone(),
        }
    }

    /// 布局操作入口。运行时操作（`EnsurePaneSession`/`RefreshAvailability`）
    /// 由 actor 先处理；这里遇到时保持 `Unchanged`。
    pub fn apply(&mut self, op: &WorkspaceOp) -> ApplyResult {
        match op {
            WorkspaceOp::OpenProject { root } => self.open_project(root),
            WorkspaceOp::StopProject { project_id } => self.stop_project(*project_id),
            WorkspaceOp::ForgetProject { project_id } => self.forget_project(*project_id),
            WorkspaceOp::ActivateProject { project_id } => self.activate_project(*project_id),
            WorkspaceOp::CreateTab {
                project_id,
                title,
                cwd,
                default_command,
            } => self.create_tab(
                *project_id,
                title.clone(),
                cwd.clone(),
                default_command.clone(),
            ),
            WorkspaceOp::CloseTab { tab_id } => self.close_tab(*tab_id),
            WorkspaceOp::ActivateTab { tab_id } => self.activate_tab(*tab_id),
            WorkspaceOp::ReorderTabs {
                project_id,
                tab_id,
                before_tab_id,
            } => self.reorder_tabs(*project_id, *tab_id, *before_tab_id),
            WorkspaceOp::RenameTab { tab_id, title } => self.rename_tab(*tab_id, title.clone()),
            WorkspaceOp::SetTabCommand { tab_id, command } => {
                self.set_tab_command(*tab_id, command.clone())
            }
            WorkspaceOp::SplitPane { pane_id, axis, cwd } => {
                self.split_pane(*pane_id, *axis, cwd.clone())
            }
            WorkspaceOp::ClosePane { pane_id } => self.close_pane(*pane_id),
            WorkspaceOp::ActivatePane { pane_id } => self.activate_pane(*pane_id),
            WorkspaceOp::SetSplitRatio { split_id, ratio } => {
                self.set_split_ratio(*split_id, *ratio)
            }
            WorkspaceOp::SetPaneCwd { pane_id, cwd } => self.set_pane_cwd(*pane_id, cwd),
            WorkspaceOp::EnsurePaneSession { .. } | WorkspaceOp::RefreshAvailability => {
                ApplyResult::unchanged()
            }
        }
    }

    fn bump(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }

    fn open_project(&mut self, root: &Path) -> ApplyResult {
        let root = normalize_project_root(root);
        let existing = self
            .projects
            .iter()
            .position(|project| project.root == root);
        if let Some(index) = existing {
            let mut changed = false;
            if index != 0 {
                let mut project = self.projects.remove(index);
                project.last_opened_at = now_secs();
                self.projects.insert(0, project);
                changed = true;
            }
            let project_id = self.projects[0].id;
            let mut created = CreatedIds::default();
            if self.active_project != Some(project_id) {
                self.active_project = Some(project_id);
                changed = true;
            }
            if self.projects[0].tabs.is_empty() {
                let (tab_id, pane_id) = self.new_tab_for(0, None, None);
                created.tab = Some(tab_id);
                created.pane = Some(pane_id);
                changed = true;
            } else if self.projects[0].active_tab.is_none() {
                self.projects[0].active_tab = Some(self.projects[0].tabs[0].id);
                changed = true;
            }
            if changed {
                self.bump();
                return ApplyResult::applied(created);
            }
            return ApplyResult::unchanged();
        }

        let id = self.allocate(ProjectId);
        let mut project = Project {
            id,
            root: root.clone(),
            name: None,
            last_opened_at: now_secs(),
            active_tab: None,
            tabs: Vec::new(),
        };
        // 新项目总在最前；先建 tab 再插入，保持顺序一次到位。
        let tab_id = self.allocate(TabId);
        let pane_id = self.allocate(PaneId);
        project.tabs.push(Tab {
            id: tab_id,
            title: None,
            default_command: None,
            active_pane: Some(pane_id),
            layout: PaneNode::Leaf(Pane {
                id: pane_id,
                cwd: root,
                size: TermSize::default(),
            }),
        });
        project.active_tab = Some(tab_id);
        self.projects.insert(0, project);
        self.active_project = Some(id);
        self.bump();
        ApplyResult::applied(CreatedIds {
            project: Some(id),
            tab: Some(tab_id),
            pane: Some(pane_id),
            split: None,
        })
    }

    /// 在 projects[index] 内建 tab + pane；返回 `(tab_id, pane_id)`。
    fn new_tab_for(
        &mut self,
        index: usize,
        cwd: Option<PathBuf>,
        default_command: Option<String>,
    ) -> (TabId, PaneId) {
        let project = &self.projects[index];
        let cwd = cwd.unwrap_or_else(|| {
            project
                .tabs
                .iter()
                .find(|tab| Some(tab.id) == project.active_tab)
                .and_then(|tab| tab.active_pane)
                .and_then(|pane| tab_pane_cwd(&project.tabs, pane))
                .unwrap_or_else(|| project.root.clone())
        });
        let tab_id = self.allocate(TabId);
        let pane_id = self.allocate(PaneId);
        let tab = Tab {
            id: tab_id,
            title: None,
            default_command,
            active_pane: Some(pane_id),
            layout: PaneNode::Leaf(Pane {
                id: pane_id,
                cwd,
                size: TermSize::default(),
            }),
        };
        let project = &mut self.projects[index];
        project.tabs.push(tab);
        project.active_tab = Some(tab_id);
        (tab_id, pane_id)
    }

    fn resolve_project(&self, project_id: Option<ProjectId>) -> Result<ProjectId, OpError> {
        match project_id.or(self.active_project) {
            Some(id) if self.project(id).is_some() => Ok(id),
            Some(_) => Err(OpError::not_found("project")),
            None => Err(OpError::new(
                "no_active_project",
                "no active project and no project_id given",
            )),
        }
    }

    fn stop_project(&mut self, project_id: Option<ProjectId>) -> ApplyResult {
        let project_id = match self.resolve_project(project_id) {
            Ok(id) => id,
            Err(error) => return ApplyResult::rejected(error),
        };
        let index = self
            .projects
            .iter()
            .position(|project| project.id == project_id)
            .expect("project checked above");
        if self.projects[index].tabs.is_empty() {
            return ApplyResult::unchanged();
        }
        self.projects[index].tabs.clear();
        self.projects[index].active_tab = None;
        self.bump();
        ApplyResult::applied(CreatedIds::default())
    }

    fn forget_project(&mut self, project_id: Option<ProjectId>) -> ApplyResult {
        let Some(project_id) = project_id.or(self.active_project) else {
            return ApplyResult::rejected(OpError::new(
                "no_active_project",
                "no active project and no project_id given",
            ));
        };
        let Some(index) = self
            .projects
            .iter()
            .position(|project| project.id == project_id)
        else {
            return ApplyResult::rejected(OpError::not_found("project"));
        };
        self.projects.remove(index);
        if self.active_project == Some(project_id) {
            self.active_project = self.projects.first().map(|project| project.id);
        }
        self.bump();
        ApplyResult::applied(CreatedIds::default())
    }

    fn activate_project(&mut self, project_id: ProjectId) -> ApplyResult {
        let Some(index) = self
            .projects
            .iter()
            .position(|project| project.id == project_id)
        else {
            return ApplyResult::rejected(OpError::not_found("project"));
        };
        let active_tab = self.projects[index]
            .active_tab
            .or_else(|| self.projects[index].tabs.first().map(|tab| tab.id));
        let changed = self.active_project != Some(project_id)
            || self.projects[index].active_tab != active_tab;
        if !changed {
            return ApplyResult::unchanged();
        }
        self.active_project = Some(project_id);
        self.projects[index].active_tab = active_tab;
        self.bump();
        ApplyResult::applied(CreatedIds::default())
    }

    fn create_tab(
        &mut self,
        project_id: Option<ProjectId>,
        title: Option<String>,
        cwd: Option<PathBuf>,
        default_command: Option<String>,
    ) -> ApplyResult {
        let project_id = match self.resolve_project(project_id) {
            Ok(id) => id,
            Err(error) => return ApplyResult::rejected(error),
        };
        let Some(index) = self
            .projects
            .iter()
            .position(|project| project.id == project_id)
        else {
            return ApplyResult::rejected(OpError::not_found("project"));
        };
        let (tab_id, pane_id) = self.new_tab_for(
            index,
            cwd.map(|cwd| normalize_pane_cwd(&cwd)),
            default_command,
        );
        if let Some(title) = title.filter(|value| !value.trim().is_empty()) {
            self.projects[index]
                .tabs
                .last_mut()
                .expect("tab just created")
                .title = Some(title.trim().to_string());
        }
        self.active_project = Some(project_id);
        self.bump();
        ApplyResult::applied(CreatedIds {
            project: Some(project_id),
            tab: Some(tab_id),
            pane: Some(pane_id),
            split: None,
        })
    }

    fn close_tab(&mut self, tab_id: TabId) -> ApplyResult {
        let Some(project_index) = self
            .projects
            .iter()
            .position(|project| project.tabs.iter().any(|tab| tab.id == tab_id))
        else {
            return ApplyResult::rejected(OpError::not_found("tab"));
        };
        let project = &mut self.projects[project_index];
        let Some(index) = project.tabs.iter().position(|tab| tab.id == tab_id) else {
            return ApplyResult::rejected(OpError::not_found("tab"));
        };
        project.tabs.remove(index);
        project.active_tab = project
            .tabs
            .get(index)
            .or_else(|| index.checked_sub(1).and_then(|prev| project.tabs.get(prev)))
            .map(|tab| tab.id);
        self.bump();
        ApplyResult::applied(CreatedIds::default())
    }

    fn activate_tab(&mut self, tab_id: TabId) -> ApplyResult {
        let Some(project_index) = self
            .projects
            .iter()
            .position(|project| project.tabs.iter().any(|tab| tab.id == tab_id))
        else {
            return ApplyResult::rejected(OpError::not_found("tab"));
        };
        let project = &mut self.projects[project_index];
        let Some(tab) = project.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return ApplyResult::rejected(OpError::not_found("tab"));
        };
        let active_pane = tab
            .active_pane
            .or_else(|| tab.layout.leaves().next().map(|pane| pane.id));
        let changed = self.active_project != Some(project.id)
            || project.active_tab != Some(tab_id)
            || tab.active_pane != active_pane;
        if !changed {
            return ApplyResult::unchanged();
        }
        tab.active_pane = active_pane;
        project.active_tab = Some(tab_id);
        self.active_project = Some(project.id);
        self.bump();
        ApplyResult::applied(CreatedIds::default())
    }

    fn reorder_tabs(
        &mut self,
        project_id: ProjectId,
        tab_id: TabId,
        before_tab_id: Option<TabId>,
    ) -> ApplyResult {
        let Some(project) = self.project_mut(project_id) else {
            return ApplyResult::rejected(OpError::not_found("project"));
        };
        let Some(from) = project.tabs.iter().position(|tab| tab.id == tab_id) else {
            return ApplyResult::rejected(OpError::not_found("tab"));
        };
        let to = match before_tab_id {
            Some(before) => match project.tabs.iter().position(|tab| tab.id == before) {
                Some(index) => index,
                None => return ApplyResult::rejected(OpError::not_found("tab")),
            },
            None => project.tabs.len(),
        };
        // 目标即自身或自身后一位：顺序不变。
        if from == to || from + 1 == to {
            return ApplyResult::unchanged();
        }
        let tab = project.tabs.remove(from);
        let to = if from < to { to - 1 } else { to };
        project.tabs.insert(to, tab);
        self.bump();
        ApplyResult::applied(CreatedIds::default())
    }

    fn rename_tab(&mut self, tab_id: TabId, title: Option<String>) -> ApplyResult {
        let normalized = title
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty());
        let Some(tab) = self.tab_mut(tab_id) else {
            return ApplyResult::rejected(OpError::not_found("tab"));
        };
        if tab.title == normalized {
            return ApplyResult::unchanged();
        }
        tab.title = normalized;
        self.bump();
        ApplyResult::applied(CreatedIds::default())
    }

    fn set_tab_command(&mut self, tab_id: TabId, command: Option<String>) -> ApplyResult {
        let normalized = command
            .map(|command| command.trim().to_string())
            .filter(|command| !command.is_empty());
        let Some(tab) = self.tab_mut(tab_id) else {
            return ApplyResult::rejected(OpError::not_found("tab"));
        };
        if tab.default_command == normalized {
            return ApplyResult::unchanged();
        }
        tab.default_command = normalized;
        self.bump();
        ApplyResult::applied(CreatedIds::default())
    }

    fn tab_mut(&mut self, tab_id: TabId) -> Option<&mut Tab> {
        self.projects
            .iter_mut()
            .flat_map(|project| &mut project.tabs)
            .find(|tab| tab.id == tab_id)
    }

    fn split_pane(&mut self, pane_id: PaneId, axis: Axis, cwd: Option<PathBuf>) -> ApplyResult {
        let Some((project_id, tab_id, target)) = self.locate_pane(pane_id) else {
            return ApplyResult::rejected(OpError::not_found("pane"));
        };
        let cwd = cwd
            .map(|cwd| normalize_pane_cwd(&cwd))
            .unwrap_or_else(|| target.cwd.clone());
        let size = target.size;
        let split_id = self.allocate(SplitId);
        let new_pane_id = self.allocate(PaneId);
        let Some(tab) = self.tab_mut(tab_id) else {
            return ApplyResult::rejected(OpError::not_found("tab"));
        };
        let layout = std::mem::replace(
            &mut tab.layout,
            PaneNode::Leaf(Pane {
                id: new_pane_id,
                cwd: cwd.clone(),
                size,
            }),
        );
        tab.layout = layout.replace_leaf(pane_id, |leaf| PaneNode::Split {
            id: split_id,
            axis,
            ratio: 0.5,
            first: Box::new(PaneNode::Leaf(leaf)),
            second: Box::new(PaneNode::Leaf(Pane {
                id: new_pane_id,
                cwd: cwd.clone(),
                size,
            })),
        });
        tab.active_pane = Some(new_pane_id);
        if let Some(project) = self.project_mut(project_id) {
            project.active_tab = Some(tab_id);
        }
        self.active_project = Some(project_id);
        self.bump();
        ApplyResult::applied(CreatedIds {
            project: Some(project_id),
            tab: Some(tab_id),
            pane: Some(new_pane_id),
            split: Some(split_id),
        })
    }

    fn close_pane(&mut self, pane_id: PaneId) -> ApplyResult {
        let Some((project_id, tab_id, _)) = self.locate_pane(pane_id) else {
            return ApplyResult::rejected(OpError::not_found("pane"));
        };
        let Some(tab) = self.tab_mut(tab_id) else {
            return ApplyResult::rejected(OpError::not_found("tab"));
        };
        let mut focus = None;
        let layout = tab.layout.clone();
        let Some(new_root) = layout.remove_leaf(pane_id, &mut focus) else {
            return ApplyResult::rejected(OpError::not_found("pane"));
        };
        let Some(new_root) = new_root else {
            // 根叶子：删除整个 tab。
            return self.close_tab(tab_id);
        };
        let Some(tab) = self.tab_mut(tab_id) else {
            return ApplyResult::rejected(OpError::not_found("tab"));
        };
        tab.layout = new_root;
        tab.active_pane = focus.or_else(|| tab.layout.leaves().next().map(|pane| pane.id));
        if let Some(project) = self.project_mut(project_id) {
            project.active_tab = Some(tab_id);
        }
        self.active_project = Some(project_id);
        self.bump();
        ApplyResult::applied(CreatedIds::default())
    }

    fn activate_pane(&mut self, pane_id: PaneId) -> ApplyResult {
        let Some((project_id, tab_id, _)) = self.locate_pane(pane_id) else {
            return ApplyResult::rejected(OpError::not_found("pane"));
        };
        let (project_tab, tab_active) = match self.project(project_id).and_then(|project| {
            project
                .tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .map(|tab| (project.active_tab, tab.active_pane))
        }) {
            Some(state) => state,
            None => return ApplyResult::rejected(OpError::not_found("tab")),
        };
        let changed = self.active_project != Some(project_id)
            || project_tab != Some(tab_id)
            || tab_active != Some(pane_id);
        if !changed {
            return ApplyResult::unchanged();
        }
        let Some(project) = self.project_mut(project_id) else {
            return ApplyResult::rejected(OpError::not_found("project"));
        };
        if let Some(tab) = project.tabs.iter_mut().find(|tab| tab.id == tab_id) {
            tab.active_pane = Some(pane_id);
        }
        project.active_tab = Some(tab_id);
        self.active_project = Some(project_id);
        self.bump();
        ApplyResult::applied(CreatedIds::default())
    }

    fn set_split_ratio(&mut self, split_id: SplitId, ratio: f32) -> ApplyResult {
        let ratio = ratio.clamp(0.05, 0.95);
        let current = self
            .projects
            .iter()
            .flat_map(|project| &project.tabs)
            .find_map(|tab| tab.layout.split_ratio(split_id));
        let Some(current) = current else {
            return ApplyResult::rejected(OpError::not_found("split"));
        };
        if (current - ratio).abs() < f32::EPSILON {
            return ApplyResult::unchanged();
        }
        for project in &mut self.projects {
            for tab in &mut project.tabs {
                if tab.layout.set_split_ratio(split_id, ratio) {
                    self.bump();
                    return ApplyResult::applied(CreatedIds::default());
                }
            }
        }
        ApplyResult::rejected(OpError::not_found("split"))
    }

    pub fn set_pane_cwd_checked(
        &mut self,
        pane_id: PaneId,
        cwd: &Path,
        status: &PaneStatus,
    ) -> ApplyResult {
        match status {
            PaneStatus::Starting => {
                return ApplyResult::rejected(OpError::new(
                    "pane_starting",
                    "pane session is starting",
                ));
            }
            PaneStatus::Live => {
                return ApplyResult::rejected(OpError::new("pane_live", "pane session is live"));
            }
            PaneStatus::Dormant { .. } | PaneStatus::Unavailable { .. } => {}
        }
        self.set_pane_cwd(pane_id, cwd)
    }

    fn set_pane_cwd(&mut self, pane_id: PaneId, cwd: &Path) -> ApplyResult {
        let Some(pane) = self.pane_mut(pane_id) else {
            return ApplyResult::rejected(OpError::not_found("pane"));
        };
        let cwd = normalize_pane_cwd(cwd);
        if pane.cwd == cwd {
            return ApplyResult::unchanged();
        }
        pane.cwd = cwd;
        self.bump();
        ApplyResult::applied(CreatedIds::default())
    }
}

fn tab_pane_cwd(tabs: &[Tab], pane_id: PaneId) -> Option<PathBuf> {
    tabs.iter()
        .flat_map(|tab| tab.layout.leaves())
        .find(|pane| pane.id == pane_id)
        .map(|pane| pane.cwd.clone())
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

/// 分屏查询与修改。
impl PaneNode {
    pub fn split_ratio(&self, split_id: SplitId) -> Option<f32> {
        match self {
            PaneNode::Leaf(_) => None,
            PaneNode::Split {
                id,
                ratio,
                first,
                second,
                ..
            } => {
                if *id == split_id {
                    return Some(*ratio);
                }
                first
                    .split_ratio(split_id)
                    .or_else(|| second.split_ratio(split_id))
            }
        }
    }

    pub fn set_split_ratio(&mut self, split_id: SplitId, ratio: f32) -> bool {
        match self {
            PaneNode::Leaf(_) => false,
            PaneNode::Split {
                id,
                ratio: current,
                first,
                second,
                ..
            } => {
                if *id == split_id {
                    *current = ratio;
                    return true;
                }
                first.set_split_ratio(split_id, ratio) || second.set_split_ratio(split_id, ratio)
            }
        }
    }
}

impl PaneNode {
    pub fn leaves(&self) -> impl Iterator<Item = &Pane> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out.into_iter()
    }

    fn collect_leaves<'a>(&'a self, out: &mut Vec<&'a Pane>) {
        match self {
            PaneNode::Leaf(pane) => out.push(pane),
            PaneNode::Split { first, second, .. } => {
                first.collect_leaves(out);
                second.collect_leaves(out);
            }
        }
    }

    pub fn find_pane(&self, pane_id: PaneId) -> Option<&Pane> {
        match self {
            PaneNode::Leaf(pane) => (pane.id == pane_id).then_some(pane),
            PaneNode::Split { first, second, .. } => first
                .find_pane(pane_id)
                .or_else(|| second.find_pane(pane_id)),
        }
    }

    pub fn find_pane_mut(&mut self, pane_id: PaneId) -> Option<&mut Pane> {
        match self {
            PaneNode::Leaf(pane) => (pane.id == pane_id).then_some(pane),
            PaneNode::Split { first, second, .. } => first
                .find_pane_mut(pane_id)
                .or_else(|| second.find_pane_mut(pane_id)),
        }
    }

    /// 用 `make` 替换第一个匹配的叶子。
    pub fn replace_leaf(self, pane_id: PaneId, make: impl FnOnce(Pane) -> PaneNode) -> PaneNode {
        let mut make = Some(make);
        self.replace_leaf_inner(pane_id, &mut make).0
    }

    fn replace_leaf_inner<F: FnOnce(Pane) -> PaneNode>(
        self,
        pane_id: PaneId,
        make: &mut Option<F>,
    ) -> (PaneNode, bool) {
        match self {
            PaneNode::Leaf(pane) if pane.id == pane_id => {
                (make.take().expect("leaf visited once")(pane), true)
            }
            PaneNode::Leaf(pane) => (PaneNode::Leaf(pane), false),
            PaneNode::Split {
                id,
                axis,
                ratio,
                first,
                second,
            } => {
                let (first, hit) = first.replace_leaf_inner(pane_id, make);
                if hit {
                    return (
                        PaneNode::Split {
                            id,
                            axis,
                            ratio,
                            first: Box::new(first),
                            second,
                        },
                        true,
                    );
                }
                let (second, hit) = second.replace_leaf_inner(pane_id, make);
                (
                    PaneNode::Split {
                        id,
                        axis,
                        ratio,
                        first: Box::new(first),
                        second: Box::new(second),
                    },
                    hit,
                )
            }
        }
    }

    /// 删除叶子；返回 `None` 表示未命中，`Some(None)` 表示删除后为空
    /// （根叶子），`Some(Some(node))` 为折叠后的树。`focus` 收下兄弟子树
    /// 第一个 leaf 的 id。
    pub fn remove_leaf(
        self,
        pane_id: PaneId,
        focus: &mut Option<PaneId>,
    ) -> Option<Option<PaneNode>> {
        match self {
            PaneNode::Leaf(pane) => (pane.id == pane_id).then_some(None),
            PaneNode::Split {
                id,
                axis,
                ratio,
                first,
                second,
            } => {
                if let Some(PaneNode::Leaf(pane)) = Some(first.as_ref())
                    && pane.id == pane_id
                {
                    *focus = second.leaves().next().map(|pane| pane.id);
                    return Some(Some(*second));
                }
                if let Some(PaneNode::Leaf(pane)) = Some(second.as_ref())
                    && pane.id == pane_id
                {
                    *focus = first.leaves().next().map(|pane| pane.id);
                    return Some(Some(*first));
                }
                if let Some(result) = first.as_ref().clone().remove_leaf(pane_id, focus) {
                    return Some(Some(match result {
                        Some(first) => PaneNode::Split {
                            id,
                            axis,
                            ratio,
                            first: Box::new(first),
                            second,
                        },
                        None => *second,
                    }));
                }
                if let Some(result) = second.as_ref().clone().remove_leaf(pane_id, focus) {
                    return Some(Some(match result {
                        Some(second) => PaneNode::Split {
                            id,
                            axis,
                            ratio,
                            first,
                            second: Box::new(second),
                        },
                        None => *first,
                    }));
                }
                None
            }
        }
    }

    pub fn snapshot(&self, available: bool, runtime: &Runtime) -> PaneNodeSnapshot {
        match self {
            PaneNode::Leaf(pane) => {
                let session = runtime.pane_session.get(&pane.id).cloned();
                let cwd_available = pane.cwd.is_dir();
                let status = if session.is_some() {
                    PaneStatus::Live
                } else if runtime.starting.contains(&pane.id) {
                    PaneStatus::Starting
                } else if !available {
                    PaneStatus::Unavailable {
                        reason: PaneStatusReason::ProjectRootMissing,
                    }
                } else if !cwd_available {
                    PaneStatus::Unavailable {
                        reason: PaneStatusReason::CwdMissing,
                    }
                } else {
                    PaneStatus::Dormant {
                        reason: if runtime.spawn_failed.contains(&pane.id) {
                            PaneStatusReason::SpawnFailed
                        } else {
                            PaneStatusReason::NotStarted
                        },
                    }
                };
                PaneNodeSnapshot::Leaf(PaneSnapshot {
                    id: pane.id,
                    cwd: pane.cwd.clone(),
                    size: pane.size,
                    session,
                    cwd_available,
                    status,
                })
            }
            PaneNode::Split {
                id,
                axis,
                ratio,
                first,
                second,
            } => PaneNodeSnapshot::Split {
                id: *id,
                axis: *axis,
                ratio: *ratio,
                first: Box::new(first.snapshot(available, runtime)),
                second: Box::new(second.snapshot(available, runtime)),
            },
        }
    }
}

/// 绝对化 + 词法清理；存在时 canonicalize，缺失时保留词法结果。
pub fn normalize_project_root(path: &Path) -> PathBuf {
    normalize_path(path)
}

pub fn normalize_pane_cwd(path: &Path) -> PathBuf {
    normalize_path(path)
}

fn normalize_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let lexical = lexical_clean(&absolute);
    // canonicalize 只在路径存在时成功；失败保留词法结果（缺失目录必须保留）。
    dunce::canonicalize(&lexical).unwrap_or(lexical)
}

fn lexical_clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "crossh-workspace-{name}-{}-{}",
            std::process::id(),
            now_secs()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn open(ws: &mut Workspace, root: &Path) -> CreatedIds {
        let result = ws.apply(&WorkspaceOp::OpenProject {
            root: root.to_path_buf(),
        });
        assert_eq!(result.outcome, MutateOutcome::Applied);
        result.created
    }

    #[test]
    fn serde_roundtrip_preserves_tree_and_ids() {
        let root = temp_dir("serde");
        let mut ws = Workspace::new();
        let created = open(&mut ws, &root);
        let pane = created.pane.unwrap();
        let split = ws.apply(&WorkspaceOp::SplitPane {
            pane_id: pane,
            axis: Axis::Row,
            cwd: None,
        });
        assert_eq!(split.outcome, MutateOutcome::Applied);
        let encoded = serde_json::to_vec(&ws).unwrap();
        let decoded: Workspace = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(ws, decoded);
        assert_eq!(decoded.format_version, WORKSPACE_FORMAT_VERSION);
        assert!(decoded.next_id > 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ids_are_monotonic_and_revision_only_on_layout_change() {
        let root = temp_dir("revision");
        let mut ws = Workspace::new();
        let created = open(&mut ws, &root);
        let revision = ws.revision;
        assert_eq!(ws.revision, 1);
        assert_eq!(ws.projects.len(), 1);
        let same = ws.apply(&WorkspaceOp::OpenProject { root: root.clone() });
        assert_eq!(same.outcome, MutateOutcome::Unchanged);
        assert_eq!(ws.revision, revision);
        let created_tab = ws.apply(&WorkspaceOp::CreateTab {
            project_id: Some(created.project.unwrap()),
            title: None,
            cwd: None,
            default_command: None,
        });
        assert_eq!(created_tab.outcome, MutateOutcome::Applied);
        assert_eq!(ws.revision, revision + 1);
        let tab = created_tab.created.tab.unwrap();
        assert!(tab.0 > created.tab.unwrap().0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn relative_root_is_absolute_and_missing_root_is_kept() {
        let missing = std::env::temp_dir().join(format!(
            "crossh-workspace-missing-{}-{}",
            std::process::id(),
            now_secs()
        ));
        let mut ws = Workspace::new();
        let created = open(&mut ws, &missing);
        assert_eq!(
            ws.projects[0].root,
            dunce::canonicalize(&missing).unwrap_or_else(|_| missing.clone())
        );
        let snapshot = ws.snapshot(&Runtime::default());
        assert!(!snapshot.projects[0].available);
        let created_id = created.pane.unwrap();
        let pane = ws.pane(created_id).unwrap();
        assert_eq!(pane.cwd, ws.projects[0].root);
        let _ = std::fs::remove_dir_all(missing);
    }

    #[test]
    fn split_close_folds_and_moves_focus() {
        let root = temp_dir("split");
        let mut ws = Workspace::new();
        let created = open(&mut ws, &root);
        let pane = created.pane.unwrap();
        let split = ws
            .apply(&WorkspaceOp::SplitPane {
                pane_id: pane,
                axis: Axis::Row,
                cwd: None,
            })
            .created;
        let split_id = split.split.unwrap();
        let new_pane = split.pane.unwrap();
        assert_eq!(
            ws.tab(created.tab.unwrap()).unwrap().active_pane,
            Some(new_pane)
        );
        let snapshot = ws.snapshot(&Runtime::default());
        match &snapshot.projects[0].tabs[0].layout {
            PaneNodeSnapshot::Split { id, ratio, .. } => {
                assert_eq!(*id, split_id);
                assert_eq!(*ratio, 0.5);
            }
            _ => panic!("expected split"),
        }
        let close = ws.apply(&WorkspaceOp::ClosePane { pane_id: new_pane });
        assert_eq!(close.outcome, MutateOutcome::Applied);
        assert!(matches!(
            ws.tab(created.tab.unwrap()).unwrap().layout,
            PaneNode::Leaf(_)
        ));
        assert_eq!(
            ws.tab(created.tab.unwrap()).unwrap().active_pane,
            Some(pane)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn closing_root_leaf_removes_tab() {
        let root = temp_dir("close-root");
        let mut ws = Workspace::new();
        let created = open(&mut ws, &root);
        let pane = created.pane.unwrap();
        let result = ws.apply(&WorkspaceOp::ClosePane { pane_id: pane });
        assert_eq!(result.outcome, MutateOutcome::Applied);
        assert!(ws.tabs_is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn close_tab_activates_neighbor() {
        let root = temp_dir("close-tab");
        let mut ws = Workspace::new();
        let created = open(&mut ws, &root);
        let project = created.project.unwrap();
        let first_tab = created.tab.unwrap();
        let second = ws
            .apply(&WorkspaceOp::CreateTab {
                project_id: Some(project),
                title: None,
                cwd: None,
                default_command: None,
            })
            .created
            .tab
            .unwrap();
        ws.apply(&WorkspaceOp::CloseTab { tab_id: second });
        assert_eq!(ws.project(project).unwrap().active_tab, Some(first_tab));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn split_ratio_clamps_and_only_changes_ratio() {
        let root = temp_dir("ratio");
        let mut ws = Workspace::new();
        let created = open(&mut ws, &root);
        let pane = created.pane.unwrap();
        let split = ws
            .apply(&WorkspaceOp::SplitPane {
                pane_id: pane,
                axis: Axis::Column,
                cwd: None,
            })
            .created
            .split
            .unwrap();
        let result = ws.apply(&WorkspaceOp::SetSplitRatio {
            split_id: split,
            ratio: 9.0,
        });
        assert_eq!(result.outcome, MutateOutcome::Applied);
        let snapshot = ws.snapshot(&Runtime::default());
        match &snapshot.projects[0].tabs[0].layout {
            PaneNodeSnapshot::Split { ratio, .. } => assert_eq!(*ratio, 0.95),
            _ => panic!("expected split"),
        }
        let unchanged = ws.apply(&WorkspaceOp::SetSplitRatio {
            split_id: split,
            ratio: 0.95,
        });
        assert_eq!(unchanged.outcome, MutateOutcome::Unchanged);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn project_order_moves_existing_to_front_and_forget_falls_back() {
        let a = temp_dir("order-a");
        let b = temp_dir("order-b");
        let mut ws = Workspace::new();
        let created_a = open(&mut ws, &a);
        let created_b = open(&mut ws, &b);
        assert_eq!(ws.projects[0].id, created_b.project.unwrap());
        open(&mut ws, &a);
        assert_eq!(ws.projects[0].id, created_a.project.unwrap());
        ws.apply(&WorkspaceOp::ForgetProject {
            project_id: Some(created_a.project.unwrap()),
        });
        assert_eq!(ws.active_project, Some(created_b.project.unwrap()));
        assert_eq!(ws.projects.len(), 1);
        let _ = std::fs::remove_dir_all(a);
        let _ = std::fs::remove_dir_all(b);
    }

    #[test]
    fn snapshot_status_order_and_runtime_merge_rules() {
        let root = temp_dir("status");
        let mut ws = Workspace::new();
        let created = open(&mut ws, &root);
        let pane = created.pane.unwrap();
        let mut runtime = Runtime::default();
        let snapshot = ws.snapshot(&runtime);
        assert!(matches!(
            snapshot.projects[0].tabs[0].layout,
            PaneNodeSnapshot::Leaf(PaneSnapshot {
                status: PaneStatus::Dormant {
                    reason: PaneStatusReason::NotStarted
                },
                ..
            })
        ));
        runtime.pane_session.insert(pane, "s1".to_string());
        runtime.session_pane.insert("s1".to_string(), pane);
        let snapshot = ws.snapshot(&runtime);
        match &snapshot.projects[0].tabs[0].layout {
            PaneNodeSnapshot::Leaf(pane_snapshot) => {
                assert_eq!(pane_snapshot.session.as_deref(), Some("s1"));
                assert_eq!(pane_snapshot.status, PaneStatus::Live);
            }
            _ => panic!("expected leaf"),
        }
        // 项目根缺失也不把 live pane 降级。
        std::fs::remove_dir_all(&root).unwrap();
        let snapshot = ws.snapshot(&runtime);
        assert!(!snapshot.projects[0].available);
        match &snapshot.projects[0].tabs[0].layout {
            PaneNodeSnapshot::Leaf(pane_snapshot) => {
                assert_eq!(pane_snapshot.status, PaneStatus::Live);
                assert_eq!(pane_snapshot.session.as_deref(), Some("s1"));
            }
            _ => panic!("expected leaf"),
        }
        runtime.pane_session.clear();
        runtime.session_pane.clear();
        let snapshot = ws.snapshot(&runtime);
        match &snapshot.projects[0].tabs[0].layout {
            PaneNodeSnapshot::Leaf(pane_snapshot) => {
                assert!(matches!(
                    pane_snapshot.status,
                    PaneStatus::Unavailable {
                        reason: PaneStatusReason::ProjectRootMissing
                    }
                ));
            }
            _ => panic!("expected leaf"),
        }
    }

    #[test]
    fn active_pane_syncs_three_levels() {
        let root = temp_dir("active");
        let mut ws = Workspace::new();
        let created = open(&mut ws, &root);
        let project = created.project.unwrap();
        let first_pane = created.pane.unwrap();
        let second_tab = ws
            .apply(&WorkspaceOp::CreateTab {
                project_id: Some(project),
                title: None,
                cwd: None,
                default_command: None,
            })
            .created;
        let second_pane = second_tab.pane.unwrap();
        ws.apply(&WorkspaceOp::ActivatePane {
            pane_id: first_pane,
        });
        assert_eq!(ws.active_project, Some(project));
        assert_eq!(
            ws.project(project).unwrap().active_tab,
            Some(created.tab.unwrap())
        );
        assert_eq!(
            ws.tab(created.tab.unwrap()).unwrap().active_pane,
            Some(first_pane)
        );
        ws.apply(&WorkspaceOp::ActivatePane {
            pane_id: second_pane,
        });
        assert_eq!(
            ws.project(project).unwrap().active_tab,
            Some(second_tab.tab.unwrap())
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_2x2_shape_is_expressible() {
        let root = temp_dir("2x2");
        let mut ws = Workspace::new();
        let created = open(&mut ws, &root);
        let top_left = created.pane.unwrap();
        let split_row = ws
            .apply(&WorkspaceOp::SplitPane {
                pane_id: top_left,
                axis: Axis::Row,
                cwd: None,
            })
            .created;
        let top_right = split_row.pane.unwrap();
        let split_left_col = ws
            .apply(&WorkspaceOp::SplitPane {
                pane_id: top_left,
                axis: Axis::Column,
                cwd: None,
            })
            .created;
        let bottom_left = split_left_col.pane.unwrap();
        let split_right_col = ws
            .apply(&WorkspaceOp::SplitPane {
                pane_id: top_right,
                axis: Axis::Column,
                cwd: None,
            })
            .created;
        let bottom_right = split_right_col.pane.unwrap();
        let leaves: Vec<_> = ws
            .tab(created.tab.unwrap())
            .unwrap()
            .layout
            .leaves()
            .map(|p| p.id)
            .collect();
        assert_eq!(leaves.len(), 4);
        assert!(leaves.contains(&top_left));
        assert!(leaves.contains(&top_right));
        assert!(leaves.contains(&bottom_left));
        assert!(leaves.contains(&bottom_right));
        let _ = std::fs::remove_dir_all(root);
    }

    impl Workspace {
        fn tabs_is_empty(&self) -> bool {
            self.projects.iter().all(|project| project.tabs.is_empty())
        }
    }
}

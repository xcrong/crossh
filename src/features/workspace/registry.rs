//! 工作区客户端状态：daemon 快照 + 按 `PaneId` 归属的终端视图与界面状态。
//!
//! 布局、tab、焦点与项目顺序都来自快照，本模块不维护布局副本；渲染层与
//! 交互层只读取 `snapshot` 与这里的映射。

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use std::cell::Cell;

use gpui::{Entity, Subscription, Task};

use crossh_core::git_status::GitStatus;
use crossh_session::workspace::{
    Axis, PaneId, PaneSnapshot, ProjectId, ProjectSnapshot, SplitId, TabId, TabSnapshot,
    WorkspaceSnapshot,
};
use crossh_terminal::ConnState;
use crossh_terminal_view::TerminalView;

use super::projection;
use super::state::{ComposeEntry, GitStatusRefresh};
use super::toaster::ToasterState;
use crossh_core::text_editing::TextEditingState;

/// 分屏比例拖拽的实时状态；`ratio` 是当前预览值，松手后提交 daemon。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SplitDrag {
    pub(crate) split_id: SplitId,
    pub(crate) axis: Axis,
    pub(crate) total_px: f32,
    pub(crate) start_ratio: f32,
    pub(crate) start_position: f32,
    pub(crate) ratio: f32,
}

pub(crate) struct WorkspaceState {
    pub(crate) snapshot: WorkspaceSnapshot,
    pub(crate) pane_views: BTreeMap<PaneId, Entity<TerminalView>>,
    /// 每个视图当前绑定的 daemon 会话 id；daemon 重启后同一 pane 会换会话。
    pub(crate) pane_sessions: BTreeMap<PaneId, String>,
    pub(crate) compose: BTreeMap<PaneId, ComposeEntry>,
    pub(crate) git_status: BTreeMap<PaneId, GitStatus>,
    pub(crate) git_refresh: BTreeMap<PaneId, GitStatusRefresh>,
    pub(crate) connection: ConnState,
    pub(crate) toaster: ToasterState,
    pub(crate) _toast_task: Option<Task<()>>,
    pub(crate) pane_subscriptions: BTreeMap<PaneId, Subscription>,
    /// 每个 tab 的默认命令只在本次 GUI attach 后执行一次。
    pub(crate) default_command_runs: BTreeSet<TabId>,
    /// 拖动分屏手柄时共享给全局鼠标监听的比例状态。
    pub(crate) split_drag_cell: Rc<Cell<Option<SplitDrag>>>,
}

impl WorkspaceState {
    pub(crate) fn new() -> Self {
        Self {
            snapshot: WorkspaceSnapshot {
                revision: 0,
                active_project: None,
                projects: Vec::new(),
                storage_error: None,
            },
            pane_views: BTreeMap::new(),
            pane_sessions: BTreeMap::new(),
            compose: BTreeMap::new(),
            git_status: BTreeMap::new(),
            git_refresh: BTreeMap::new(),
            connection: ConnState::Connecting,
            toaster: ToasterState::default(),
            _toast_task: None,
            pane_subscriptions: BTreeMap::new(),
            default_command_runs: BTreeSet::new(),
            split_drag_cell: Rc::new(Cell::new(None)),
        }
    }

    pub(crate) fn active_project(&self) -> Option<&ProjectSnapshot> {
        projection::active_project(&self.snapshot)
    }

    pub(crate) fn active_tab(&self) -> Option<&TabSnapshot> {
        projection::active_tab(&self.snapshot)
    }

    pub(crate) fn active_pane(&self) -> Option<&PaneSnapshot> {
        projection::active_pane(&self.snapshot)
    }

    pub(crate) fn focused_pane(&self) -> Option<PaneId> {
        projection::active_pane_id(&self.snapshot)
    }

    pub(crate) fn pane(&self, pane_id: PaneId) -> Option<&PaneSnapshot> {
        projection::locate_pane(&self.snapshot, pane_id).map(|location| location.pane)
    }

    pub(crate) fn project(&self, project_id: ProjectId) -> Option<&ProjectSnapshot> {
        self.snapshot
            .projects
            .iter()
            .find(|project| project.id == project_id)
    }

    pub(crate) fn tab(&self, tab_id: TabId) -> Option<(&ProjectSnapshot, &TabSnapshot)> {
        self.snapshot.projects.iter().find_map(|project| {
            project
                .tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .map(|tab| (project, tab))
        })
    }

    pub(crate) fn pane_view(&self, pane_id: PaneId) -> Option<&Entity<TerminalView>> {
        self.pane_views.get(&pane_id)
    }

    pub(crate) fn is_connected(&self) -> bool {
        matches!(self.connection, ConnState::Connected)
    }

    pub(crate) fn compose_visible(&self, pane_id: PaneId) -> bool {
        self.compose
            .get(&pane_id)
            .is_some_and(|entry| entry.visible)
    }

    pub(crate) fn compose_state_for(&self, pane_id: PaneId) -> Option<&TextEditingState> {
        self.compose.get(&pane_id).map(|entry| &entry.state)
    }

    pub(crate) fn compose_entry_mut(&mut self, pane_id: PaneId) -> &mut ComposeEntry {
        self.compose.entry(pane_id).or_default()
    }

    pub(crate) fn compose_visible_for_focused(&self) -> bool {
        self.focused_pane()
            .is_some_and(|pane_id| self.compose_visible(pane_id))
    }

    /// 移除已不在快照中的 pane 的客户端状态（视图实体由 AppShell 一并丢弃）。
    pub(crate) fn retain_panes(&mut self, live: &BTreeSet<PaneId>) {
        self.pane_views.retain(|pane_id, _| live.contains(pane_id));
        self.pane_sessions
            .retain(|pane_id, _| live.contains(pane_id));
        self.pane_subscriptions
            .retain(|pane_id, _| live.contains(pane_id));
        self.compose.retain(|pane_id, _| live.contains(pane_id));
        self.git_status.retain(|pane_id, _| live.contains(pane_id));
        self.git_refresh.retain(|pane_id, _| live.contains(pane_id));
        self.default_command_runs.retain(|tab_id| {
            self.snapshot
                .projects
                .iter()
                .any(|project| project.tabs.iter().any(|tab| tab.id == *tab_id))
        });
    }

    pub(crate) fn live_panes(&self) -> BTreeSet<PaneId> {
        self.snapshot
            .projects
            .iter()
            .flat_map(|project| &project.tabs)
            .flat_map(|tab| tab.layout.leaves())
            .map(|pane| pane.id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_state_has_no_layout_and_is_connecting() {
        let workspace = WorkspaceState::new();
        assert!(workspace.active_project().is_none());
        assert!(workspace.focused_pane().is_none());
        assert!(!workspace.is_connected());
        assert!(!workspace.compose_visible_for_focused());
    }

    #[test]
    fn compose_drafts_are_keyed_by_pane() {
        let mut workspace = WorkspaceState::new();
        let first = PaneId(1);
        let second = PaneId(2);
        workspace.compose_entry_mut(first).state = TextEditingState::new("one");
        workspace.compose_entry_mut(second).state = TextEditingState::new("two");
        assert_eq!(workspace.compose_state_for(first).unwrap().value, "one");
        assert_eq!(workspace.compose_state_for(second).unwrap().value, "two");
        workspace.retain_panes(&BTreeSet::from([second]));
        assert!(workspace.compose_state_for(first).is_none());
        assert_eq!(workspace.compose_state_for(second).unwrap().value, "two");
    }
}

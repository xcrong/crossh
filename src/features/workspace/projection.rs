//! 工作区快照的纯投影：活动位置、树遍历与同 revision 合并。
//!
//! 本模块零 `gpui` 依赖，渲染层只消费这里的查询结果。活动 pane 由
//! `active_project → active_tab → active_pane` 三级投影得出，快照本身没有
//! workspace 级 active_pane。

use std::path::Path;

use crossh_session::workspace::{
    PaneId, PaneNodeSnapshot, PaneSnapshot, PaneStatus, ProjectSnapshot, TabId, TabSnapshot,
    WorkspaceSnapshot,
};

/// pane 在快照中的完整位置。
#[derive(Clone, Copy)]
pub(crate) struct PaneLocation<'a> {
    pub(crate) project: &'a ProjectSnapshot,
    pub(crate) tab: &'a TabSnapshot,
    pub(crate) pane: &'a PaneSnapshot,
}

pub(crate) fn active_project(snapshot: &WorkspaceSnapshot) -> Option<&ProjectSnapshot> {
    snapshot
        .active_project
        .and_then(|id| snapshot.projects.iter().find(|project| project.id == id))
}

pub(crate) fn active_tab(snapshot: &WorkspaceSnapshot) -> Option<&TabSnapshot> {
    let project = active_project(snapshot)?;
    project
        .active_tab
        .and_then(|id| project.tabs.iter().find(|tab| tab.id == id))
}

pub(crate) fn active_pane(snapshot: &WorkspaceSnapshot) -> Option<&PaneSnapshot> {
    let tab = active_tab(snapshot)?;
    let pane_id = tab.active_pane?;
    tab.layout.find_pane(pane_id)
}

pub(crate) fn active_pane_id(snapshot: &WorkspaceSnapshot) -> Option<PaneId> {
    active_pane(snapshot).map(|pane| pane.id)
}

pub(crate) fn project_of_pane(
    snapshot: &WorkspaceSnapshot,
    pane_id: PaneId,
) -> Option<&ProjectSnapshot> {
    locate_pane(snapshot, pane_id).map(|location| location.project)
}

pub(crate) fn locate_pane(
    snapshot: &WorkspaceSnapshot,
    pane_id: PaneId,
) -> Option<PaneLocation<'_>> {
    for project in &snapshot.projects {
        for tab in &project.tabs {
            if let Some(pane) = tab.layout.find_pane(pane_id) {
                return Some(PaneLocation { project, tab, pane });
            }
        }
    }
    None
}

pub(crate) fn project_pane_count(project: &ProjectSnapshot) -> usize {
    project
        .tabs
        .iter()
        .map(|tab| tab.layout.leaves().count())
        .sum()
}

/// tab 的稳定显示标题：自定义标题 > 活跃 pane cwd 的目录名 > 序号。
pub(crate) fn tab_title(tab: &TabSnapshot, index: usize) -> String {
    if let Some(title) = tab
        .title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
    {
        return title.to_string();
    }
    let cwd = tab
        .active_pane
        .and_then(|id| tab.layout.find_pane(id))
        .or_else(|| tab.layout.leaves().next())
        .map(|pane| pane.cwd.as_path());
    cwd.and_then(display_name)
        .unwrap_or_else(|| format!("tab {}", index + 1))
}

fn display_name(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| {
            let text = path.to_string_lossy();
            (!text.is_empty()).then(|| text.into_owned())
        })
}

/// 快照合并：更新 revision 整份替换；同 revision 只合并运行时字段，
/// 布局与活动位置保持当前值；更旧 revision 直接忽略。
pub(crate) fn merge_snapshot(
    current: WorkspaceSnapshot,
    mut incoming: WorkspaceSnapshot,
) -> WorkspaceSnapshot {
    if incoming.revision > current.revision {
        return incoming;
    }
    if incoming.revision < current.revision {
        return current;
    }

    let mut current = current;
    current.storage_error = incoming.storage_error;
    for project in &mut current.projects {
        let Some(next) = incoming
            .projects
            .iter_mut()
            .find(|candidate| candidate.id == project.id)
        else {
            continue;
        };
        project.available = next.available;
        for tab in &mut project.tabs {
            let Some(next_tab) = next.tabs.iter().find(|candidate| candidate.id == tab.id) else {
                continue;
            };
            merge_node_runtime(&mut tab.layout, &next_tab.layout);
        }
    }
    current
}

fn merge_node_runtime(current: &mut PaneNodeSnapshot, incoming: &PaneNodeSnapshot) {
    match (current, incoming) {
        (PaneNodeSnapshot::Leaf(current), PaneNodeSnapshot::Leaf(incoming))
            if current.id == incoming.id =>
        {
            current.session = incoming.session.clone();
            current.cwd_available = incoming.cwd_available;
            current.status = incoming.status.clone();
        }
        (
            PaneNodeSnapshot::Split {
                id: current_id,
                first: current_first,
                second: current_second,
                ..
            },
            PaneNodeSnapshot::Split {
                id: incoming_id,
                first: incoming_first,
                second: incoming_second,
                ..
            },
        ) if current_id == incoming_id => {
            merge_node_runtime(current_first, incoming_first);
            merge_node_runtime(current_second, incoming_second);
        }
        _ => {}
    }
}

/// 分屏比例的本地预览：拖拽期间仅 clamp，松手后才提交 daemon。
pub(crate) fn clamp_split_ratio(ratio: f32) -> f32 {
    if ratio.is_finite() {
        ratio.clamp(0.05, 0.95)
    } else {
        0.5
    }
}

pub(crate) fn pane_status_reason_key(status: &PaneStatus) -> &'static str {
    match status {
        PaneStatus::Starting => "workspace.pane_starting",
        PaneStatus::Live => "workspace.pane_live",
        PaneStatus::Dormant {
            reason: crossh_session::workspace::PaneStatusReason::SpawnFailed,
        } => "workspace.pane_spawn_failed",
        PaneStatus::Dormant {
            reason: crossh_session::workspace::PaneStatusReason::ProjectRootMissing,
        } => "workspace.project_root_missing",
        PaneStatus::Dormant {
            reason: crossh_session::workspace::PaneStatusReason::CwdMissing,
        } => "workspace.cwd_missing",
        PaneStatus::Dormant { .. } => "workspace.pane_dormant",
        PaneStatus::Unavailable {
            reason: crossh_session::workspace::PaneStatusReason::ProjectRootMissing,
        } => "workspace.project_root_missing",
        PaneStatus::Unavailable {
            reason: crossh_session::workspace::PaneStatusReason::CwdMissing,
        } => "workspace.cwd_missing",
        PaneStatus::Unavailable { .. } => "workspace.pane_unavailable",
    }
}

pub(crate) fn tab_index(project: &ProjectSnapshot, tab_id: TabId) -> Option<usize> {
    project.tabs.iter().position(|tab| tab.id == tab_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossh_session::workspace::{
        Axis, PaneStatusReason, ProjectId, SplitId, TermSize, WorkspaceSnapshot,
    };

    fn pane(id: u64, cwd: &str) -> PaneSnapshot {
        PaneSnapshot {
            id: PaneId(id),
            cwd: cwd.into(),
            size: TermSize::default(),
            session: None,
            cwd_available: true,
            status: PaneStatus::Dormant {
                reason: PaneStatusReason::NotStarted,
            },
        }
    }

    fn snapshot() -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            revision: 7,
            active_project: Some(ProjectId(1)),
            projects: vec![
                ProjectSnapshot {
                    id: ProjectId(1),
                    root: "/one".into(),
                    name: None,
                    available: true,
                    active_tab: Some(TabId(2)),
                    tabs: vec![TabSnapshot {
                        id: TabId(2),
                        title: None,
                        default_command: None,
                        active_pane: Some(PaneId(4)),
                        layout: PaneNodeSnapshot::Split {
                            id: SplitId(3),
                            axis: Axis::Row,
                            ratio: 0.5,
                            first: Box::new(PaneNodeSnapshot::Leaf(pane(4, "/one/a"))),
                            second: Box::new(PaneNodeSnapshot::Leaf(pane(5, "/one/b"))),
                        },
                    }],
                },
                ProjectSnapshot {
                    id: ProjectId(9),
                    root: "/two".into(),
                    name: None,
                    available: false,
                    active_tab: None,
                    tabs: Vec::new(),
                },
            ],
            storage_error: None,
        }
    }

    #[test]
    fn active_location_is_projected_through_three_levels() {
        let snapshot = snapshot();
        assert_eq!(active_project(&snapshot).unwrap().id, ProjectId(1));
        assert_eq!(active_tab(&snapshot).unwrap().id, TabId(2));
        assert_eq!(active_pane(&snapshot).unwrap().id, PaneId(4));
        let located = locate_pane(&snapshot, PaneId(5)).unwrap();
        assert_eq!(located.project.id, ProjectId(1));
        assert_eq!(located.tab.id, TabId(2));
        assert_eq!(project_pane_count(located.project), 2);
        assert!(locate_pane(&snapshot, PaneId(99)).is_none());
    }

    #[test]
    fn same_revision_merges_only_runtime_fields() {
        let current = snapshot();
        let mut incoming = current.clone();
        incoming.projects[0].available = false;
        incoming.projects[0].tabs[0].title = Some("changed".into());
        incoming.projects[0].tabs[0].active_pane = Some(PaneId(5));
        let PaneNodeSnapshot::Split {
            first,
            ratio: split_ratio,
            ..
        } = &mut incoming.projects[0].tabs[0].layout
        else {
            unreachable!()
        };
        *split_ratio = 0.9;
        let PaneNodeSnapshot::Leaf(first) = first.as_mut() else {
            unreachable!()
        };
        first.session = Some("sess-1".into());
        first.status = PaneStatus::Live;
        first.cwd_available = false;

        let merged = merge_snapshot(current, incoming);
        assert_eq!(merged.revision, 7);
        assert!(!merged.projects[0].available);
        // 布局与活动位置保持当前值。
        assert!(merged.projects[0].tabs[0].title.is_none());
        assert_eq!(merged.projects[0].tabs[0].active_pane, Some(PaneId(4)));
        let PaneNodeSnapshot::Split {
            first,
            ratio: split_ratio,
            ..
        } = &merged.projects[0].tabs[0].layout
        else {
            unreachable!()
        };
        assert_eq!(*split_ratio, 0.5);
        let PaneNodeSnapshot::Leaf(first) = first.as_ref() else {
            unreachable!()
        };
        assert_eq!(first.session.as_deref(), Some("sess-1"));
        assert_eq!(first.status, PaneStatus::Live);
        assert!(!first.cwd_available);
    }

    #[test]
    fn newer_revision_replaces_and_older_is_ignored() {
        let current = snapshot();
        let mut newer = current.clone();
        newer.revision = 8;
        newer.active_project = None;
        assert!(
            merge_snapshot(current.clone(), newer)
                .active_project
                .is_none()
        );

        let mut older = current.clone();
        older.revision = 6;
        older.active_project = None;
        assert_eq!(
            merge_snapshot(current, older).active_project,
            Some(ProjectId(1))
        );
    }

    #[test]
    fn split_ratio_preview_is_clamped_and_nan_falls_back() {
        assert_eq!(clamp_split_ratio(0.01), 0.05);
        assert_eq!(clamp_split_ratio(0.5), 0.5);
        assert_eq!(clamp_split_ratio(0.99), 0.95);
        assert_eq!(clamp_split_ratio(f32::NAN), 0.5);
    }

    #[test]
    fn tab_title_prefers_custom_then_cwd_basename() {
        let snapshot = snapshot();
        let tab = &snapshot.projects[0].tabs[0];
        assert_eq!(tab_title(tab, 0), "a");
        let mut titled = tab.clone();
        titled.title = Some("work".into());
        assert_eq!(tab_title(&titled, 0), "work");
    }
}

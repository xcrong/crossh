//! Workspace-owned session registry.
//!
//! This state is deliberately independent from rendering. The shell coordinates
//! actions, while the registry owns the collections that describe open panes.

use std::collections::BTreeMap;
use std::path::PathBuf;

use gpui::Subscription;

use super::view::{ActiveView, LocalDir, LocalSession, LocalSessionId, Tab};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SplitSide {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerminalSplitState {
    pub(crate) left: ActiveView,
    pub(crate) right: ActiveView,
    pub(crate) focused: SplitSide,
}

impl TerminalSplitState {
    pub(crate) fn new(left: ActiveView, right: ActiveView) -> Self {
        Self {
            left,
            right,
            focused: SplitSide::Right,
        }
    }

    pub(crate) fn focus(&mut self, side: SplitSide) {
        self.focused = side;
    }

    pub(crate) fn focused_view(self) -> ActiveView {
        match self.focused {
            SplitSide::Left => self.left,
            SplitSide::Right => self.right,
        }
    }
}

pub(crate) struct SessionRegistry {
    pub(crate) remote_tabs: Vec<Tab>,
    pub(crate) local_sessions: BTreeMap<LocalSessionId, LocalSession>,
    pub(crate) local_dirs: BTreeMap<PathBuf, LocalDir>,
    pub(crate) next_local_session_id: LocalSessionId,
    /// GPUI subscriptions are owned by the workspace for the lifetime of its sessions.
    pub(crate) terminal_subscriptions: Vec<Subscription>,
}

impl SessionRegistry {
    pub(crate) fn new(local_dirs: BTreeMap<PathBuf, LocalDir>) -> Self {
        Self {
            remote_tabs: Vec::new(),
            local_sessions: BTreeMap::new(),
            local_dirs,
            next_local_session_id: 1,
            terminal_subscriptions: Vec::new(),
        }
    }

    pub(crate) fn allocate_local_session_id(&mut self) -> LocalSessionId {
        let id = self.next_local_session_id;
        self.next_local_session_id += 1;
        id
    }
}

pub(crate) struct WorkspaceState {
    pub(crate) sessions: SessionRegistry,
    pub(crate) active_view: Option<ActiveView>,
    pub(crate) terminal_split: Option<TerminalSplitState>,
}

impl WorkspaceState {
    pub(crate) fn new(local_dirs: BTreeMap<PathBuf, LocalDir>) -> Self {
        Self {
            sessions: SessionRegistry::new(local_dirs),
            active_view: None,
            terminal_split: None,
        }
    }

    pub(crate) fn begin_terminal_split(&mut self, right: ActiveView) -> bool {
        let Some(left) = self.active_view else {
            return false;
        };
        if self.terminal_split.is_some() || left == right {
            return false;
        }
        self.terminal_split = Some(TerminalSplitState::new(left, right));
        true
    }

    pub(crate) fn focused_view(&self) -> Option<ActiveView> {
        self.terminal_split
            .map(TerminalSplitState::focused_view)
            .or(self.active_view)
    }

    pub(crate) fn is_split_secondary(&self, view: ActiveView) -> bool {
        self.terminal_split.is_some_and(|split| split.right == view)
    }

    pub(crate) fn focus_terminal_split(&mut self, side: SplitSide) -> bool {
        let Some(split) = &mut self.terminal_split else {
            return false;
        };
        split.focus(side);
        true
    }

    pub(crate) fn focus_split_view(&mut self, view: ActiveView) -> bool {
        let Some(split) = &mut self.terminal_split else {
            return false;
        };
        let side = if split.left == view {
            SplitSide::Left
        } else if split.right == view {
            SplitSide::Right
        } else {
            return false;
        };
        split.focus(side);
        true
    }

    pub(crate) fn collapse_terminal_split(&mut self) -> bool {
        self.terminal_split.take().is_some()
    }

    /// Remove a split reference before the underlying tab/session is deleted.
    /// The remaining pane becomes the regular active view when the left pane is
    /// closed; closing the right pane simply returns to the left pane.
    pub(crate) fn prepare_split_view_close(&mut self, view: ActiveView) -> bool {
        let Some(split) = self.terminal_split else {
            return false;
        };
        if split.left == view {
            self.active_view = Some(split.right);
            self.terminal_split = None;
            true
        } else if split.right == view {
            self.terminal_split = None;
            true
        } else {
            false
        }
    }

    /// Remote tabs are stored in a vector, so a removal shifts references after
    /// the removed index. This only repairs split-owned references; normal
    /// active-view fallback remains in the tab close operation.
    pub(crate) fn remap_split_remote_tab_indices(&mut self, removed: usize) {
        let Some(split) = self.terminal_split else {
            return;
        };
        let Some(left) = remap_remote_tab(split.left, removed) else {
            self.terminal_split = None;
            return;
        };
        let Some(right) = remap_remote_tab(split.right, removed) else {
            self.terminal_split = None;
            return;
        };
        self.terminal_split = Some(TerminalSplitState {
            left,
            right,
            ..split
        });
    }
}

fn remap_remote_tab(view: ActiveView, removed: usize) -> Option<ActiveView> {
    match view {
        ActiveView::RemoteTab(index) if index == removed => None,
        ActiveView::RemoteTab(index) if index > removed => Some(ActiveView::RemoteTab(index - 1)),
        _ => Some(view),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_ids_are_monotonic_and_state_starts_inactive() {
        let remembered = BTreeMap::from([(
            PathBuf::from("/workspace"),
            LocalDir {
                project_dir: PathBuf::from("/workspace"),
                sessions: Vec::new(),
                active_session: None,
            },
        )]);
        let mut workspace = WorkspaceState::new(remembered);

        assert_eq!(workspace.active_view, None);
        assert!(workspace.sessions.remote_tabs.is_empty());
        assert!(workspace.sessions.local_sessions.is_empty());
        assert_eq!(workspace.sessions.local_dirs.len(), 1);
        assert_eq!(workspace.sessions.allocate_local_session_id(), 1);
        assert_eq!(workspace.sessions.allocate_local_session_id(), 2);
        assert_eq!(workspace.sessions.allocate_local_session_id(), 3);
    }

    #[test]
    fn terminal_split_starts_on_the_right_and_tracks_focus() {
        let left = ActiveView::LocalSession(1);
        let right = ActiveView::LocalSession(2);
        let mut split = TerminalSplitState::new(left, right);

        assert_eq!(split.focused_view(), right);

        split.focus(SplitSide::Left);
        assert_eq!(split.focused_view(), left);
    }

    #[test]
    fn workspace_does_not_replace_an_existing_terminal_split() {
        let mut workspace = WorkspaceState::new(BTreeMap::new());
        workspace.active_view = Some(ActiveView::LocalSession(1));

        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(2)));
        assert!(!workspace.begin_terminal_split(ActiveView::LocalSession(3)));
        assert_eq!(
            workspace.terminal_split.unwrap().right,
            ActiveView::LocalSession(2)
        );
    }

    #[test]
    fn closing_a_split_view_keeps_the_other_view_active() {
        let mut workspace = WorkspaceState::new(BTreeMap::new());
        workspace.active_view = Some(ActiveView::LocalSession(1));
        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(2)));

        assert!(workspace.prepare_split_view_close(ActiveView::LocalSession(2)));
        assert_eq!(workspace.active_view, Some(ActiveView::LocalSession(1)));
        assert!(workspace.terminal_split.is_none());

        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(2)));
        assert!(workspace.prepare_split_view_close(ActiveView::LocalSession(1)));
        assert_eq!(workspace.active_view, Some(ActiveView::LocalSession(2)));
        assert!(workspace.terminal_split.is_none());
    }

    #[test]
    fn removing_a_remote_tab_repairs_split_indices() {
        let mut workspace = WorkspaceState::new(BTreeMap::new());
        workspace.active_view = Some(ActiveView::RemoteTab(0));
        assert!(workspace.begin_terminal_split(ActiveView::RemoteTab(3)));

        workspace.remap_split_remote_tab_indices(1);
        let split = workspace.terminal_split.expect("split should remain valid");
        assert_eq!(split.left, ActiveView::RemoteTab(0));
        assert_eq!(split.right, ActiveView::RemoteTab(2));

        workspace.remap_split_remote_tab_indices(2);
        assert!(workspace.terminal_split.is_none());
    }
}

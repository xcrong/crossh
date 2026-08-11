//! Workspace-owned session registry.
//!
//! This state is deliberately independent from rendering. The shell coordinates
//! actions, while the registry owns the collections that describe open panes.

use std::collections::BTreeMap;
use std::path::PathBuf;

use gpui::Subscription;

use super::view::{ActiveView, LocalDir, LocalSession, LocalSessionId, Tab};

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
}

impl WorkspaceState {
    pub(crate) fn new(local_dirs: BTreeMap<PathBuf, LocalDir>) -> Self {
        Self {
            sessions: SessionRegistry::new(local_dirs),
            active_view: None,
        }
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
}

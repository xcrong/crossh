//! AgentSessionRuntime, mirroring pi-agent's `AgentSessionRuntime`.
//!
//! Owns `session + services(cwd)` and provides uniform `switch/new/fork/import`.

use crate::manager::SessionManager;
use crate::session::AgentSession;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone)]
pub struct AgentSessionServices {
    pub cwd: PathBuf,
    pub manager: Arc<dyn SessionManager>,
}

impl AgentSessionServices {
    pub fn new(cwd: PathBuf, manager: Arc<dyn SessionManager>) -> Self {
        Self { cwd, manager }
    }
}

pub struct AgentSessionRuntime {
    pub session: AgentSession,
    pub services: AgentSessionServices,
    pub session_path: Option<PathBuf>,
}

impl AgentSessionRuntime {
    pub fn new(
        session: AgentSession,
        session_path: Option<PathBuf>,
        services: AgentSessionServices,
    ) -> Self {
        Self {
            session,
            services,
            session_path,
        }
    }

    pub fn cwd(&self) -> &Path {
        &self.services.cwd
    }

    /// Switch to an existing session file. Teardown-first, then apply.
    pub fn switch_session(&mut self, path: &Path) -> Result<(), String> {
        let loaded = self.services.manager.load_session(path)?;
        self.services.cwd = loaded.cwd.clone();
        self.session = loaded;
        self.session_path = Some(path.to_path_buf());
        Ok(())
    }

    pub fn new_session(&mut self) -> Result<(), String> {
        let (path, session) = self.services.manager.create_session(&self.services.cwd)?;
        self.session = session;
        self.session_path = Some(path);
        Ok(())
    }

    pub fn fork(&mut self, entry_id: &str) -> Result<(), String> {
        let (path, forked) =
            self.services
                .manager
                .fork_session(&self.services.cwd, &self.session, entry_id)?;
        self.session = forked;
        self.session_path = Some(path);
        Ok(())
    }

    pub fn save(&self) -> Result<(), String> {
        if let Some(path) = &self.session_path {
            self.services.manager.save_session(path, &self.session)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::InMemorySessionManager;
    use crate::{Message, Role};
    use std::sync::Arc;
    use tempfile::tempdir;

    #[test]
    fn spec_20260821_agent_runtime_runtime_switch_preserves_on_failure() {
        let mgr: Arc<dyn SessionManager> = Arc::new(InMemorySessionManager::new());
        let dir = tempdir().unwrap();
        let (p1, s1) = mgr.create_session(dir.path()).unwrap();
        let mut rt = AgentSessionRuntime::new(
            s1.clone(),
            Some(p1.clone()),
            AgentSessionServices::new(dir.path().to_path_buf(), mgr.clone()),
        );
        let original_id = rt.session.id.clone();
        let err = rt.switch_session(Path::new("/nonexistent/path.jsonl"));
        assert!(err.is_err());
        assert_eq!(rt.session.id, original_id);
    }

    #[test]
    fn spec_20260821_agent_runtime_runtime_fork_creates_new_branch() {
        let mgr: Arc<dyn SessionManager> = Arc::new(InMemorySessionManager::new());
        let dir = tempdir().unwrap();
        let (p, mut s) = mgr.create_session(dir.path()).unwrap();
        s.messages.push(Message::new(Role::User, "hello"));
        mgr.save_session(&p, &s).unwrap();
        let mut rt = AgentSessionRuntime::new(
            s.clone(),
            Some(p),
            AgentSessionServices::new(dir.path().to_path_buf(), mgr.clone()),
        );
        let entries = crate::session::tree_entries_from_messages(&rt.session);
        let entry_id = entries[0].id.clone();
        rt.fork(&entry_id).unwrap();
        assert_ne!(rt.session.id, s.id);
        assert_eq!(rt.session.messages.len(), 1);
    }
}

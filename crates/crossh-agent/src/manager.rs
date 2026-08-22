//! SessionManager trait, mirroring pi-agent's `SessionManager`.

use crate::entry::{SessionEntry, SessionEntryData};
use crate::session::{
    AgentSession, AgentSessionSummary, load_session as load_legacy, save_session as save_legacy,
    session_root,
};
use std::collections::BTreeMap;
#[cfg(test)]
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub trait SessionManager: Send + Sync {
    fn create_session(&self, cwd: &Path) -> Result<(PathBuf, AgentSession), String>;
    fn load_session(&self, path: &Path) -> Result<AgentSession, String>;
    fn save_session(&self, path: &Path, session: &AgentSession) -> Result<(), String>;
    fn list_sessions(&self, cwd: &Path) -> Result<Vec<AgentSessionSummary>, String>;
    fn tree_entries(&self, session: &AgentSession) -> Vec<SessionEntry> {
        crate::session::tree_entries_from_messages(session)
    }
    fn fork_session(
        &self,
        cwd: &Path,
        source: &AgentSession,
        entry_id: &str,
    ) -> Result<(PathBuf, AgentSession), String>;
}

#[derive(Clone, Default)]
pub struct FsSessionManager;

impl SessionManager for FsSessionManager {
    fn create_session(&self, cwd: &Path) -> Result<(PathBuf, AgentSession), String> {
        crate::session::create_session(cwd)
    }
    fn load_session(&self, path: &Path) -> Result<AgentSession, String> {
        load_legacy(path)
    }
    fn save_session(&self, path: &Path, session: &AgentSession) -> Result<(), String> {
        save_legacy(path, session)
    }
    fn list_sessions(&self, cwd: &Path) -> Result<Vec<AgentSessionSummary>, String> {
        crate::session::list_sessions(cwd)
    }
    fn fork_session(
        &self,
        cwd: &Path,
        source: &AgentSession,
        entry_id: &str,
    ) -> Result<(PathBuf, AgentSession), String> {
        fork_fs(cwd, source, entry_id)
    }
}

#[derive(Clone, Default)]
pub struct InMemorySessionManager {
    inner: Arc<Mutex<BTreeMap<String, AgentSession>>>,
}

impl InMemorySessionManager {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SessionManager for InMemorySessionManager {
    fn create_session(&self, cwd: &Path) -> Result<(PathBuf, AgentSession), String> {
        let session = AgentSession::new(cwd.to_path_buf());
        let path = PathBuf::from(format!("/in-memory/{}.jsonl", session.id));
        self.inner
            .lock()
            .unwrap()
            .insert(session.id.clone(), session.clone());
        Ok((path, session))
    }
    fn load_session(&self, path: &Path) -> Result<AgentSession, String> {
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("invalid in-memory path")?;
        self.inner
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| "session not found".into())
    }
    fn save_session(&self, _path: &Path, session: &AgentSession) -> Result<(), String> {
        self.inner
            .lock()
            .unwrap()
            .insert(session.id.clone(), session.clone());
        Ok(())
    }
    fn list_sessions(&self, _cwd: &Path) -> Result<Vec<AgentSessionSummary>, String> {
        let map = self.inner.lock().unwrap();
        let mut out = Vec::new();
        for s in map.values() {
            out.push(AgentSessionSummary {
                path: PathBuf::from(format!("/in-memory/{}.jsonl", s.id)),
                id: s.id.clone(),
                name: s.name.clone(),
                cwd: s.cwd.clone(),
                updated_at: s.updated_at,
                message_count: s.messages.len(),
            });
        }
        out.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
        Ok(out)
    }
    fn fork_session(
        &self,
        cwd: &Path,
        source: &AgentSession,
        entry_id: &str,
    ) -> Result<(PathBuf, AgentSession), String> {
        let forked = fork_in_memory(cwd, source, entry_id)?;
        let path = PathBuf::from(format!("/in-memory/{}.jsonl", forked.id));
        self.inner
            .lock()
            .unwrap()
            .insert(forked.id.clone(), forked.clone());
        Ok((path, forked))
    }
}

fn fork_in_memory(
    cwd: &Path,
    source: &AgentSession,
    entry_id: &str,
) -> Result<AgentSession, String> {
    let entries = crate::session::tree_entries_from_messages(source);
    let pos = entries
        .iter()
        .position(|e| e.id == entry_id)
        .ok_or_else(|| "entry not found".to_string())?;
    let mut forked = AgentSession::new(cwd.to_path_buf());
    forked.name = source.name.as_ref().map(|n| format!("{n} fork"));
    for entry in entries.iter().take(pos + 1) {
        if let SessionEntryData::Message { message } = &entry.data {
            forked.messages.push(message.clone());
        }
    }
    Ok(forked)
}

fn fork_fs(
    cwd: &Path,
    source: &AgentSession,
    entry_id: &str,
) -> Result<(PathBuf, AgentSession), String> {
    let forked = fork_in_memory(cwd, source, entry_id)?;
    // Persist to real fs: create a file with the forked id, atomic write via save_legacy.
    let forked_path = session_root(cwd)?.join(format!("{}.jsonl", forked.id));
    // Ensure directory exists and is restricted, then atomic save.
    save_legacy(&forked_path, &forked)?;
    // If create_session was previously used to allocate a temp id, we no longer need it.
    // Directly return the forked session with its own id.
    Ok((forked_path, forked))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, Role};
    use tempfile::tempdir;

    #[test]
    fn spec_20260821_agent_runtime_tree_entries_preserve_parent_chain() {
        let mut s = AgentSession::new("/tmp/p");
        s.messages.push(Message::new(Role::User, "a"));
        s.messages.push(Message::new(Role::Assistant, "b"));
        s.messages.push(Message::new(Role::User, "c"));
        let entries = crate::session::tree_entries_from_messages(&s);
        assert_eq!(entries.len(), 3);
        assert!(entries[0].parent_id.is_none());
        assert_eq!(
            entries[1].parent_id.as_deref(),
            Some(entries[0].id.as_str())
        );
        assert_eq!(
            entries[2].parent_id.as_deref(),
            Some(entries[1].id.as_str())
        );
    }

    #[test]
    fn spec_20260821_agent_runtime_fork_shares_ancestors_only() {
        let dir = tempdir().unwrap();
        let mut src = AgentSession::new(dir.path());
        src.messages.push(Message::new(Role::User, "turn1"));
        src.messages.push(Message::new(Role::Assistant, "ans1"));
        src.messages.push(Message::new(Role::User, "turn2"));
        let entries = crate::session::tree_entries_from_messages(&src);
        let fork_id = entries[1].id.clone();
        let mgr = FsSessionManager;
        let (_path, forked) = mgr.fork_session(dir.path(), &src, &fork_id).unwrap();
        assert_eq!(forked.messages.len(), 2);
        assert_eq!(forked.messages[0].text, "turn1");
        assert_eq!(forked.messages[1].text, "ans1");
        assert_eq!(src.messages.len(), 3);
        // Fs fork must have written a real file.
        assert!(_path.exists());
    }

    #[test]
    fn spec_20260821_agent_runtime_in_memory_manager_is_isolated() {
        let mgr = InMemorySessionManager::new();
        let dir = Path::new("/tmp/proj");
        let (_p, s) = mgr.create_session(dir).unwrap();
        let loaded = mgr.load_session(&_p).unwrap();
        assert_eq!(s.id, loaded.id);
    }

    #[test]
    fn in_memory_fork_does_not_touch_fs() {
        let mgr = InMemorySessionManager::new();
        let dir = tempfile::tempdir().unwrap();
        let mut src = AgentSession::new(dir.path());
        src.messages.push(Message::new(Role::User, "a"));
        src.messages.push(Message::new(Role::Assistant, "b"));
        let entries = crate::session::tree_entries_from_messages(&src);
        let fork_id = entries[0].id.clone();
        let (path, forked) = mgr.fork_session(dir.path(), &src, &fork_id).unwrap();
        assert_eq!(forked.messages.len(), 1);
        // Path is in-memory, file must not exist on real fs.
        assert!(path.to_string_lossy().starts_with("/in-memory"));
        assert!(!path.exists());
        // Session directory must stay empty.
        let root = crate::session::session_root(dir.path()).unwrap();
        let exists = root.exists()
            && fs::read_dir(&root)
                .map(|mut d| d.next().is_some())
                .unwrap_or(false);
        assert!(!exists, "in-memory fork must not pollute fs");
    }
}

//! Git Remote 的无 UI 状态和异步请求上下文。

use std::path::PathBuf;

use crossh_core::git::GitError;
use crossh_core::git_remote::RemoteSummary;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum RemoteListState {
    #[default]
    Idle,
    Loading,
    Ready,
    Error(String),
}

impl RemoteListState {
    pub(super) fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }
}

pub(super) struct RemoteState {
    pub(super) entries: Vec<RemoteSummary>,
    pub(super) selected: Option<String>,
    pub(super) list_state: RemoteListState,
    list_generation: u64,
}

pub(super) struct RemoteListRequest {
    pub(super) generation: u64,
    pub(super) cwd: PathBuf,
}

impl Default for RemoteState {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            selected: None,
            list_state: RemoteListState::Idle,
            list_generation: 0,
        }
    }
}

impl RemoteState {
    pub(super) fn begin_list(&mut self, cwd: PathBuf, force: bool) -> Option<RemoteListRequest> {
        if self.list_state.is_loading() && !force {
            return None;
        }
        self.list_generation = self.list_generation.wrapping_add(1);
        self.list_state = RemoteListState::Loading;
        Some(RemoteListRequest {
            generation: self.list_generation,
            cwd,
        })
    }

    pub(super) fn apply_list(
        &mut self,
        request: RemoteListRequest,
        result: Result<Vec<RemoteSummary>, GitError>,
    ) -> bool {
        if self.list_generation != request.generation {
            return false;
        }
        match result {
            Ok(entries) => {
                let next_selected = self
                    .selected
                    .as_ref()
                    .filter(|name| entries.iter().any(|entry| &entry.name == *name))
                    .cloned()
                    .or_else(|| entries.first().map(|entry| entry.name.clone()));
                self.entries = entries;
                self.selected = next_selected;
                self.list_state = RemoteListState::Ready;
            }
            Err(error) => {
                self.list_state = RemoteListState::Error(error.to_string());
            }
        }
        true
    }

    pub(super) fn select(&mut self, name: String) -> bool {
        if !self.entries.iter().any(|entry| entry.name == name)
            || self.selected.as_deref() == Some(name.as_str())
        {
            return false;
        }
        self.selected = Some(name);
        true
    }

    pub(super) fn selected_remote(&self) -> Option<&RemoteSummary> {
        self.selected
            .as_ref()
            .and_then(|name| self.entries.iter().find(|entry| &entry.name == name))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn remote(name: &str) -> RemoteSummary {
        RemoteSummary {
            name: name.to_string(),
            fetch_url: Some(format!("git@local:{name}.git")),
            push_url: Some(format!("git@local:{name}.git")),
        }
    }

    #[test]
    fn list_preserves_existing_selection() {
        let mut state = RemoteState::default();
        let request = state.begin_list(PathBuf::from("/repo"), false).unwrap();
        state.apply_list(request, Ok(vec![remote("origin"), remote("upstream")]));

        assert_eq!(state.selected.as_deref(), Some("origin"));
        assert!(state.select("upstream".to_string()));

        let request = state.begin_list(PathBuf::from("/repo"), true).unwrap();
        state.apply_list(request, Ok(vec![remote("origin"), remote("upstream")]));

        assert_eq!(state.selected.as_deref(), Some("upstream"));
    }

    #[test]
    fn stale_list_result_cannot_replace_newer_generation() {
        let mut state = RemoteState::default();
        let first = state.begin_list(PathBuf::from("/repo"), false).unwrap();
        let second = state.begin_list(PathBuf::from("/repo"), true).unwrap();

        assert!(!state.apply_list(first, Ok(vec![remote("old")])));
        assert!(state.apply_list(second, Ok(vec![remote("new")])));
        assert_eq!(state.entries[0].name, "new");
    }
}

//! Git Branch 的无 UI 状态和异步请求上下文。

use std::path::PathBuf;

use crossh_core::git::GitError;
use crossh_core::git_branch::BranchSummary;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum BranchListState {
    #[default]
    Idle,
    Loading,
    Ready,
    Error(String),
}

impl BranchListState {
    pub(super) fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }
}

pub(super) struct BranchState {
    pub(super) entries: Vec<BranchSummary>,
    pub(super) selected: Option<String>,
    pub(super) list_state: BranchListState,
    list_generation: u64,
}

pub(super) struct BranchListRequest {
    pub(super) generation: u64,
    pub(super) cwd: PathBuf,
}

impl Default for BranchState {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            selected: None,
            list_state: BranchListState::Idle,
            list_generation: 0,
        }
    }
}

impl BranchState {
    pub(super) fn begin_list(&mut self, cwd: PathBuf, force: bool) -> Option<BranchListRequest> {
        if self.list_state.is_loading() && !force {
            return None;
        }
        self.list_generation = self.list_generation.wrapping_add(1);
        self.list_state = BranchListState::Loading;
        Some(BranchListRequest {
            generation: self.list_generation,
            cwd,
        })
    }

    pub(super) fn apply_list(
        &mut self,
        request: BranchListRequest,
        result: Result<Vec<BranchSummary>, GitError>,
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
                    .or_else(|| {
                        entries
                            .iter()
                            .find(|entry| entry.current)
                            .map(|entry| entry.name.clone())
                    })
                    .or_else(|| entries.first().map(|entry| entry.name.clone()));
                self.entries = entries;
                self.selected = next_selected;
                self.list_state = BranchListState::Ready;
            }
            Err(error) => {
                self.list_state = BranchListState::Error(error.to_string());
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

    pub(super) fn selected_branch(&self) -> Option<&BranchSummary> {
        self.selected
            .as_ref()
            .and_then(|name| self.entries.iter().find(|entry| &entry.name == name))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn branch(name: &str, current: bool) -> BranchSummary {
        BranchSummary {
            name: name.to_string(),
            current,
            upstream: None,
            ahead: 0,
            behind: 0,
            upstream_gone: false,
            commit: "1234567".to_string(),
            subject: format!("Commit for {name}"),
        }
    }

    #[test]
    fn list_defaults_to_current_and_preserves_existing_selection() {
        let mut state = BranchState::default();
        let main = branch("main", true);
        let feature = branch("feature/history", false);
        let request = state.begin_list(PathBuf::from("/repo"), false).unwrap();
        state.apply_list(request, Ok(vec![main.clone(), feature.clone()]));

        assert_eq!(state.selected.as_deref(), Some("main"));
        assert!(state.select(feature.name.clone()));

        let request = state.begin_list(PathBuf::from("/repo"), true).unwrap();
        state.apply_list(request, Ok(vec![main, feature]));

        assert_eq!(state.selected.as_deref(), Some("feature/history"));
    }

    #[test]
    fn stale_list_result_cannot_replace_newer_generation() {
        let mut state = BranchState::default();
        let first = state.begin_list(PathBuf::from("/repo"), false).unwrap();
        let second = state.begin_list(PathBuf::from("/repo"), true).unwrap();

        assert!(!state.apply_list(first, Ok(vec![branch("old", true)])));
        assert!(state.apply_list(second, Ok(vec![branch("new", true)])));
        assert_eq!(state.entries[0].name, "new");
    }
}

//! Git Stash 的无 UI 状态和异步请求上下文。

use std::path::PathBuf;

use crossh_core::git::GitError;
use crossh_core::git_stash::StashSummary;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum StashListState {
    #[default]
    Idle,
    Loading,
    Ready,
    Error(String),
}

impl StashListState {
    pub(super) fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }
}

pub(super) struct StashState {
    pub(super) entries: Vec<StashSummary>,
    pub(super) selected: Option<String>,
    pub(super) list_state: StashListState,
    list_generation: u64,
}

pub(super) struct StashListRequest {
    pub(super) generation: u64,
    pub(super) cwd: PathBuf,
}

impl Default for StashState {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            selected: None,
            list_state: StashListState::Idle,
            list_generation: 0,
        }
    }
}

impl StashState {
    pub(super) fn begin_list(&mut self, cwd: PathBuf, force: bool) -> Option<StashListRequest> {
        if self.list_state.is_loading() && !force {
            return None;
        }
        self.list_generation = self.list_generation.wrapping_add(1);
        self.list_state = StashListState::Loading;
        Some(StashListRequest {
            generation: self.list_generation,
            cwd,
        })
    }

    pub(super) fn apply_list(
        &mut self,
        request: StashListRequest,
        result: Result<Vec<StashSummary>, GitError>,
    ) -> bool {
        if self.list_generation != request.generation {
            return false;
        }
        match result {
            Ok(entries) => {
                let next_selected = self
                    .selected
                    .as_ref()
                    .filter(|selector| entries.iter().any(|entry| &entry.selector == *selector))
                    .cloned()
                    .or_else(|| entries.first().map(|entry| entry.selector.clone()));
                self.entries = entries;
                self.selected = next_selected;
                self.list_state = StashListState::Ready;
            }
            Err(error) => {
                self.list_state = StashListState::Error(error.to_string());
            }
        }
        true
    }

    pub(super) fn select(&mut self, selector: String) -> bool {
        if !self.entries.iter().any(|entry| entry.selector == selector)
            || self.selected.as_deref() == Some(selector.as_str())
        {
            return false;
        }
        self.selected = Some(selector);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn stash(selector: &str) -> StashSummary {
        StashSummary {
            selector: selector.to_string(),
            id: "1234567890abcdef".to_string(),
            date: "2026-08-16 12:00:00 +0800".to_string(),
            message: format!("WIP {selector}"),
        }
    }

    #[test]
    fn list_selection_defaults_to_first_and_preserves_existing_stash() {
        let mut state = StashState::default();
        let first = stash("stash@{0}");
        let second = stash("stash@{1}");
        let request = state.begin_list(PathBuf::from("/repo"), false).unwrap();
        state.apply_list(request, Ok(vec![first.clone(), second.clone()]));

        assert_eq!(state.selected.as_deref(), Some(first.selector.as_str()));
        assert!(state.select(second.selector.clone()));

        let request = state.begin_list(PathBuf::from("/repo"), true).unwrap();
        state.apply_list(request, Ok(vec![first, second.clone()]));

        assert_eq!(state.selected.as_deref(), Some(second.selector.as_str()));
    }

    #[test]
    fn stale_list_result_cannot_replace_newer_generation() {
        let mut state = StashState::default();
        let first = state.begin_list(PathBuf::from("/repo"), false).unwrap();
        let second = state.begin_list(PathBuf::from("/repo"), true).unwrap();

        assert!(!state.apply_list(first, Ok(vec![stash("stash@{old}")])));
        assert!(state.apply_list(second, Ok(vec![stash("stash@{new}")])));
        assert_eq!(state.entries[0].selector, "stash@{new}");
    }
}

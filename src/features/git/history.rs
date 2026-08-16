//! Git History 的无 UI 状态和异步请求上下文。

use std::path::PathBuf;

use crossh_core::git::GitError;
use crossh_core::git_history::{CommitDetail, CommitSummary, DEFAULT_HISTORY_LIMIT};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum HistoryListState {
    #[default]
    Idle,
    Loading,
    Ready,
    Error(String),
}

impl HistoryListState {
    pub(super) fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }

    pub(super) fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum HistoryDetailState {
    #[default]
    Idle,
    Loading(String),
    Ready(CommitDetail),
    Error(String),
}

impl HistoryDetailState {
    pub(super) fn selected_id(&self) -> Option<&str> {
        match self {
            Self::Loading(id) => Some(id),
            Self::Ready(detail) => Some(detail.summary.id.as_str()),
            Self::Idle | Self::Error(_) => None,
        }
    }
}

pub(super) struct HistoryState {
    pub(super) entries: Vec<CommitSummary>,
    pub(super) selected: Option<String>,
    pub(super) list_state: HistoryListState,
    pub(super) detail: HistoryDetailState,
    list_generation: u64,
    detail_generation: u64,
}

pub(super) struct HistoryListRequest {
    pub(super) generation: u64,
    pub(super) cwd: PathBuf,
    pub(super) limit: usize,
}

pub(super) struct HistoryDetailRequest {
    pub(super) generation: u64,
    pub(super) cwd: PathBuf,
    pub(super) id: String,
}

impl Default for HistoryState {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            selected: None,
            list_state: HistoryListState::Idle,
            detail: HistoryDetailState::Idle,
            list_generation: 0,
            detail_generation: 0,
        }
    }
}

impl HistoryState {
    pub(super) fn begin_list(&mut self, cwd: PathBuf, force: bool) -> Option<HistoryListRequest> {
        if self.list_state.is_loading() && !force {
            return None;
        }
        self.list_generation = self.list_generation.wrapping_add(1);
        self.list_state = HistoryListState::Loading;
        Some(HistoryListRequest {
            generation: self.list_generation,
            cwd,
            limit: DEFAULT_HISTORY_LIMIT,
        })
    }

    pub(super) fn apply_list(
        &mut self,
        request: HistoryListRequest,
        result: Result<Vec<CommitSummary>, GitError>,
    ) -> bool {
        if self.list_generation != request.generation {
            return false;
        }
        match result {
            Ok(entries) => {
                let next_selected = self
                    .selected
                    .as_ref()
                    .filter(|id| entries.iter().any(|entry| &entry.id == *id))
                    .cloned()
                    .or_else(|| entries.first().map(|entry| entry.id.clone()));
                let selection_changed = self.selected != next_selected;
                self.entries = entries;
                self.selected = next_selected;
                self.list_state = HistoryListState::Ready;
                if selection_changed {
                    self.detail = HistoryDetailState::Idle;
                }
            }
            Err(error) => {
                self.list_state = HistoryListState::Error(error.to_string());
                return true;
            }
        }
        true
    }

    pub(super) fn select(&mut self, id: String) -> bool {
        if !self.entries.iter().any(|entry| entry.id == id) || self.selected == Some(id.clone()) {
            return false;
        }
        self.selected = Some(id);
        self.detail = HistoryDetailState::Idle;
        true
    }

    pub(super) fn begin_detail(&mut self, cwd: PathBuf) -> Option<HistoryDetailRequest> {
        let id = self.selected.clone()?;
        self.detail_generation = self.detail_generation.wrapping_add(1);
        self.detail = HistoryDetailState::Loading(id.clone());
        Some(HistoryDetailRequest {
            generation: self.detail_generation,
            cwd,
            id,
        })
    }

    pub(super) fn apply_detail(
        &mut self,
        request: HistoryDetailRequest,
        result: Result<CommitDetail, GitError>,
    ) -> bool {
        if self.detail_generation != request.generation
            || self.selected.as_deref() != Some(request.id.as_str())
        {
            return false;
        }
        self.detail = match result {
            Ok(detail) => HistoryDetailState::Ready(detail),
            Err(error) => HistoryDetailState::Error(error.to_string()),
        };
        true
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn summary(id: &str) -> CommitSummary {
        CommitSummary {
            id: id.to_string(),
            short_id: id[..7].to_string(),
            author: "Author".to_string(),
            date: "2026-08-16T12:00:00+08:00".to_string(),
            subject: format!("Commit {id}"),
            parents: Vec::new(),
        }
    }

    #[test]
    fn list_selection_defaults_to_latest_and_preserves_existing_commit() {
        let mut state = HistoryState::default();
        let first = summary("1111111111111111111111111111111111111111");
        let second = summary("2222222222222222222222222222222222222222");
        let request = state.begin_list(PathBuf::from("/repo"), false).unwrap();
        state.apply_list(request, Ok(vec![first.clone(), second.clone()]));

        assert_eq!(state.selected.as_deref(), Some(first.id.as_str()));

        state.select(second.id.clone());
        let request = state.begin_list(PathBuf::from("/repo"), true).unwrap();
        state.apply_list(request, Ok(vec![first.clone(), second.clone()]));

        assert_eq!(state.selected.as_deref(), Some(second.id.as_str()));
    }

    #[test]
    fn stale_detail_result_cannot_replace_a_newer_selection() {
        let mut state = HistoryState::default();
        let first = summary("1111111111111111111111111111111111111111");
        let second = summary("2222222222222222222222222222222222222222");
        state.entries = vec![first.clone(), second.clone()];
        state.selected = Some(first.id.clone());
        let request = state.begin_detail(PathBuf::from("/repo")).unwrap();
        state.select(second.id.clone());

        assert!(!state.apply_detail(
            request,
            Ok(CommitDetail {
                summary: first,
                body: String::new(),
                files: Vec::new(),
            })
        ));
        assert_eq!(state.detail, HistoryDetailState::Idle);
    }
}

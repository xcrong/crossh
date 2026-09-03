//! Git History 的无 UI 状态和异步请求上下文。

use std::collections::HashMap;
use std::path::PathBuf;

use crossh_core::git::{FileDiff, GitError};
use crossh_core::git_history::{
    CommitDetail, CommitSummary, DEFAULT_HISTORY_LIMIT, HistoryRef, HistorySnapshot,
};
use crossh_core::git_history_graph::{HistoryGraphRow, layout_history};

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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum HistoryFileDiffState {
    #[default]
    Idle,
    Loading,
    Ready(Option<FileDiff>),
    Error(String),
}

pub(super) struct HistoryState {
    pub(super) entries: Vec<CommitSummary>,
    pub(super) refs: Vec<HistoryRef>,
    pub(super) graph: Vec<HistoryGraphRow>,
    pub(super) selected: Option<String>,
    pub(super) query: String,
    pub(super) list_state: HistoryListState,
    pub(super) detail: HistoryDetailState,
    pub(super) selected_file: Option<String>,
    pub(super) file_diffs: HashMap<String, HistoryFileDiffState>,
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

pub(super) struct HistoryFileDiffRequest {
    pub(super) cwd: PathBuf,
    pub(super) commit_id: String,
    pub(super) path: String,
    pub(super) old_path: Option<String>,
}

impl Default for HistoryState {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            refs: Vec::new(),
            graph: Vec::new(),
            selected: None,
            query: String::new(),
            list_state: HistoryListState::Idle,
            detail: HistoryDetailState::Idle,
            selected_file: None,
            file_diffs: HashMap::new(),
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
        result: Result<HistorySnapshot, GitError>,
    ) -> bool {
        if self.list_generation != request.generation {
            return false;
        }
        match result {
            Ok(snapshot) => {
                let entries = snapshot.entries;
                let next_selected = self
                    .selected
                    .as_ref()
                    .filter(|id| {
                        entries.iter().any(|entry| &entry.id == *id)
                            && self.entry_is_visible_by(&entries, &snapshot.refs, id)
                    })
                    .cloned()
                    .or_else(|| {
                        entries
                            .iter()
                            .find(|entry| {
                                self.entry_is_visible_by(&entries, &snapshot.refs, &entry.id)
                            })
                            .map(|entry| entry.id.clone())
                    });
                let selection_changed = self.selected != next_selected;
                self.graph = layout_history(&entries);
                self.entries = entries;
                self.refs = snapshot.refs;
                self.selected = next_selected;
                self.list_state = HistoryListState::Ready;
                if selection_changed {
                    self.detail = HistoryDetailState::Idle;
                    self.clear_file_state();
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
        if !self.visible_rows().iter().any(|entry| entry.entry.id == id)
            || self.selected == Some(id.clone())
        {
            return false;
        }
        self.selected = Some(id);
        self.detail = HistoryDetailState::Idle;
        self.clear_file_state();
        true
    }

    pub(super) fn set_query(&mut self, query: String) -> bool {
        let query = query.trim().to_string();
        if self.query == query {
            return false;
        }
        self.query = query;
        let next_selected = self
            .selected
            .as_ref()
            .filter(|id| self.visible_rows().iter().any(|row| &row.entry.id == *id))
            .cloned()
            .or_else(|| self.visible_rows().first().map(|row| row.entry.id.clone()));
        let selection_changed = self.selected != next_selected;
        self.selected = next_selected;
        if selection_changed {
            self.detail = HistoryDetailState::Idle;
            self.clear_file_state();
        }
        true
    }

    pub(super) fn visible_rows(&self) -> Vec<HistoryRow> {
        self.entries
            .iter()
            .zip(self.graph.iter())
            .filter(|(entry, _)| self.entry_is_visible(entry, &self.refs))
            .map(|(entry, graph)| HistoryRow {
                entry: entry.clone(),
                graph: graph.clone(),
            })
            .collect()
    }

    pub(super) fn refs_for(&self, commit_id: &str) -> Vec<HistoryRef> {
        self.refs
            .iter()
            .filter(|reference| reference.target == commit_id)
            .cloned()
            .collect()
    }

    fn entry_is_visible(&self, entry: &CommitSummary, refs: &[HistoryRef]) -> bool {
        self.entry_is_visible_by(&self.entries, refs, &entry.id)
    }

    fn entry_is_visible_by(
        &self,
        entries: &[CommitSummary],
        refs: &[HistoryRef],
        id: &str,
    ) -> bool {
        let Some(entry) = entries.iter().find(|entry| entry.id == id) else {
            return false;
        };
        let query = self.query.trim().to_ascii_lowercase();
        if query.is_empty() {
            return true;
        }
        let matches = |value: &str| value.to_ascii_lowercase().contains(&query);
        matches(&entry.id)
            || matches(&entry.short_id)
            || matches(&entry.author)
            || matches(&entry.date)
            || matches(&entry.subject)
            || refs
                .iter()
                .filter(|reference| reference.target == entry.id)
                .any(|reference| matches(&reference.name))
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
        match result {
            Ok(detail) => {
                // 默认选中首个文件，详情 pane 可直接加载它的 diff。
                if self
                    .selected_file
                    .as_ref()
                    .is_none_or(|path| !detail.files.iter().any(|file| &file.path == path))
                {
                    self.selected_file = detail.files.first().map(|file| file.path.clone());
                }
                self.detail = HistoryDetailState::Ready(detail);
            }
            Err(error) => {
                self.clear_file_state();
                self.detail = HistoryDetailState::Error(error.to_string());
            }
        }
        true
    }

    fn clear_file_state(&mut self) {
        self.selected_file = None;
        self.file_diffs.clear();
    }

    pub(super) fn select_file(&mut self, path: String) -> bool {
        let known = matches!(
            &self.detail,
            HistoryDetailState::Ready(detail)
                if detail.files.iter().any(|file| file.path == path)
        );
        if !known || self.selected_file.as_deref() == Some(path.as_str()) {
            return false;
        }
        self.selected_file = Some(path);
        true
    }

    pub(super) fn begin_file_diff(&mut self, cwd: PathBuf) -> Option<HistoryFileDiffRequest> {
        let commit_id = self.selected.clone()?;
        let path = self.selected_file.clone()?;
        let HistoryDetailState::Ready(detail) = &self.detail else {
            return None;
        };
        let old_path = detail
            .files
            .iter()
            .find(|file| file.path == path)
            .map(|file| file.old_path.clone())?;
        // 提交内容不可变，同 commit+path 的结果可直接复用；Error 允许重新加载。
        if matches!(
            self.file_diffs.get(&path),
            Some(HistoryFileDiffState::Loading) | Some(HistoryFileDiffState::Ready(_))
        ) {
            return None;
        }
        self.file_diffs
            .insert(path.clone(), HistoryFileDiffState::Loading);
        Some(HistoryFileDiffRequest {
            cwd,
            commit_id,
            path,
            old_path,
        })
    }

    pub(super) fn apply_file_diff(
        &mut self,
        request: &HistoryFileDiffRequest,
        result: Result<Option<FileDiff>, GitError>,
    ) -> bool {
        if self.selected.as_deref() != Some(request.commit_id.as_str()) {
            return false;
        }
        let known = matches!(
            &self.detail,
            HistoryDetailState::Ready(detail)
                if detail.files.iter().any(|file| file.path == request.path)
        );
        if !known {
            return false;
        }
        self.file_diffs.insert(
            request.path.clone(),
            match result {
                Ok(diff) => HistoryFileDiffState::Ready(diff),
                Err(error) => HistoryFileDiffState::Error(error.to_string()),
            },
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crossh_core::git_history_graph::layout_history;

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
        state.apply_list(
            request,
            Ok(HistorySnapshot {
                entries: vec![first.clone(), second.clone()],
                refs: Vec::new(),
            }),
        );

        assert_eq!(state.selected.as_deref(), Some(first.id.as_str()));

        state.select(second.id.clone());
        let request = state.begin_list(PathBuf::from("/repo"), true).unwrap();
        state.apply_list(
            request,
            Ok(HistorySnapshot {
                entries: vec![first.clone(), second.clone()],
                refs: Vec::new(),
            }),
        );

        assert_eq!(state.selected.as_deref(), Some(second.id.as_str()));
    }

    #[test]
    fn stale_detail_result_cannot_replace_a_newer_selection() {
        let mut state = HistoryState::default();
        let first = summary("1111111111111111111111111111111111111111");
        let second = summary("2222222222222222222222222222222222222222");
        state.entries = vec![first.clone(), second.clone()];
        state.graph = layout_history(&state.entries);
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

    #[test]
    fn query_filters_rows_without_losing_graph_alignment() {
        let mut state = HistoryState::default();
        let first = summary("1111111111111111111111111111111111111111");
        let second = summary("2222222222222222222222222222222222222222");
        state.entries = vec![first.clone(), second.clone()];
        state.graph = layout_history(&state.entries);

        assert!(state.set_query("Commit 2".to_string()));
        let rows = state.visible_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].entry.id, second.id);
        assert_eq!(state.selected.as_deref(), Some(second.id.as_str()));
    }

    fn detail_with_files(id: &str, paths: &[&str]) -> CommitDetail {
        use crossh_core::git_history::CommitFileChange;

        CommitDetail {
            summary: summary(id),
            body: String::new(),
            files: paths
                .iter()
                .map(|path| CommitFileChange {
                    path: path.to_string(),
                    old_path: None,
                    insertions: 1,
                    deletions: 0,
                    binary: false,
                })
                .collect(),
        }
    }

    #[test]
    fn detail_defaults_to_first_file_and_caches_its_diff() {
        use crossh_core::git::FileDiff;

        let mut state = HistoryState::default();
        let id = "1111111111111111111111111111111111111111";
        state.entries = vec![summary(id)];
        state.graph = layout_history(&state.entries);
        state.selected = Some(id.to_string());
        let request = state.begin_detail(PathBuf::from("/repo")).unwrap();
        assert!(state.apply_detail(request, Ok(detail_with_files(id, &["a.txt", "b.txt"]))));

        assert_eq!(state.selected_file.as_deref(), Some("a.txt"));

        let diff_request = state
            .begin_file_diff(PathBuf::from("/repo"))
            .expect("first file starts loading");
        assert_eq!(diff_request.path, "a.txt");
        // Loading 中重复请求直接复用，不再发起。
        assert!(state.begin_file_diff(PathBuf::from("/repo")).is_none());

        assert!(state.apply_file_diff(&diff_request, Ok(Some(FileDiff::default()))));
        assert!(matches!(
            state.file_diffs.get("a.txt"),
            Some(HistoryFileDiffState::Ready(_))
        ));
        // 缓存命中不再发起请求。
        assert!(state.begin_file_diff(PathBuf::from("/repo")).is_none());
    }

    #[test]
    fn stale_file_diff_is_dropped_after_commit_switch() {
        let mut state = HistoryState::default();
        let first = "1111111111111111111111111111111111111111";
        let second = "2222222222222222222222222222222222222222";
        state.entries = vec![summary(first), summary(second)];
        state.graph = layout_history(&state.entries);
        state.selected = Some(first.to_string());
        let detail_request = state.begin_detail(PathBuf::from("/repo")).unwrap();
        assert!(state.apply_detail(detail_request, Ok(detail_with_files(first, &["a.txt"]))));
        let diff_request = state.begin_file_diff(PathBuf::from("/repo")).unwrap();

        state.select(second.to_string());
        assert_eq!(state.selected_file, None);
        assert!(!state.apply_file_diff(&diff_request, Ok(None)));
    }

    #[test]
    fn unknown_file_cannot_be_selected() {
        let mut state = HistoryState::default();
        let id = "1111111111111111111111111111111111111111";
        state.entries = vec![summary(id)];
        state.graph = layout_history(&state.entries);
        state.selected = Some(id.to_string());
        let request = state.begin_detail(PathBuf::from("/repo")).unwrap();
        assert!(state.apply_detail(request, Ok(detail_with_files(id, &["a.txt", "b.txt"]))));

        assert_eq!(state.selected_file.as_deref(), Some("a.txt"));
        assert!(!state.select_file("missing.txt".to_string()));
        assert!(state.select_file("b.txt".to_string()));
        assert_eq!(state.selected_file.as_deref(), Some("b.txt"));
        assert!(!state.select_file("b.txt".to_string()));
    }
}

#[derive(Clone, Debug)]
pub(super) struct HistoryRow {
    pub(super) entry: CommitSummary,
    pub(super) graph: HistoryGraphRow,
}

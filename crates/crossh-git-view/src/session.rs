//! Git Viewer 的无 UI 会话状态。
//!
//! 这里保存 Git 工作台的领域状态和异步请求上下文，不依赖 GPUI。

use std::collections::BTreeSet;
use std::path::PathBuf;

use crossh_core::git::{ChangeScan, FileChange, FileDiff, GitError};
use crossh_core::git_conflict::ConflictResolution;
use crossh_core::git_status::GitStatus;
use crossh_core::terminal::path_display_name;

use super::branch::BranchState;
use super::history::HistoryState;
use super::remote::RemoteState;
use super::stash::StashState;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct ChangeKey {
    pub(super) path: String,
    pub(super) staged: bool,
}

impl From<&FileChange> for ChangeKey {
    fn from(change: &FileChange) -> Self {
        Self {
            path: change.path.clone(),
            staged: change.staged,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DiffState {
    Idle,
    Loading(ChangeKey),
    Ready(ChangeKey, Option<FileDiff>),
    Error(ChangeKey, String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum OperationState {
    #[default]
    Idle,
    Running,
    Error(String),
}

/// Git 扫描只允许一个在途请求；重复请求合并为完成后的下一次扫描。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct RefreshState {
    in_flight: bool,
    pending: bool,
}

impl RefreshState {
    pub(super) fn request(&mut self) -> bool {
        if self.in_flight {
            self.pending = true;
            false
        } else {
            self.in_flight = true;
            true
        }
    }

    pub(super) fn finish(&mut self) -> bool {
        self.in_flight = false;
        std::mem::take(&mut self.pending)
    }

    pub(super) fn in_flight(self) -> bool {
        self.in_flight
    }
}

pub(super) fn selected_index(
    changes: &[FileChange],
    selected: Option<&ChangeKey>,
) -> Option<usize> {
    selected.and_then(|selected| {
        changes
            .iter()
            .position(|change| ChangeKey::from(change) == *selected)
    })
}

fn reconcile_selection(
    changes: &[FileChange],
    selected: Option<&ChangeKey>,
    selected_changes: &BTreeSet<ChangeKey>,
    previous_index: Option<usize>,
) -> Option<ChangeKey> {
    if let Some(index) = selected_index(changes, selected) {
        return Some(ChangeKey::from(&changes[index]));
    }
    if let Some(change) = changes
        .iter()
        .find(|change| selected_changes.contains(&ChangeKey::from(*change)))
    {
        return Some(ChangeKey::from(change));
    }
    let last = changes.len().checked_sub(1)?;
    let index = previous_index.unwrap_or(0).min(last);
    Some(ChangeKey::from(&changes[index]))
}

fn should_refresh_diff(
    force: bool,
    previous_changes: &[FileChange],
    next_changes: &[FileChange],
    previous_selected: Option<&ChangeKey>,
    next_selected: Option<&ChangeKey>,
) -> bool {
    if force {
        return true;
    }
    selected_change(previous_changes, previous_selected)
        != selected_change(next_changes, next_selected)
}

fn selected_change<'a>(
    changes: &'a [FileChange],
    selected: Option<&ChangeKey>,
) -> Option<&'a FileChange> {
    selected_index(changes, selected).and_then(|index| changes.get(index))
}
fn operation_affects_path(operation: &GitOperation, path: &str) -> bool {
    match operation {
        GitOperation::Stage(paths)
        | GitOperation::Unstage(paths)
        | GitOperation::Discard(paths) => paths.iter().any(|candidate| candidate == path),
        GitOperation::StageHunk { entry, .. } | GitOperation::UnstageHunk { entry, .. } => {
            entry.path == path
        }
        GitOperation::Commit(_)
        | GitOperation::Push
        | GitOperation::Pull
        | GitOperation::SwitchBranch(_)
        | GitOperation::StashPush
        | GitOperation::StashApply(_)
        | GitOperation::StashPop(_)
        | GitOperation::StashDrop(_)
        | GitOperation::FetchRemote(_)
        | GitOperation::FetchAllRemotes
        | GitOperation::AddRemote { .. }
        | GitOperation::RemoveRemote(_) => false,
        GitOperation::ResolveConflict {
            path: candidate, ..
        } => candidate == path,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum GitOperation {
    Stage(Vec<String>),
    Unstage(Vec<String>),
    Discard(Vec<String>),
    StageHunk {
        entry: FileChange,
        hunk_index: usize,
    },
    UnstageHunk {
        entry: FileChange,
        hunk_index: usize,
    },
    Commit(String),
    Push,
    Pull,
    SwitchBranch(String),
    StashPush,
    StashApply(String),
    StashPop(String),
    StashDrop(String),
    FetchRemote(String),
    FetchAllRemotes,
    AddRemote {
        name: String,
        url: String,
    },
    RemoveRemote(String),
    ResolveConflict {
        path: String,
        resolution: ConflictResolution,
    },
}

pub(super) struct GitSession {
    pub(super) cwd: PathBuf,
    pub(super) label: String,
    pub(super) changes: Vec<FileChange>,
    pub(super) status: Option<GitStatus>,
    pub(super) selected: Option<ChangeKey>,
    pub(super) selected_changes: BTreeSet<ChangeKey>,
    pub(super) diff: DiffState,
    pub(super) initial_loading: bool,
    pub(super) refresh: RefreshState,
    pub(super) load_error: Option<String>,
    pub(super) operation: OperationState,
    pub(super) branch: BranchState,
    pub(super) history: HistoryState,
    pub(super) stash: StashState,
    pub(super) remote: RemoteState,
    pub(super) list_generation: u64,
    pub(super) diff_generation: u64,
    pub(super) operation_generation: u64,
    pub(super) force_diff_refresh_pending: bool,
}

pub(super) struct RefreshRequest {
    pub(super) generation: u64,
    pub(super) cwd: PathBuf,
    pub(super) previous_index: Option<usize>,
    pub(super) previous_changes: Vec<FileChange>,
    pub(super) previous_selected: Option<ChangeKey>,
    pub(super) force_diff_reload: bool,
    pub(super) was_initial_loading: bool,
}

pub(super) struct DiffRequest {
    pub(super) generation: u64,
    pub(super) cwd: PathBuf,
    pub(super) key: ChangeKey,
    pub(super) entry: FileChange,
}

pub(super) struct OperationRequest {
    pub(super) generation: u64,
    pub(super) cwd: PathBuf,
    pub(super) operation: GitOperation,
    pub(super) desired_selection: Option<ChangeKey>,
    pub(super) desired_selected_changes: BTreeSet<ChangeKey>,
    pub(super) clear_message: bool,
}

pub(super) struct RefreshCompletion {
    pub(super) refresh_again: bool,
    pub(super) reload_diff: bool,
    pub(super) state_changed: bool,
}

pub(super) struct OperationCompletion {
    pub(super) accepted: bool,
    pub(super) clear_message: bool,
}

impl GitSession {
    pub(super) fn new(cwd: PathBuf) -> Self {
        Self {
            label: path_display_name(&cwd),
            cwd,
            changes: Vec::new(),
            status: None,
            selected: None,
            selected_changes: BTreeSet::new(),
            diff: DiffState::Idle,
            initial_loading: true,
            refresh: RefreshState::default(),
            load_error: None,
            operation: OperationState::Idle,
            branch: BranchState::default(),
            history: HistoryState::default(),
            stash: StashState::default(),
            remote: RemoteState::default(),
            list_generation: 0,
            diff_generation: 0,
            operation_generation: 0,
            force_diff_refresh_pending: false,
        }
    }

    pub(super) fn begin_refresh(&mut self, force_diff_reload: bool) -> Option<RefreshRequest> {
        self.force_diff_refresh_pending |= force_diff_reload;
        if !self.refresh.request() {
            return None;
        }
        let force_diff_reload = std::mem::take(&mut self.force_diff_refresh_pending);
        self.list_generation = self.list_generation.wrapping_add(1);
        let generation = self.list_generation;
        let previous_index = selected_index(&self.changes, self.selected.as_ref());
        let previous_changes = self.changes.clone();
        let previous_selected = self.selected.clone();
        let was_initial_loading = self.initial_loading;
        self.initial_loading = self.changes.is_empty();

        Some(RefreshRequest {
            generation,
            cwd: self.cwd.clone(),
            previous_index,
            previous_changes,
            previous_selected,
            force_diff_reload,
            was_initial_loading,
        })
    }

    pub(super) fn apply_scan(
        &mut self,
        request: RefreshRequest,
        scan: Result<ChangeScan, GitError>,
    ) -> RefreshCompletion {
        let refresh_again = self.refresh.finish();
        if self.list_generation != request.generation {
            return RefreshCompletion {
                refresh_again,
                reload_diff: false,
                state_changed: false,
            };
        }

        let mut state_changed = request.was_initial_loading;
        let reload_diff;
        match scan {
            Ok(scan) => {
                let next_selected = reconcile_selection(
                    &scan.changes,
                    self.selected.as_ref(),
                    &self.selected_changes,
                    request.previous_index,
                );
                let mut next_selected_changes = self
                    .selected_changes
                    .iter()
                    .filter(|key| selected_index(&scan.changes, Some(key)).is_some())
                    .cloned()
                    .collect::<BTreeSet<_>>();
                if let Some(selected) = &next_selected {
                    next_selected_changes.insert(selected.clone());
                }
                reload_diff = should_refresh_diff(
                    request.force_diff_reload,
                    &request.previous_changes,
                    &scan.changes,
                    request.previous_selected.as_ref(),
                    next_selected.as_ref(),
                );
                state_changed |= self.changes != scan.changes;
                state_changed |= self.selected != next_selected;
                state_changed |= self.selected_changes != next_selected_changes;
                state_changed |= self.status != scan.status;
                state_changed |= self.load_error.take().is_some();
                self.changes = scan.changes;
                self.selected = next_selected;
                self.selected_changes = next_selected_changes;
                self.status = scan.status;
            }
            Err(error) => {
                reload_diff = false;
                let error = error.to_string();
                state_changed |= self.load_error.as_deref() != Some(error.as_str());
                self.load_error = Some(error);
            }
        }
        self.initial_loading = false;

        RefreshCompletion {
            refresh_again,
            reload_diff,
            state_changed,
        }
    }

    pub(super) fn select(&mut self, key: ChangeKey, additive: bool, range: bool) {
        if range {
            let target_index = selected_index(&self.changes, Some(&key));
            let anchor_index = selected_index(&self.changes, self.selected.as_ref());
            if let (Some(target_index), Some(anchor_index)) = (target_index, anchor_index) {
                if !additive {
                    self.selected_changes.clear();
                }
                let start = target_index.min(anchor_index);
                let end = target_index.max(anchor_index);
                self.selected_changes
                    .extend(self.changes[start..=end].iter().map(ChangeKey::from));
                self.selected = Some(key);
                return;
            }
        }

        if additive {
            if self.selected_changes.remove(&key) {
                if self.selected.as_ref() == Some(&key) {
                    self.selected = self
                        .changes
                        .iter()
                        .find(|change| self.selected_changes.contains(&ChangeKey::from(*change)))
                        .map(ChangeKey::from);
                }
            } else {
                self.selected_changes.insert(key.clone());
                self.selected = Some(key);
            }
        } else {
            self.selected_changes.clear();
            self.selected_changes.insert(key.clone());
            self.selected = Some(key);
        }
    }

    pub(super) fn select_all(&mut self) {
        self.selected_changes = self.changes.iter().map(ChangeKey::from).collect();
        self.selected = self.changes.first().map(ChangeKey::from);
    }

    pub(super) fn clear_selection(&mut self) {
        self.selected_changes.clear();
        self.selected = None;
    }

    pub(super) fn is_selected(&self, key: &ChangeKey) -> bool {
        self.selected_changes.contains(key)
    }

    pub(super) fn selected_count(&self) -> usize {
        self.selected_changes.len()
    }

    pub(super) fn selected_keys(&self) -> Vec<ChangeKey> {
        self.changes
            .iter()
            .filter_map(|change| {
                let key = ChangeKey::from(change);
                self.selected_changes.contains(&key).then_some(key)
            })
            .collect()
    }

    pub(super) fn selected_paths(&self, staged: bool) -> Vec<String> {
        self.changes
            .iter()
            .filter(|change| {
                change.staged == staged && self.selected_changes.contains(&ChangeKey::from(*change))
            })
            .map(|change| change.path.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(super) fn discard_paths(&self) -> Vec<String> {
        self.changes
            .iter()
            .filter(|change| {
                !change.staged
                    && !matches!(change.status, crossh_core::git::ChangeStatus::Conflict)
                    && self.selected_changes.contains(&ChangeKey::from(*change))
            })
            .map(|change| change.path.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(super) fn can_discard_selection(&self) -> bool {
        let selected = self.selected_keys();
        !selected.is_empty()
            && selected.iter().all(|key| {
                self.changes
                    .iter()
                    .find(|change| ChangeKey::from(*change) == *key)
                    .is_some_and(|change| {
                        !change.staged
                            && !matches!(change.status, crossh_core::git::ChangeStatus::Conflict)
                    })
            })
    }

    pub(super) fn begin_diff(&mut self) -> Option<DiffRequest> {
        let Some(index) = selected_index(&self.changes, self.selected.as_ref()) else {
            self.diff = DiffState::Idle;
            return None;
        };
        let entry = self.changes[index].clone();
        let key = ChangeKey::from(&entry);
        self.diff_generation = self.diff_generation.wrapping_add(1);
        let generation = self.diff_generation;
        let keep_current_diff = matches!(
            &self.diff,
            DiffState::Ready(current_key, _) if current_key == &key
        );
        if !keep_current_diff {
            self.diff = DiffState::Loading(key.clone());
        }

        Some(DiffRequest {
            generation,
            cwd: self.cwd.clone(),
            key,
            entry,
        })
    }

    pub(super) fn apply_diff(
        &mut self,
        request: DiffRequest,
        result: Result<Option<FileDiff>, GitError>,
    ) -> bool {
        if self.diff_generation != request.generation
            || self.selected.as_ref() != Some(&request.key)
        {
            return false;
        }
        self.diff = match result {
            Ok(file_diff) => DiffState::Ready(request.key, file_diff),
            Err(error) => DiffState::Error(request.key, error.to_string()),
        };
        true
    }

    pub(super) fn begin_operation(
        &mut self,
        operation: GitOperation,
        desired_selection: Option<ChangeKey>,
        clear_message: bool,
    ) -> Option<OperationRequest> {
        if matches!(self.operation, OperationState::Running) {
            return None;
        }
        self.operation_generation = self.operation_generation.wrapping_add(1);
        self.operation = OperationState::Running;
        let desired_selected_changes = self.selected_changes_after_operation(&operation);
        Some(OperationRequest {
            generation: self.operation_generation,
            cwd: self.cwd.clone(),
            operation,
            desired_selection,
            desired_selected_changes,
            clear_message,
        })
    }

    fn selected_changes_after_operation(&self, operation: &GitOperation) -> BTreeSet<ChangeKey> {
        self.selected_changes
            .iter()
            .filter_map(|key| {
                if !operation_affects_path(operation, &key.path) {
                    return Some(key.clone());
                }
                match operation {
                    GitOperation::Stage(_) | GitOperation::StageHunk { .. } if !key.staged => {
                        Some(ChangeKey {
                            path: key.path.clone(),
                            staged: true,
                        })
                    }
                    GitOperation::Unstage(_) | GitOperation::UnstageHunk { .. } if key.staged => {
                        Some(ChangeKey {
                            path: key.path.clone(),
                            staged: false,
                        })
                    }
                    GitOperation::Discard(_) if !key.staged => None,
                    _ => Some(key.clone()),
                }
            })
            .collect()
    }

    pub(super) fn apply_operation(
        &mut self,
        request: OperationRequest,
        result: Result<(), GitError>,
    ) -> OperationCompletion {
        if self.operation_generation != request.generation {
            return OperationCompletion {
                accepted: false,
                clear_message: false,
            };
        }
        match result {
            Ok(()) => {
                self.operation = OperationState::Idle;
                self.selected = request.desired_selection;
                self.selected_changes = request.desired_selected_changes;
                if let Some(selected) = &self.selected {
                    self.selected_changes.insert(selected.clone());
                }
                OperationCompletion {
                    accepted: true,
                    clear_message: request.clear_message,
                }
            }
            Err(error) => {
                self.operation = OperationState::Error(error.to_string());
                OperationCompletion {
                    accepted: true,
                    clear_message: false,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crossh_core::git::{
        ChangeScan, ChangeStatus, DiffLine, DiffLineKind, FileChange, FileDiff,
    };
    use crossh_core::git_status::GitStatus;

    use super::{
        ChangeKey, DiffState, GitOperation, GitSession, OperationState, selected_index,
        should_refresh_diff,
    };

    fn change(path: &str, staged: bool) -> FileChange {
        FileChange {
            path: path.to_string(),
            orig_path: None,
            status: ChangeStatus::Modified,
            staged,
            insertions: 1,
            deletions: 0,
        }
    }

    #[test]
    fn first_refresh_request_captures_repository_and_generation() {
        let mut session = GitSession::new(PathBuf::from("/tmp/repository"));

        let request = session
            .begin_refresh(false)
            .expect("the first refresh should start");

        assert_eq!(request.generation, 1);
        assert_eq!(request.cwd, PathBuf::from("/tmp/repository"));
        assert!(session.refresh.in_flight());
        assert!(session.initial_loading);
    }

    #[test]
    fn overlapping_refresh_requests_are_coalesced() {
        let mut session = GitSession::new(PathBuf::from("/tmp/repository"));

        assert!(session.begin_refresh(false).is_some());
        assert!(session.begin_refresh(true).is_none());
        assert!(session.force_diff_refresh_pending);
    }

    #[test]
    fn scan_reconciles_selection_and_requests_the_selected_diff() {
        let mut session = GitSession::new(PathBuf::from("/tmp/repository"));
        let request = session.begin_refresh(false).unwrap();
        let completion = session.apply_scan(
            request,
            Ok(ChangeScan {
                changes: vec![change("src/main.rs", false)],
                status: Some(GitStatus {
                    branch: "main".to_string(),
                    ..Default::default()
                }),
            }),
        );

        assert!(completion.reload_diff);
        assert!(completion.state_changed);
        assert_eq!(
            session.selected,
            Some(ChangeKey {
                path: "src/main.rs".to_string(),
                staged: false,
            })
        );
        assert_eq!(
            session.status.as_ref().map(|status| status.branch.as_str()),
            Some("main")
        );
        assert_eq!(
            session.selected_keys(),
            vec![ChangeKey {
                path: "src/main.rs".to_string(),
                staged: false,
            }]
        );
    }

    #[test]
    fn diff_result_is_ignored_after_selection_generation_changes() {
        let mut session = GitSession::new(PathBuf::from("/tmp/repository"));
        session.changes = vec![change("a.rs", false), change("b.rs", false)];
        session.selected = Some(ChangeKey {
            path: "a.rs".to_string(),
            staged: false,
        });
        let request = session.begin_diff().unwrap();
        session.selected = Some(ChangeKey {
            path: "b.rs".to_string(),
            staged: false,
        });

        assert!(!session.apply_diff(request, Ok(Some(FileDiff::default()))));
    }

    #[test]
    fn successful_operation_updates_state_and_clears_commit_message() {
        let mut session = GitSession::new(PathBuf::from("/tmp/repository"));
        let request = session
            .begin_operation(GitOperation::Commit("message".to_string()), None, true)
            .unwrap();

        assert_eq!(session.operation, OperationState::Running);
        let completion = session.apply_operation(request, Ok(()));
        assert!(completion.accepted);
        assert!(completion.clear_message);
        assert_eq!(session.operation, OperationState::Idle);
    }

    #[test]
    fn successful_batch_stage_moves_the_selected_keys_to_the_index() {
        let mut session = GitSession::new(PathBuf::from("/tmp/repository"));
        session.changes = vec![change("a.rs", false), change("b.rs", false)];
        session.select_all();
        let request = session
            .begin_operation(
                GitOperation::Stage(vec!["a.rs".to_string(), "b.rs".to_string()]),
                Some(ChangeKey {
                    path: "a.rs".to_string(),
                    staged: true,
                }),
                false,
            )
            .unwrap();

        let completion = session.apply_operation(request, Ok(()));
        assert!(completion.accepted);

        assert_eq!(
            session.selected_changes,
            [
                ChangeKey {
                    path: "a.rs".to_string(),
                    staged: true,
                },
                ChangeKey {
                    path: "b.rs".to_string(),
                    staged: true,
                },
            ]
            .into_iter()
            .collect()
        );
        assert_eq!(
            session.selected,
            Some(ChangeKey {
                path: "a.rs".to_string(),
                staged: true,
            })
        );

        let refresh = session.begin_refresh(false).unwrap();
        session.apply_scan(
            refresh,
            Ok(ChangeScan {
                changes: vec![change("a.rs", true), change("b.rs", true)],
                status: None,
            }),
        );
        assert_eq!(
            session.selected_keys(),
            vec![
                ChangeKey {
                    path: "a.rs".to_string(),
                    staged: true,
                },
                ChangeKey {
                    path: "b.rs".to_string(),
                    staged: true,
                },
            ]
        );
    }

    #[test]
    fn successful_hunk_stage_moves_the_selected_file_to_the_index() {
        let mut session = GitSession::new(PathBuf::from("/tmp/repository"));
        let entry = change("a.rs", false);
        session.changes = vec![entry.clone()];
        session.select(
            ChangeKey {
                path: "a.rs".to_string(),
                staged: false,
            },
            false,
            false,
        );

        let request = session
            .begin_operation(
                GitOperation::StageHunk {
                    entry,
                    hunk_index: 1,
                },
                Some(ChangeKey {
                    path: "a.rs".to_string(),
                    staged: true,
                }),
                false,
            )
            .unwrap();
        let completion = session.apply_operation(request, Ok(()));

        assert!(completion.accepted);
        assert_eq!(
            session.selected,
            Some(ChangeKey {
                path: "a.rs".to_string(),
                staged: true,
            })
        );
        assert_eq!(
            session.selected_changes,
            [ChangeKey {
                path: "a.rs".to_string(),
                staged: true,
            }]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn selection_helpers_keep_staged_and_working_entries_distinct() {
        let changes = vec![change("same.rs", true), change("same.rs", false)];
        let selected = ChangeKey {
            path: "same.rs".to_string(),
            staged: false,
        };

        assert_eq!(selected_index(&changes, Some(&selected)), Some(1));
        assert!(!should_refresh_diff(
            false,
            &changes,
            &changes,
            Some(&selected),
            Some(&selected),
        ));
        assert!(should_refresh_diff(
            true,
            &changes,
            &changes,
            Some(&selected),
            Some(&selected),
        ));
    }

    #[test]
    fn selection_supports_replacement_toggle_and_range_selection() {
        let mut session = GitSession::new(PathBuf::from("/tmp/repository"));
        session.changes = vec![
            change("a.rs", false),
            change("b.rs", false),
            change("c.rs", false),
            change("d.rs", true),
        ];

        let a = ChangeKey {
            path: "a.rs".to_string(),
            staged: false,
        };
        let b = ChangeKey {
            path: "b.rs".to_string(),
            staged: false,
        };
        let c = ChangeKey {
            path: "c.rs".to_string(),
            staged: false,
        };
        let d = ChangeKey {
            path: "d.rs".to_string(),
            staged: true,
        };

        session.select(a.clone(), false, false);
        session.select(c.clone(), true, false);
        assert_eq!(session.selected_keys(), vec![a.clone(), c.clone()]);
        assert_eq!(session.selected.as_ref(), Some(&c));

        session.select(a.clone(), true, false);
        assert_eq!(session.selected_keys(), vec![c.clone()]);

        session.select(d.clone(), false, true);
        assert_eq!(session.selected_keys(), vec![c.clone(), d.clone()]);
        assert_eq!(session.selected.as_ref(), Some(&d));

        session.select(b, false, false);
        assert_eq!(
            session.selected_keys(),
            vec![ChangeKey {
                path: "b.rs".to_string(),
                staged: false,
            }]
        );
    }

    #[test]
    fn selected_paths_are_grouped_by_stage_state_without_duplicates() {
        let mut session = GitSession::new(PathBuf::from("/tmp/repository"));
        session.changes = vec![
            change("same.rs", true),
            change("same.rs", false),
            change("other.rs", false),
        ];
        session.select_all();

        assert_eq!(session.selected_paths(true), vec!["same.rs".to_string()]);
        assert_eq!(
            session.selected_paths(false),
            vec!["other.rs".to_string(), "same.rs".to_string()]
        );
        assert_eq!(session.selected_count(), 3);
    }

    #[test]
    fn diff_state_accepts_typed_lines_without_ui_data() {
        let key = ChangeKey {
            path: "a.rs".to_string(),
            staged: false,
        };
        let mut session = GitSession::new(PathBuf::from("/tmp/repository"));
        session.changes = vec![change("a.rs", false)];
        session.selected = Some(key.clone());
        let request = session.begin_diff().unwrap();
        let diff = FileDiff {
            lines: vec![DiffLine {
                kind: DiffLineKind::Context,
                hunk_index: None,
                old_ln: Some(1),
                new_ln: Some(1),
                text: "line".to_string(),
            }],
            ..Default::default()
        };

        assert!(session.apply_diff(request, Ok(Some(diff))));
        assert!(matches!(session.diff, DiffState::Ready(ready, Some(_)) if ready == key));
    }
}

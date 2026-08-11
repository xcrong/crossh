use crossh_core::git::{FileChange, FileDiff};
use gpui::{Pixels, px};

pub const GIT_COMPACT_WIDTH: f32 = 840.;
pub const CHANGES_PANE_DEFAULT_WIDTH: f32 = 300.;
pub const CHANGES_PANE_MIN_WIDTH: f32 = 220.;
pub const CHANGES_PANE_MAX_WIDTH: f32 = 420.;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ChangeKey {
    pub path: String,
    pub staged: bool,
}

impl From<&FileChange> for ChangeKey {
    fn from(change: &FileChange) -> Self {
        Self {
            path: change.path.clone(),
            staged: change.staged,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CompactPage {
    #[default]
    Changes,
    Diff,
}

#[derive(Clone, Debug)]
pub enum DiffState {
    Idle,
    Loading(ChangeKey),
    Ready(ChangeKey, Option<FileDiff>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum OperationState {
    #[default]
    Idle,
    Running,
    Error(String),
}

pub fn uses_compact_git_layout(width: Pixels) -> bool {
    width < px(GIT_COMPACT_WIDTH)
}

pub fn clamp_changes_pane_width(width: f32) -> f32 {
    width.clamp(CHANGES_PANE_MIN_WIDTH, CHANGES_PANE_MAX_WIDTH)
}

pub fn selected_index(changes: &[FileChange], selected: Option<&ChangeKey>) -> Option<usize> {
    selected.and_then(|selected| {
        changes
            .iter()
            .position(|change| ChangeKey::from(change) == *selected)
    })
}

pub fn reconcile_selection(
    changes: &[FileChange],
    selected: Option<&ChangeKey>,
    previous_index: Option<usize>,
) -> Option<ChangeKey> {
    if let Some(index) = selected_index(changes, selected) {
        return Some(ChangeKey::from(&changes[index]));
    }
    let last = changes.len().checked_sub(1)?;
    let index = previous_index.unwrap_or(0).min(last);
    Some(ChangeKey::from(&changes[index]))
}

pub fn diff_uses_staged_baseline(change: &FileChange) -> bool {
    change.staged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossh_core::git::ChangeStatus;

    fn change(path: &str, staged: bool) -> FileChange {
        FileChange {
            path: path.to_string(),
            orig_path: None,
            status: ChangeStatus::Modified,
            staged,
            insertions: 0,
            deletions: 0,
        }
    }

    #[test]
    fn git_layout_switches_at_compact_width() {
        assert!(uses_compact_git_layout(px(839.)));
        assert!(!uses_compact_git_layout(px(840.)));
    }

    #[test]
    fn changes_pane_width_is_clamped() {
        assert_eq!(clamp_changes_pane_width(100.), CHANGES_PANE_MIN_WIDTH);
        assert_eq!(clamp_changes_pane_width(320.), 320.);
        assert_eq!(clamp_changes_pane_width(600.), CHANGES_PANE_MAX_WIDTH);
    }

    #[test]
    fn selection_survives_reordering_by_stable_key() {
        let selected = ChangeKey {
            path: "b.rs".to_string(),
            staged: false,
        };
        let changes = vec![change("a.rs", false), change("b.rs", false)];
        assert_eq!(
            reconcile_selection(&changes, Some(&selected), Some(0)),
            Some(selected)
        );
    }

    #[test]
    fn missing_selection_moves_to_nearest_previous_index() {
        let selected = ChangeKey {
            path: "gone.rs".to_string(),
            staged: false,
        };
        let changes = vec![change("a.rs", false), change("c.rs", false)];
        assert_eq!(
            reconcile_selection(&changes, Some(&selected), Some(1)),
            Some(ChangeKey::from(&changes[1]))
        );
    }

    #[test]
    fn staged_and_working_entries_choose_their_own_diff_baseline() {
        assert!(diff_uses_staged_baseline(&change("both.rs", true)));
        assert!(!diff_uses_staged_baseline(&change("both.rs", false)));
    }
}

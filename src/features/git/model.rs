//! Git Viewer 的纯布局状态。

pub const GIT_COMPACT_WIDTH: f32 = 840.;
pub const CHANGES_PANE_DEFAULT_WIDTH: f32 = 300.;
pub const CHANGES_PANE_MIN_WIDTH: f32 = 220.;
pub const CHANGES_PANE_MAX_WIDTH: f32 = 420.;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CompactPage {
    #[default]
    Changes,
    Diff,
    History,
    HistoryDetail,
    Branches,
    Stashes,
}

pub fn uses_compact_git_layout(width: f32) -> bool {
    width < GIT_COMPACT_WIDTH
}

pub fn clamp_changes_pane_width(width: f32) -> f32 {
    width.clamp(CHANGES_PANE_MIN_WIDTH, CHANGES_PANE_MAX_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_layout_switches_at_compact_width() {
        assert!(uses_compact_git_layout(839.));
        assert!(!uses_compact_git_layout(840.));
    }

    #[test]
    fn changes_pane_width_is_clamped() {
        assert_eq!(clamp_changes_pane_width(100.), CHANGES_PANE_MIN_WIDTH);
        assert_eq!(clamp_changes_pane_width(320.), 320.);
        assert_eq!(clamp_changes_pane_width(600.), CHANGES_PANE_MAX_WIDTH);
    }
}

//! Workspace-owned session registry.
//!
//! This state is deliberately independent from rendering. The shell coordinates
//! actions, while the registry owns the collections that describe open panes.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{Subscription, Task};

use super::state::{ActiveView, LocalDir, LocalSession, LocalSessionId};
use super::toaster::ToasterState;
use crate::shared::text_editing::TextEditingState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SplitSide {
    Left,
    Right,
    BottomLeft,
    BottomRight,
}

impl SplitSide {
    #[allow(dead_code)]
    pub(crate) fn is_left_column(self) -> bool {
        matches!(self, SplitSide::Left | SplitSide::BottomLeft)
    }

    pub(crate) fn is_right_column(self) -> bool {
        matches!(self, SplitSide::Right | SplitSide::BottomRight)
    }

    #[allow(dead_code)]
    pub(crate) fn is_top(self) -> bool {
        matches!(self, SplitSide::Left | SplitSide::Right)
    }

    #[allow(dead_code)]
    pub(crate) fn is_bottom(self) -> bool {
        matches!(self, SplitSide::BottomLeft | SplitSide::BottomRight)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SplitViewCloseOutcome {
    /// 视图不在分栏中。
    Inactive,
    /// 分栏已随视图关闭；`retire_pane` 是需要上层退休处理的右窗格
    /// （仅当分栏隐藏——属主不是活动视图——时关闭属主出现）。
    Closed { retire_pane: Option<ActiveView> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerminalSplitState {
    pub(crate) left: ActiveView,
    pub(crate) right: Option<ActiveView>,
    pub(crate) bottom_left: Option<ActiveView>,
    pub(crate) bottom_right: Option<ActiveView>,
    pub(crate) focused: SplitSide,
}

impl TerminalSplitState {
    pub(crate) fn new(left: ActiveView, right: ActiveView) -> Self {
        Self {
            left,
            right: Some(right),
            bottom_left: None,
            bottom_right: None,
            focused: SplitSide::Right,
        }
    }

    pub(crate) fn focus(&mut self, side: SplitSide) {
        // 仅在目标窗格存在时切换焦点，避免聚焦到空槽位。
        let can_focus = match side {
            SplitSide::Left => true,
            SplitSide::Right => self.right.is_some(),
            SplitSide::BottomLeft => self.bottom_left.is_some(),
            SplitSide::BottomRight => self.bottom_right.is_some(),
        };
        if can_focus {
            self.focused = side;
        }
    }

    pub(crate) fn focused_view(self) -> ActiveView {
        match self.focused {
            SplitSide::Left => self.left,
            SplitSide::Right => self.right.unwrap_or(self.left),
            SplitSide::BottomLeft => self.bottom_left.unwrap_or(self.left),
            SplitSide::BottomRight => self.bottom_right.or(self.right).unwrap_or(self.left),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn pane_for(&self, side: SplitSide) -> Option<ActiveView> {
        match side {
            SplitSide::Left => Some(self.left),
            SplitSide::Right => self.right,
            SplitSide::BottomLeft => self.bottom_left,
            SplitSide::BottomRight => self.bottom_right,
        }
    }

    pub(crate) fn contains(&self, view: ActiveView) -> bool {
        self.left == view
            || self.right == Some(view)
            || self.bottom_left == Some(view)
            || self.bottom_right == Some(view)
    }

    pub(crate) fn secondary_views(&self) -> Vec<ActiveView> {
        let mut out = Vec::new();
        if let Some(v) = self.right {
            out.push(v);
        }
        if let Some(v) = self.bottom_left {
            out.push(v);
        }
        if let Some(v) = self.bottom_right {
            out.push(v);
        }
        out
    }

    pub(crate) fn has_any_secondary(&self) -> bool {
        self.right.is_some() || self.bottom_left.is_some() || self.bottom_right.is_some()
    }

    pub(crate) fn is_secondary(&self, view: ActiveView) -> bool {
        self.right == Some(view)
            || self.bottom_left == Some(view)
            || self.bottom_right == Some(view)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ComposeEntry {
    pub visible: bool,
    pub state: TextEditingState,
    pub last_visible: bool,
}

impl Default for ComposeEntry {
    fn default() -> Self {
        Self {
            visible: false,
            state: TextEditingState::new(String::new()),
            last_visible: false,
        }
    }
}

pub(crate) struct SessionRegistry {
    pub(crate) local_sessions: BTreeMap<LocalSessionId, LocalSession>,
    pub(crate) local_dirs: BTreeMap<PathBuf, LocalDir>,
    pub(crate) next_local_session_id: LocalSessionId,
    /// GPUI subscriptions are owned by the workspace for the lifetime of its sessions.
    pub(crate) terminal_subscriptions: Vec<Subscription>,
}

impl SessionRegistry {
    pub(crate) fn new(local_dirs: BTreeMap<PathBuf, LocalDir>) -> Self {
        Self {
            local_sessions: BTreeMap::new(),
            local_dirs,
            next_local_session_id: 1,
            terminal_subscriptions: Vec::new(),
        }
    }

    pub(crate) fn allocate_local_session_id(&mut self) -> LocalSessionId {
        let id = self.next_local_session_id;
        self.next_local_session_id += 1;
        id
    }
}

pub(crate) struct WorkspaceState {
    pub(crate) sessions: SessionRegistry,
    pub(crate) active_view: Option<ActiveView>,
    pub(crate) toaster: ToasterState,
    pub(crate) _toast_task: Option<Task<()>>,
    pub(crate) terminal_splits: BTreeMap<ActiveView, TerminalSplitState>,
    pub(crate) compose: BTreeMap<ActiveView, ComposeEntry>,
    pub(crate) split_widths: BTreeMap<ActiveView, Rc<Cell<f32>>>,
    pub(crate) split_heights: BTreeMap<ActiveView, Rc<Cell<f32>>>,
    pub(crate) split_heights_right: BTreeMap<ActiveView, Rc<Cell<f32>>>,
}

impl WorkspaceState {
    /// 统一移除分栏及对应的宽度/高度槽位，避免多处 map 不同步。
    fn remove_split_entry(&mut self, owner: &ActiveView) -> Option<TerminalSplitState> {
        let split = self.terminal_splits.remove(owner)?;
        self.split_widths.remove(owner);
        self.split_heights.remove(owner);
        self.split_heights_right.remove(owner);
        Some(split)
    }

    fn retain_splits_not_involving(
        &mut self,
        closed: &[ActiveView],
        removed: &mut Vec<TerminalSplitState>,
    ) {
        self.terminal_splits.retain(|owner, split| {
            let involved = closed.iter().any(|v| split.contains(*v));
            if involved {
                removed.push(*split);
                self.split_widths.remove(owner);
                self.split_heights.remove(owner);
                self.split_heights_right.remove(owner);
                false
            } else {
                true
            }
        });
    }

    pub(crate) fn new(local_dirs: BTreeMap<PathBuf, LocalDir>) -> Self {
        Self {
            sessions: SessionRegistry::new(local_dirs),
            active_view: None,
            toaster: ToasterState::default(),
            _toast_task: None,
            terminal_splits: BTreeMap::new(),
            compose: BTreeMap::new(),
            split_widths: BTreeMap::new(),
            split_heights: BTreeMap::new(),
            split_heights_right: BTreeMap::new(),
        }
    }

    /// 为当前活动视图创建水平分栏（左右）。若已存在垂直分栏则在其基础上扩展。
    pub(crate) fn begin_terminal_split(&mut self, right: ActiveView) -> bool {
        let Some(left) = self.active_view else {
            return false;
        };
        if left == right
            || self.is_split_secondary(right)
            || self.terminal_splits.contains_key(&right)
        {
            return false;
        }
        if let Some(existing) = self.terminal_splits.get_mut(&left) {
            if existing.right.is_some() {
                return false;
            }
            if existing.contains(right) {
                return false;
            }
            existing.right = Some(right);
            existing.focused = SplitSide::Right;
            self.split_widths
                .entry(left)
                .or_insert_with(|| Rc::new(Cell::new(0.)));
            self.split_heights
                .entry(left)
                .or_insert_with(|| Rc::new(Cell::new(0.)));
            self.split_heights_right
                .entry(left)
                .or_insert_with(|| Rc::new(Cell::new(0.)));
            return true;
        }
        self.terminal_splits
            .insert(left, TerminalSplitState::new(left, right));
        // 宽度/高度槽位随分栏创建：0.0 哨兵 = 渲染层走均分默认。
        self.split_widths.insert(left, Rc::new(Cell::new(0.)));
        self.split_heights.insert(left, Rc::new(Cell::new(0.)));
        self.split_heights_right
            .insert(left, Rc::new(Cell::new(0.)));
        true
    }

    pub(crate) fn active_split(&self) -> Option<TerminalSplitState> {
        self.active_view
            .and_then(|owner| self.terminal_splits.get(&owner).copied())
    }

    pub(crate) fn split_of(&self, owner: ActiveView) -> Option<TerminalSplitState> {
        self.terminal_splits.get(&owner).copied()
    }

    pub(crate) fn focused_view(&self) -> Option<ActiveView> {
        // 分栏跟随其属主（split.left 即创建时的活动视图）：只有属主正在展示时，分栏的聚焦侧才算聚焦；否则焦点就是活动视图。
        match self.active_split() {
            Some(split) if Some(split.left) == self.active_view => Some(split.focused_view()),
            _ => self.active_view,
        }
    }

    /// 视图是否正在充当某个分栏的 secondary 窗格（标签栏跳过渲染，包括右/下）。
    pub(crate) fn is_split_secondary(&self, view: ActiveView) -> bool {
        self.terminal_splits
            .values()
            .any(|split| split.is_secondary(view))
    }

    pub(crate) fn focus_terminal_split(&mut self, side: SplitSide) -> bool {
        let Some(owner) = self.active_view else {
            return false;
        };
        if let Some(split) = self.terminal_splits.get_mut(&owner) {
            let before = split.focused;
            split.focus(side);
            return split.focused != before;
        }
        false
    }

    pub(crate) fn focus_split_view(&mut self, view: ActiveView) -> bool {
        let Some(owner) = self.active_view else {
            return false;
        };
        let Some(split) = self.terminal_splits.get_mut(&owner) else {
            return false;
        };
        if Some(split.left) != self.active_view {
            return false;
        }
        let side = if split.left == view {
            SplitSide::Left
        } else if split.right == Some(view) {
            SplitSide::Right
        } else if split.bottom_left == Some(view) {
            SplitSide::BottomLeft
        } else if split.bottom_right == Some(view) {
            SplitSide::BottomRight
        } else {
            return false;
        };
        let before = split.focused;
        split.focus(side);
        split.focused != before
    }

    /// 视图参与的任意分栏（属主或任一 secondary）。
    pub(crate) fn split_containing(&self, view: ActiveView) -> Option<TerminalSplitState> {
        self.terminal_splits.get(&view).copied().or_else(|| {
            self.split_owner_of(view)
                .and_then(|owner| self.split_of(owner))
        })
    }

    /// 批量清扫前拆掉与 `closed` 中任一视图相关的分栏（属主或任一 secondary）。
    pub(crate) fn take_splits_involving(
        &mut self,
        closed: &[ActiveView],
    ) -> Vec<TerminalSplitState> {
        let mut removed = Vec::new();
        self.retain_splits_not_involving(closed, &mut removed);
        removed
    }

    fn split_owner_of(&self, view: ActiveView) -> Option<ActiveView> {
        self.terminal_splits.iter().find_map(|(&owner, split)| {
            split.contains(view).then_some(owner).filter(|o| *o != view)
        })
    }

    /// 找到右窗格/下窗格是 `view` 的分栏所属主（兼容旧名）。
    #[allow(dead_code)]
    fn split_owner_of_right(&self, view: ActiveView) -> Option<ActiveView> {
        self.split_owner_of(view)
    }

    /// 为当前活动视图创建或扩展水平分栏（左右）。若已存在垂直分栏则在其基础上扩展为 2/3 格。
    #[allow(dead_code)]
    pub(crate) fn begin_horizontal_split(&mut self, right: ActiveView) -> bool {
        self.begin_terminal_split(right)
    }

    /// 尝试为当前活动视图的聚焦列创建垂直分栏（上下），最多四格。
    pub(crate) fn begin_vertical_split(&mut self, bottom: ActiveView) -> bool {
        let Some(owner) = self.active_view else {
            return false;
        };
        if owner == bottom
            || self.is_split_secondary(bottom)
            || self.terminal_splits.contains_key(&bottom)
        {
            return false;
        }
        if let Some(split) = self.terminal_splits.get_mut(&owner) {
            // 已有分栏：根据当前聚焦列决定目标格
            let target_is_right = split.focused.is_right_column() && split.right.is_some();
            if target_is_right {
                if split.bottom_right.is_some() {
                    return false;
                }
                // 防重
                if split.contains(bottom) {
                    return false;
                }
                split.bottom_right = Some(bottom);
                split.focused = SplitSide::BottomRight;
            } else {
                if split.bottom_left.is_some() {
                    return false;
                }
                if split.contains(bottom) {
                    return false;
                }
                split.bottom_left = Some(bottom);
                split.focused = SplitSide::BottomLeft;
            }
            // 已有宽度槽位，无需新建；确保高度槽位存在
            self.split_heights
                .entry(owner)
                .or_insert_with(|| Rc::new(Cell::new(0.)));
            self.split_heights_right
                .entry(owner)
                .or_insert_with(|| Rc::new(Cell::new(0.)));
            true
        } else {
            // 尚无分栏：创建单列垂直分栏
            let state = TerminalSplitState {
                left: owner,
                right: None,
                bottom_left: Some(bottom),
                bottom_right: None,
                focused: SplitSide::BottomLeft,
            };
            self.terminal_splits.insert(owner, state);
            self.split_widths.insert(owner, Rc::new(Cell::new(0.)));
            self.split_heights.insert(owner, Rc::new(Cell::new(0.)));
            self.split_heights_right
                .insert(owner, Rc::new(Cell::new(0.)));
            true
        }
    }

    #[allow(dead_code)]
    pub(crate) fn has_vertical_split(&self) -> bool {
        self.active_split()
            .is_some_and(|s| s.bottom_left.is_some() || s.bottom_right.is_some())
    }

    #[allow(dead_code)]
    pub(crate) fn can_add_horizontal(&self) -> bool {
        if let Some(split) = self.active_split() {
            split.right.is_none()
        } else {
            self.active_view.is_some()
        }
    }

    pub(crate) fn can_add_vertical(&self) -> bool {
        if let Some(split) = self.active_split() {
            let col_is_right = split.focused.is_right_column() && split.right.is_some();
            if col_is_right {
                split.bottom_right.is_none()
            } else {
                split.bottom_left.is_none()
            }
        } else {
            self.active_view.is_some()
        }
    }

    /// 分栏视图（属主或任一 secondary）被关闭前的状态处理，支持 2x2。
    pub(crate) fn prepare_split_view_close(&mut self, view: ActiveView) -> SplitViewCloseOutcome {
        // 关闭属主
        if let Some(split) = self.terminal_splits.get(&view).copied() {
            // 先移除属主条目
            self.remove_split_entry(&view);
            return self.prepare_owner_close(split, view);
        }
        // 关闭 secondary：找到所属 owner
        let Some(owner) = self.split_owner_of(view) else {
            return SplitViewCloseOutcome::Inactive;
        };
        // 借助可变借用处理二次 pane 的移除
        let outcome = {
            let Some(split) = self.terminal_splits.get_mut(&owner) else {
                return SplitViewCloseOutcome::Inactive;
            };
            // 识别被关闭的格
            let mut removed_side: Option<SplitSide> = None;
            if split.right == Some(view) {
                removed_side = Some(SplitSide::Right);
                // 若右列还有下格，将其上移至右格
                if let Some(br) = split.bottom_right.take() {
                    split.right = Some(br);
                    // 晋升后右列单格，重置右列高度为等分哨兵
                    if let Some(cell) = self.split_heights_right.get(&owner) {
                        cell.set(0.0);
                    }
                    // 聚焦调整：若原聚焦在 Right 或 BottomRight，聚焦到新的 Right
                    if matches!(split.focused, SplitSide::Right | SplitSide::BottomRight) {
                        split.focused = SplitSide::Right;
                    }
                } else {
                    split.right = None;
                    if matches!(split.focused, SplitSide::Right | SplitSide::BottomRight) {
                        split.focused = SplitSide::Left;
                    }
                    // 右列消失，重置右列高度哨兵
                    if let Some(cell) = self.split_heights_right.get(&owner) {
                        cell.set(0.0);
                    }
                }
            } else if split.bottom_left == Some(view) {
                removed_side = Some(SplitSide::BottomLeft);
                split.bottom_left = None;
                if split.focused == SplitSide::BottomLeft {
                    split.focused = SplitSide::Left;
                }
                // 左列恢复单格，重置左列高度
                if let Some(cell) = self.split_heights.get(&owner) {
                    cell.set(0.0);
                }
            } else if split.bottom_right == Some(view) {
                removed_side = Some(SplitSide::BottomRight);
                split.bottom_right = None;
                if split.focused == SplitSide::BottomRight {
                    // 若右列仍有上格，聚焦回 Right，否则 Left
                    if split.right.is_some() {
                        split.focused = SplitSide::Right;
                    } else {
                        split.focused = SplitSide::Left;
                    }
                }
                // 右列恢复单格或保持单列，重置右列高度
                if let Some(cell) = self.split_heights_right.get(&owner) {
                    cell.set(0.0);
                }
            }
            if removed_side.is_none() {
                return SplitViewCloseOutcome::Inactive;
            }
            // 若移除后无任何 secondary，清理整个分栏
            if !split.has_any_secondary() {
                // 需要在外部移除条目，这里标记
                true
            } else {
                false
            }
        };
        if outcome {
            // 刚才的块已判断无 secondary，需移除整个分栏
            self.remove_split_entry(&owner);
        }
        SplitViewCloseOutcome::Closed { retire_pane: None }
    }

    fn prepare_owner_close(
        &mut self,
        split: TerminalSplitState,
        closing_view: ActiveView,
    ) -> SplitViewCloseOutcome {
        // 收集所有 secondary 用于退休判断
        let secondaries = split.secondary_views();
        // 若属主是活动视图，晋升首个 secondary 为新的活动视图，并清理分栏
        if self.active_view == Some(closing_view) {
            // 优先顺序：右上 -> 左下 -> 右下
            let next = split.right.or(split.bottom_left).or(split.bottom_right);
            if let Some(next_view) = next {
                self.active_view = Some(next_view);
            } else {
                self.active_view = None;
            }
            // 属主关闭时，其余 secondary 若有活动风险，上层会按退休逻辑处理；
            // 这里仅返回基础关闭，退休由 AppShell.retire_split_pane 处理。
            // 为保持旧行为，隐藏态关闭才退休；活动态晋升不退休。
            SplitViewCloseOutcome::Closed { retire_pane: None }
        } else {
            // 隐藏态关闭属主：所有 secondary 需要退休（由上层遍历处理，这里返回首个）
            // 旧契约仅退休 right；新契约返回首个 secondary，其余由 take_splits 覆盖
            // 为兼容，返回第一个 secondary 作为 retire_pane，其余由调用方通过 take 逻辑处理
            let retire = secondaries.into_iter().next();
            // 若有其余 secondary，它们仍以独立标签形式残留需由 AppShell 决定
            SplitViewCloseOutcome::Closed {
                retire_pane: retire,
            }
        }
    }

    #[allow(dead_code)]
    fn prepare_owner_close_legacy(&mut self, split: TerminalSplitState) -> SplitViewCloseOutcome {
        if self.active_view == Some(split.left) {
            if let Some(next) = split.right {
                self.active_view = Some(next);
            } else if let Some(next) = split.bottom_left {
                self.active_view = Some(next);
            } else if let Some(next) = split.bottom_right {
                self.active_view = Some(next);
            }
            SplitViewCloseOutcome::Closed { retire_pane: None }
        } else {
            let retire = split.right.or(split.bottom_left).or(split.bottom_right);
            SplitViewCloseOutcome::Closed {
                retire_pane: retire,
            }
        }
    }

    pub(crate) fn compose_visible(&self, view: ActiveView) -> bool {
        self.compose.get(&view).is_some_and(|e| e.visible)
    }
    pub(crate) fn compose_state_for(&self, view: ActiveView) -> Option<&TextEditingState> {
        self.compose.get(&view).map(|e| &e.state)
    }

    /// 获取或创建指定终端的 compose 条目。
    pub(crate) fn compose_entry_mut(&mut self, view: ActiveView) -> &mut ComposeEntry {
        self.compose.entry(view).or_default()
    }

    pub(crate) fn compose_visible_for_focused(&self) -> bool {
        self.focused_view()
            .map(|v| self.compose_visible(v))
            .unwrap_or(false)
    }

    pub(crate) fn remove_compose_for_view(&mut self, view: ActiveView) {
        self.compose.remove(&view);
    }

    pub(crate) fn take_composes_involving(&mut self, closed: &[ActiveView]) -> Vec<ComposeEntry> {
        let mut removed = Vec::new();
        self.compose.retain(|view, entry| {
            if closed.contains(view) {
                removed.push(entry.clone());
                false
            } else {
                true
            }
        });
        removed
    }
}

#[cfg(test)]
mod tests {
    // spec 测试名带双下划线前缀（如 `spec_20260821_split_width__`），
    // 触发 non_snake_case 警告；测试模块内显式豁免。
    #![allow(non_snake_case)]

    use super::*;

    #[test]
    fn registry_ids_are_monotonic_and_state_starts_inactive() {
        let remembered = BTreeMap::from([(
            PathBuf::from("/workspace"),
            LocalDir {
                project_dir: PathBuf::from("/workspace"),
                sessions: Vec::new(),
                active_session: None,
            },
        )]);
        let mut workspace = WorkspaceState::new(remembered);

        assert_eq!(workspace.active_view, None);
        assert!(workspace.sessions.local_sessions.is_empty());
        assert_eq!(workspace.sessions.local_dirs.len(), 1);
        assert_eq!(workspace.sessions.allocate_local_session_id(), 1);
        assert_eq!(workspace.sessions.allocate_local_session_id(), 2);
        assert_eq!(workspace.sessions.allocate_local_session_id(), 3);
    }

    #[test]
    fn terminal_split_starts_on_the_right_and_tracks_focus() {
        let left = ActiveView::LocalSession(1);
        let right = ActiveView::LocalSession(2);
        let mut split = TerminalSplitState::new(left, right);

        assert_eq!(split.focused_view(), right);

        split.focus(SplitSide::Left);
        assert_eq!(split.focused_view(), left);
    }

    #[test]
    fn workspace_does_not_replace_an_existing_terminal_split() {
        let mut workspace = WorkspaceState::new(BTreeMap::new());
        workspace.active_view = Some(ActiveView::LocalSession(1));

        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(2)));
        assert!(!workspace.begin_terminal_split(ActiveView::LocalSession(3)));
        assert_eq!(
            workspace.active_split().unwrap().right,
            Some(ActiveView::LocalSession(2))
        );
    }

    #[test]
    fn closing_a_split_view_keeps_the_other_view_active() {
        let mut workspace = WorkspaceState::new(BTreeMap::new());
        workspace.active_view = Some(ActiveView::LocalSession(1));
        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(2)));

        assert_eq!(
            workspace.prepare_split_view_close(ActiveView::LocalSession(2)),
            SplitViewCloseOutcome::Closed { retire_pane: None }
        );
        assert_eq!(workspace.active_view, Some(ActiveView::LocalSession(1)));
        assert!(workspace.active_split().is_none());

        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(2)));
        assert_eq!(
            workspace.prepare_split_view_close(ActiveView::LocalSession(1)),
            SplitViewCloseOutcome::Closed { retire_pane: None }
        );
        assert_eq!(workspace.active_view, Some(ActiveView::LocalSession(2)));
        assert!(workspace.active_split().is_none());
    }

    #[test]
    fn closing_a_hidden_owner_tab_retires_the_pane_instead_of_taking_over() {
        let mut workspace = WorkspaceState::new(BTreeMap::new());
        workspace.active_view = Some(ActiveView::LocalSession(1));
        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(2)));
        workspace.active_view = Some(ActiveView::LocalSession(3));

        let outcome = workspace.prepare_split_view_close(ActiveView::LocalSession(1));
        assert_eq!(
            outcome,
            SplitViewCloseOutcome::Closed {
                retire_pane: Some(ActiveView::LocalSession(2))
            }
        );
        assert_eq!(workspace.active_view, Some(ActiveView::LocalSession(3)));
        assert!(workspace.terminal_splits.is_empty());
        assert!(!workspace.is_split_secondary(ActiveView::LocalSession(2)));
    }

    #[test]
    fn split_focus_and_focus_switch_only_count_while_the_owner_tab_is_active() {
        let mut workspace = WorkspaceState::new(BTreeMap::new());
        workspace.active_view = Some(ActiveView::LocalSession(1));
        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(2)));
        assert_eq!(workspace.focused_view(), Some(ActiveView::LocalSession(2)));

        workspace.active_view = Some(ActiveView::LocalSession(3));
        assert_eq!(workspace.focused_view(), Some(ActiveView::LocalSession(3)));
        assert!(!workspace.focus_split_view(ActiveView::LocalSession(2)));

        workspace.active_view = Some(ActiveView::LocalSession(1));
        assert_eq!(workspace.focused_view(), Some(ActiveView::LocalSession(2)));
        assert!(workspace.focus_split_view(ActiveView::LocalSession(1)));
        assert_eq!(workspace.focused_view(), Some(ActiveView::LocalSession(1)));

        match workspace.prepare_split_view_close(ActiveView::LocalSession(3)) {
            SplitViewCloseOutcome::Inactive => {}
            _ => panic!("unrelated view close must not affect the split"),
        }
    }

    /// 批量清扫：拆掉与被清扫视图（属主或右窗格）相关的分栏，不做退休。
    #[test]
    fn batch_sweep_detaches_splits_involving_closed_views() {
        let mut workspace = WorkspaceState::new(BTreeMap::new());
        workspace.active_view = Some(ActiveView::LocalSession(1));
        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(2)));
        workspace.active_view = Some(ActiveView::LocalSession(3));
        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(4)));

        // 清扫 2 和 3：涉及它们的两个分栏都被拆掉，且不产生退休结果
        let removed = workspace
            .take_splits_involving(&[ActiveView::LocalSession(2), ActiveView::LocalSession(3)]);
        assert_eq!(removed.len(), 2);
        assert!(workspace.terminal_splits.is_empty());
        assert!(!workspace.is_split_secondary(ActiveView::LocalSession(2)));
        assert!(!workspace.is_split_secondary(ActiveView::LocalSession(4)));
        assert_eq!(workspace.active_view, Some(ActiveView::LocalSession(3)));
    }

    /// 批量清扫保留侧：keep 的分栏右窗格也在清扫名单中时，分栏一并拆除。
    #[test]
    fn batch_sweep_also_detaches_keep_owner_split_when_right_is_swept() {
        let mut workspace = WorkspaceState::new(BTreeMap::new());
        workspace.active_view = Some(ActiveView::LocalSession(1));
        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(2)));

        // keep 是 1，右窗格 2 被清扫：1 的分栏必须拆除（右窗格没了）
        let removed = workspace.take_splits_involving(&[ActiveView::LocalSession(2)]);
        assert_eq!(removed.len(), 1);
        assert!(!workspace.is_split_secondary(ActiveView::LocalSession(2)));
    }

    /// 契约 1：两个属主的分栏宽度互不覆盖。
    #[test]
    fn spec_20260821_split_width__owners_keep_independent_widths() {
        let mut workspace = WorkspaceState::new(BTreeMap::new());
        workspace.active_view = Some(ActiveView::LocalSession(1));
        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(2)));
        workspace.active_view = Some(ActiveView::LocalSession(3));
        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(4)));

        workspace.split_widths[&ActiveView::LocalSession(1)].set(220.);
        workspace.split_widths[&ActiveView::LocalSession(3)].set(540.);

        assert_eq!(
            workspace.split_widths[&ActiveView::LocalSession(1)].get(),
            220.
        );
        assert_eq!(
            workspace.split_widths[&ActiveView::LocalSession(3)].get(),
            540.
        );
    }

    /// 契约 2：从未拖拽过的分栏读到 0.0 哨兵（渲染层以均分为默认值）。
    #[test]
    fn spec_20260821_split_width__fresh_slot_reads_zero_sentinel() {
        let mut workspace = WorkspaceState::new(BTreeMap::new());
        workspace.active_view = Some(ActiveView::LocalSession(1));
        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(2)));

        assert_eq!(
            workspace.split_widths[&ActiveView::LocalSession(1)].get(),
            0.
        );
    }

    /// 契约 3：只有创建成功才分配宽度槽位；失败早退不分配、不覆盖。
    #[test]
    fn spec_20260821_split_width__slot_allocated_only_on_successful_creation() {
        let mut workspace = WorkspaceState::new(BTreeMap::new());
        // 无活动视图：创建失败，不分配槽位
        assert!(!workspace.begin_terminal_split(ActiveView::LocalSession(2)));
        assert!(
            !workspace
                .split_widths
                .contains_key(&ActiveView::LocalSession(1))
        );

        workspace.active_view = Some(ActiveView::LocalSession(1));
        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(2)));
        assert!(
            workspace
                .split_widths
                .contains_key(&ActiveView::LocalSession(1))
        );

        // 属主已有分栏：再次创建失败，不重置已有槽位
        workspace.split_widths[&ActiveView::LocalSession(1)].set(300.);
        assert!(!workspace.begin_terminal_split(ActiveView::LocalSession(3)));
        assert_eq!(
            workspace.split_widths[&ActiveView::LocalSession(1)].get(),
            300.
        );
    }

    /// 契约 4：关闭属主/右窗格、批量清扫三条拆除路径都同步移除宽度槽位。
    #[test]
    fn spec_20260821_split_width__all_close_paths_remove_their_slots() {
        let mut workspace = WorkspaceState::new(BTreeMap::new());
        workspace.active_view = Some(ActiveView::LocalSession(1));
        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(2)));
        workspace.active_view = Some(ActiveView::LocalSession(3));
        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(4)));
        workspace.active_view = Some(ActiveView::LocalSession(5));
        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(6)));

        // 路径一：关闭右窗格（会话 2），属主 1 的槽位移除
        assert_eq!(
            workspace.prepare_split_view_close(ActiveView::LocalSession(2)),
            SplitViewCloseOutcome::Closed { retire_pane: None }
        );
        assert!(
            !workspace
                .split_widths
                .contains_key(&ActiveView::LocalSession(1))
        );

        // 路径二：隐藏状态下关闭属主 Tab（会话 3），其槽位移除
        workspace.active_view = Some(ActiveView::LocalSession(7));
        assert_eq!(
            workspace.prepare_split_view_close(ActiveView::LocalSession(3)),
            SplitViewCloseOutcome::Closed {
                retire_pane: Some(ActiveView::LocalSession(4))
            }
        );
        assert!(
            !workspace
                .split_widths
                .contains_key(&ActiveView::LocalSession(3))
        );

        // 路径三：批量清扫命中属主（会话 5），其槽位移除
        let removed = workspace.take_splits_involving(&[ActiveView::LocalSession(5)]);
        assert_eq!(removed.len(), 1);
        assert!(
            !workspace
                .split_widths
                .contains_key(&ActiveView::LocalSession(5))
        );
        assert!(workspace.split_widths.is_empty());
    }

    /// 契约 6：全部分栏清空后 split_widths 同步为空，不留孤儿条目。
    #[test]
    fn spec_20260821_split_width__clearing_all_splits_empties_width_slots() {
        let mut workspace = WorkspaceState::new(BTreeMap::new());
        workspace.active_view = Some(ActiveView::LocalSession(1));
        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(2)));
        workspace.active_view = Some(ActiveView::LocalSession(3));
        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(4)));
        workspace.split_widths[&ActiveView::LocalSession(1)].set(260.);
        workspace.split_widths[&ActiveView::LocalSession(3)].set(480.);

        let removed = workspace
            .take_splits_involving(&[ActiveView::LocalSession(1), ActiveView::LocalSession(3)]);
        assert_eq!(removed.len(), 2);
        assert!(workspace.terminal_splits.is_empty());
        assert!(workspace.split_widths.is_empty());
    }

    /// 多分栏共存时，关闭某个分栏的右窗格只清空其属主的分栏，
    /// 其他属主的分栏保持不变（分栏独立性在关闭路径上的契约）。
    #[test]
    fn closing_a_split_right_pane_only_clears_its_owner_split() {
        let mut workspace = WorkspaceState::new(BTreeMap::new());
        workspace.active_view = Some(ActiveView::LocalSession(1));
        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(2)));
        workspace.active_view = Some(ActiveView::LocalSession(3));
        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(4)));

        // 关闭 Tab1 分栏的右窗格（会话 2）
        assert_eq!(
            workspace.prepare_split_view_close(ActiveView::LocalSession(2)),
            SplitViewCloseOutcome::Closed { retire_pane: None }
        );
        // Tab1 的分栏清空，会话 2 不再是任何分栏的右窗格
        assert!(workspace.split_of(ActiveView::LocalSession(1)).is_none());
        assert!(!workspace.is_split_secondary(ActiveView::LocalSession(2)));
        // Tab3 的分栏与焦点原样保留
        let third = workspace.split_of(ActiveView::LocalSession(3)).unwrap();
        assert_eq!(third.right, Some(ActiveView::LocalSession(4)));
        assert!(workspace.is_split_secondary(ActiveView::LocalSession(4)));
        assert_eq!(workspace.active_view, Some(ActiveView::LocalSession(3)));
    }

    /// 用户场景契约：三个会话，会话2 开分栏后切到会话1 再开分栏，
    /// 两个分栏独立共存；active_split 跟随当前会话；状态栏亮灭随对应会话。
    #[test]
    fn splits_are_independent_across_owner_tabs() {
        let mut workspace = WorkspaceState::new(BTreeMap::new());
        workspace.active_view = Some(ActiveView::LocalSession(1));
        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(2)));

        workspace.active_view = Some(ActiveView::LocalSession(3));
        assert_eq!(workspace.focused_view(), Some(ActiveView::LocalSession(3)));
        assert!(workspace.active_split().is_none());
        assert!(workspace.begin_terminal_split(ActiveView::LocalSession(4)));
        assert_eq!(
            workspace.active_split().unwrap().right,
            Some(ActiveView::LocalSession(4))
        );

        // 会话1 的分栏原样保留
        let second = workspace.split_of(ActiveView::LocalSession(1)).unwrap();
        assert_eq!(second.right, Some(ActiveView::LocalSession(2)));
        assert!(workspace.is_split_secondary(ActiveView::LocalSession(2)));
        assert!(workspace.is_split_secondary(ActiveView::LocalSession(4)));

        // 切回会话1：它的分栏恢复为活动分栏
        workspace.active_view = Some(ActiveView::LocalSession(1));
        assert_eq!(
            workspace.active_split().unwrap().right,
            Some(ActiveView::LocalSession(2))
        );

        // 在会话3 再开分栏也不受影响
        workspace.active_view = Some(ActiveView::LocalSession(3));
        assert!(!workspace.begin_terminal_split(ActiveView::LocalSession(5)));
    }
}

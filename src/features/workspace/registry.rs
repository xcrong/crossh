//! Workspace-owned session registry.
//!
//! This state is deliberately independent from rendering. The shell coordinates
//! actions, while the registry owns the collections that describe open panes.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{Subscription, Task};

use super::state::{ActiveView, LocalDir, LocalSession, LocalSessionId, Tab};
use super::toaster::ToasterState;
use crate::shared::text_editing::TextEditingState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SplitSide {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SplitViewCloseOutcome {
    /// 视图不在分栏中。
    Inactive,
    /// 分栏已随视图关闭；`retire_pane` 是需要上层退休处理的右窗格
    /// （仅当分栏隐藏——属主 Tab 不是活动视图——时关闭属主 Tab 出现）。
    Closed { retire_pane: Option<ActiveView> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerminalSplitState {
    pub(crate) left: ActiveView,
    pub(crate) right: ActiveView,
    pub(crate) focused: SplitSide,
}

impl TerminalSplitState {
    pub(crate) fn new(left: ActiveView, right: ActiveView) -> Self {
        Self {
            left,
            right,
            focused: SplitSide::Right,
        }
    }

    pub(crate) fn focus(&mut self, side: SplitSide) {
        self.focused = side;
    }

    pub(crate) fn focused_view(self) -> ActiveView {
        match self.focused {
            SplitSide::Left => self.left,
            SplitSide::Right => self.right,
        }
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
    pub(crate) remote_tabs: Vec<Tab>,
    pub(crate) local_sessions: BTreeMap<LocalSessionId, LocalSession>,
    pub(crate) local_dirs: BTreeMap<PathBuf, LocalDir>,
    pub(crate) next_local_session_id: LocalSessionId,
    /// GPUI subscriptions are owned by the workspace for the lifetime of its sessions.
    pub(crate) terminal_subscriptions: Vec<Subscription>,
}

impl SessionRegistry {
    pub(crate) fn new(local_dirs: BTreeMap<PathBuf, LocalDir>) -> Self {
        Self {
            remote_tabs: Vec::new(),
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
}

impl WorkspaceState {
    /// 统一移除分栏及对应的宽度槽位，避免两处 map 不同步。
    fn remove_split_entry(&mut self, owner: &ActiveView) -> Option<TerminalSplitState> {
        let split = self.terminal_splits.remove(owner)?;
        self.split_widths.remove(owner);
        Some(split)
    }

    fn retain_splits_not_involving(
        &mut self,
        closed: &[ActiveView],
        removed: &mut Vec<TerminalSplitState>,
    ) {
        self.terminal_splits.retain(|owner, split| {
            if closed.contains(owner) || closed.contains(&split.right) {
                removed.push(*split);
                self.split_widths.remove(owner);
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
        }
    }

    /// 为当前活动视图创建分栏。只检查「属主自身」是否已有分栏：其他 Tab
    /// 的分栏与新分栏独立共存，互不干扰。
    pub(crate) fn begin_terminal_split(&mut self, right: ActiveView) -> bool {
        let Some(left) = self.active_view else {
            return false;
        };
        if left == right || self.terminal_splits.contains_key(&left) {
            return false;
        }
        self.terminal_splits
            .insert(left, TerminalSplitState::new(left, right));
        // 宽度槽位随分栏创建：0.0 哨兵 = 渲染层走均分默认。
        self.split_widths.insert(left, Rc::new(Cell::new(0.)));
        true
    }

    /// 活动视图属主的分栏（分栏与其属主 Tab 绑定）。
    pub(crate) fn active_split(&self) -> Option<TerminalSplitState> {
        self.active_view
            .and_then(|owner| self.terminal_splits.get(&owner).copied())
    }

    pub(crate) fn split_of(&self, owner: ActiveView) -> Option<TerminalSplitState> {
        self.terminal_splits.get(&owner).copied()
    }

    pub(crate) fn focused_view(&self) -> Option<ActiveView> {
        // 分栏跟随其属主 Tab（split.left 即创建时的活动视图）：只有属主
        // Tab 正在展示时，分栏的聚焦侧才算聚焦；否则焦点就是活动视图。
        match self.active_split() {
            Some(split) if Some(split.left) == self.active_view => Some(split.focused_view()),
            _ => self.active_view,
        }
    }

    /// 视图是否正在充当某个分栏的右窗格（标签栏跳过渲染）。
    pub(crate) fn is_split_secondary(&self, view: ActiveView) -> bool {
        self.terminal_splits
            .values()
            .any(|split| split.right == view)
    }

    pub(crate) fn focus_terminal_split(&mut self, side: SplitSide) -> bool {
        let Some(owner) = self.active_view else {
            return false;
        };
        self.terminal_splits.get_mut(&owner).is_some_and(|split| {
            split.focus(side);
            true
        })
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
        } else if split.right == view {
            SplitSide::Right
        } else {
            return false;
        };
        split.focus(side);
        true
    }

    /// 视图参与的任意分栏（属主或右窗格）。
    pub(crate) fn split_containing(&self, view: ActiveView) -> Option<TerminalSplitState> {
        self.terminal_splits.get(&view).copied().or_else(|| {
            self.split_owner_of_right(view)
                .and_then(|owner| self.split_of(owner))
        })
    }

    /// 批量清扫（close all / close others）前拆掉与 `closed` 中任一视图相关
    /// 的分栏（属主或右窗格）。**不做退休**：右窗格会话与其余会话一视同仁
    /// 地随清扫关闭，避免退休路径提前删除造成索引漂移。
    pub(crate) fn take_splits_involving(
        &mut self,
        closed: &[ActiveView],
    ) -> Vec<TerminalSplitState> {
        let mut removed = Vec::new();
        self.retain_splits_not_involving(closed, &mut removed);
        removed
    }

    /// 找到右窗格是 `view` 的分栏所属主（标签栏外的分栏窗格不存在独立
    /// 关闭入口，但 terminal 崩溃等事件仍可能触发其关闭）。
    fn split_owner_of_right(&self, view: ActiveView) -> Option<ActiveView> {
        self.terminal_splits
            .iter()
            .find_map(|(&owner, split)| (split.right == view).then_some(owner))
    }

    /// 分栏视图（属主 Tab 或右窗格）被关闭前的状态处理。
    ///
    /// 分栏激活（属主 Tab 正在展示）时关闭属主 Tab：右窗格接管成为活动
    /// 视图；分栏隐藏时关闭属主 Tab：右窗格失去归属，交由上层退休处理
    /// （无活动销毁 / 有活动保留为普通标签）。关闭右窗格只清空分栏。
    pub(crate) fn prepare_split_view_close(&mut self, view: ActiveView) -> SplitViewCloseOutcome {
        if let Some(split) = self.remove_split_entry(&view) {
            return self.prepare_owner_close(split);
        }
        if let Some(owner) = self.split_owner_of_right(view) {
            self.remove_split_entry(&owner);
            return SplitViewCloseOutcome::Closed { retire_pane: None };
        }
        SplitViewCloseOutcome::Inactive
    }


    fn prepare_owner_close(&mut self, split: TerminalSplitState) -> SplitViewCloseOutcome {
        if self.active_view == Some(split.left) {
            self.active_view = Some(split.right);
            SplitViewCloseOutcome::Closed { retire_pane: None }
        } else {
            SplitViewCloseOutcome::Closed {
                retire_pane: Some(split.right),
            }
        }
    }

    /// Remote tabs are stored in a vector, so a removal shifts references after
    /// the removed index. This only repairs split-owned references; normal
    /// active-view fallback remains in the tab close operation.
    ///
    /// 属主索引本身也会漂移，与 left/right 一起重映射；引用到被删 Tab 的
    /// 分栏直接失效清除（close 流程的 prepare 通常已经清理，此处兜底）。
    pub(crate) fn remap_split_remote_tab_indices(&mut self, removed: usize) {
        let mut next = BTreeMap::new();
        let mut next_widths = BTreeMap::new();
        let old_widths = std::mem::take(&mut self.split_widths);
        for (key, split) in std::mem::take(&mut self.terminal_splits) {
            let original_key = key;
            let key = match key {
                ActiveView::RemoteTab(index) if index == removed => continue,
                ActiveView::RemoteTab(index) if index > removed => ActiveView::RemoteTab(index - 1),
                other => other,
            };
            let Some(left) = remap_remote_tab(split.left, removed) else {
                continue;
            };
            let Some(right) = remap_remote_tab(split.right, removed) else {
                continue;
            };
            next.insert(
                key,
                TerminalSplitState {
                    left,
                    right,
                    ..split
                },
            );
            // 宽度槽位 key 与分栏属主同步迁移；分栏被删的槽位随之清除。
            if let Some(width) = old_widths.get(&original_key) {
                next_widths.insert(key, width.clone());
            }
        }
        self.terminal_splits = next;
        self.split_widths = next_widths;
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

    pub(crate) fn remap_compose_remote_tab_indices(&mut self, removed: usize) {
        let mut next = BTreeMap::new();
        for (view, entry) in std::mem::take(&mut self.compose) {
            let Some(view) = remap_remote_tab(view, removed) else {
                continue;
            };
            next.insert(view, entry);
        }
        self.compose = next;
    }
}

fn remap_remote_tab(view: ActiveView, removed: usize) -> Option<ActiveView> {
    match view {
        ActiveView::RemoteTab(index) if index == removed => None,
        ActiveView::RemoteTab(index) if index > removed => Some(ActiveView::RemoteTab(index - 1)),
        _ => Some(view),
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
        assert!(workspace.sessions.remote_tabs.is_empty());
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
            ActiveView::LocalSession(2)
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

    #[test]
    fn removing_a_remote_tab_repairs_split_indices() {
        let mut workspace = WorkspaceState::new(BTreeMap::new());
        workspace.active_view = Some(ActiveView::RemoteTab(0));
        assert!(workspace.begin_terminal_split(ActiveView::RemoteTab(3)));
        workspace.active_view = Some(ActiveView::RemoteTab(4));
        assert!(workspace.begin_terminal_split(ActiveView::RemoteTab(5)));

        workspace.remap_split_remote_tab_indices(1);
        let first = workspace.split_of(ActiveView::RemoteTab(0)).unwrap();
        assert_eq!(first.left, ActiveView::RemoteTab(0));
        assert_eq!(first.right, ActiveView::RemoteTab(2));
        let second = workspace.split_of(ActiveView::RemoteTab(3)).unwrap();
        assert_eq!(second.left, ActiveView::RemoteTab(3));
        assert_eq!(second.right, ActiveView::RemoteTab(4));

        workspace.remap_split_remote_tab_indices(2);
        assert!(workspace.split_of(ActiveView::RemoteTab(0)).is_none());
        let second = workspace.split_of(ActiveView::RemoteTab(2)).unwrap();
        assert_eq!(second.left, ActiveView::RemoteTab(2));
        assert_eq!(second.right, ActiveView::RemoteTab(3));
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

    /// 契约 5：远程 Tab 索引重映射时宽度槽位 key 跟随迁移；
    /// 属主被删的分栏的槽位清除。
    #[test]
    fn spec_20260821_split_width__remote_remap_moves_and_clears_slots() {
        let mut workspace = WorkspaceState::new(BTreeMap::new());
        workspace.active_view = Some(ActiveView::RemoteTab(0));
        assert!(workspace.begin_terminal_split(ActiveView::RemoteTab(3)));
        workspace.active_view = Some(ActiveView::RemoteTab(4));
        assert!(workspace.begin_terminal_split(ActiveView::RemoteTab(5)));
        workspace.split_widths[&ActiveView::RemoteTab(0)].set(180.);
        workspace.split_widths[&ActiveView::RemoteTab(4)].set(420.);

        // 删除索引 1：两个属主 key 都不变（0 与 4>1→3 迁移）
        workspace.remap_split_remote_tab_indices(1);
        assert_eq!(
            workspace.split_widths[&ActiveView::RemoteTab(0)].get(),
            180.
        );
        assert_eq!(
            workspace.split_widths[&ActiveView::RemoteTab(3)].get(),
            420.
        );

        // 删除索引 2：属主 0 的分栏失效，其槽位清除；第二个属主 3→2 迁移
        workspace.remap_split_remote_tab_indices(2);
        assert!(
            !workspace
                .split_widths
                .contains_key(&ActiveView::RemoteTab(0))
        );
        assert_eq!(
            workspace.split_widths[&ActiveView::RemoteTab(2)].get(),
            420.
        );
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
        assert_eq!(third.right, ActiveView::LocalSession(4));
        assert!(workspace.is_split_secondary(ActiveView::LocalSession(4)));
        assert_eq!(workspace.active_view, Some(ActiveView::LocalSession(3)));
    }

    /// 用户场景契约：三个 Tab，Tab2 开分栏后切到 Tab1 再开分栏，
    /// 两个分栏独立共存；active_split 跟随当前 Tab；状态栏亮灭随对应 Tab。
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
            ActiveView::LocalSession(4)
        );

        // Tab2 的分栏原样保留
        let second = workspace.split_of(ActiveView::LocalSession(1)).unwrap();
        assert_eq!(second.right, ActiveView::LocalSession(2));
        assert!(workspace.is_split_secondary(ActiveView::LocalSession(2)));
        assert!(workspace.is_split_secondary(ActiveView::LocalSession(4)));

        // 切回 Tab2：它的分栏恢复为活动分栏
        workspace.active_view = Some(ActiveView::LocalSession(1));
        assert_eq!(
            workspace.active_split().unwrap().right,
            ActiveView::LocalSession(2)
        );

        // 在 Tab3 再开分栏也不受影响
        workspace.active_view = Some(ActiveView::LocalSession(3));
        assert!(!workspace.begin_terminal_split(ActiveView::LocalSession(5)));
    }
}

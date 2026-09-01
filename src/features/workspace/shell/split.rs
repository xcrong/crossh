//! AppShell terminal split creation, focus, and sizing state.

use std::path::PathBuf;

use gpui::{Context, EntityId, Window};

use crate::features::workspace::registry::SplitSide;

use super::tabs::{SplitPaneRetirement, TabCloseRisk, split_pane_retirement};

use super::{ActiveView, AppShell};

impl AppShell {
    pub(crate) fn open_local_session_for_split(
        &mut self,
        project_dir: PathBuf,
        cwd: PathBuf,
        cx: &mut Context<Self>,
    ) -> Option<ActiveView> {
        let view = self.create_local_session(project_dir, cwd, cx)?;
        let ActiveView::LocalSession(session_id) = view;
        self.refresh_git_status(session_id, false, cx);
        self.status = None;
        cx.notify();
        Some(view)
    }

    pub(crate) fn toggle_terminal_split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 水平分栏切换：若当前 Tab 已有水平分栏（right 存在），则原子关闭右列（含 bottom_right）；
        // 避免两次 request_close 竞态（确认框互斥），先快照右列格子，统一做风险确认。
        if let Some(split) = self.workspace.active_split()
            && split.right.is_some()
        {
            let right_panes: Vec<ActiveView> = [split.right, split.bottom_right]
                .into_iter()
                .flatten()
                .collect();
            // 收集需要确认的 pane
            let mut needs_confirm: Option<TabCloseRisk> = None;
            for &pane in &right_panes {
                let ActiveView::LocalSession(sid) = pane;
                if let Some(risk) = self.local_session_close_risk(sid, cx)
                    && risk.needs_confirmation()
                {
                    needs_confirm = Some(risk);
                    break;
                }
            }
            if let Some(risk) = needs_confirm {
                // 单次确认覆盖整列，避免 confirmation_open 竞态
                let panes = right_panes.clone();
                self.prompt_close_tab(risk, window, cx, move |this, cx| {
                    for pane in panes {
                        let ActiveView::LocalSession(sid) = pane;
                        this.close_local_session(sid, cx);
                    }
                });
            } else {
                for pane in right_panes {
                    let ActiveView::LocalSession(sid) = pane;
                    self.close_local_session(sid, cx);
                }
            }
            return;
        }

        let Some(active_view) = self.workspace.active_view else {
            return;
        };
        let Some(right_view) = self.create_split_terminal(active_view, cx) else {
            return;
        };
        if !self.workspace.begin_terminal_split(right_view) {
            self.rollback_split_terminal(right_view, cx);
            return;
        }
        let split = self
            .workspace
            .active_split()
            .expect("split state was created above");
        self.set_terminal_adjacent_available(split.left, true, cx);
        if let Some(right) = split.right {
            self.set_terminal_adjacent_available(right, true, cx);
        }
        if let Some(bl) = split.bottom_left {
            self.set_terminal_adjacent_available(bl, true, cx);
        }
        if let Some(br) = split.bottom_right {
            self.set_terminal_adjacent_available(br, true, cx);
        }
        self.workspace.focus_terminal_split(SplitSide::Right);
        self.refocus_active_terminal(cx);
        cx.notify();
    }

    pub(crate) fn toggle_vertical_split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 垂直分栏切换：按当前聚焦列切换 bottom 格
        let Some(split) = self.workspace.active_split() else {
            // 无分栏：直接创建左列垂直分栏
            let Some(active_view) = self.workspace.active_view else {
                return;
            };
            let Some(bottom_view) = self.create_split_terminal(active_view, cx) else {
                return;
            };
            if !self.workspace.begin_vertical_split(bottom_view) {
                self.rollback_split_terminal(bottom_view, cx);
                return;
            }
            let split = self
                .workspace
                .active_split()
                .expect("vertical split created");
            for pane in [
                Some(split.left),
                split.right,
                split.bottom_left,
                split.bottom_right,
            ]
            .into_iter()
            .flatten()
            {
                self.set_terminal_adjacent_available(pane, true, cx);
            }
            // 焦点已在 begin_vertical_split 中设为 BottomLeft
            self.refocus_active_terminal(cx);
            cx.notify();
            return;
        };
        // 已有分栏：判断当前聚焦列的 bottom 是否存在
        let focused_is_right = split.focused.is_right_column() && split.right.is_some();
        let has_bottom = if focused_is_right {
            split.bottom_right.is_some()
        } else {
            split.bottom_left.is_some()
        };
        if has_bottom {
            // 关闭该列的 bottom
            let bottom = if focused_is_right {
                split.bottom_right
            } else {
                split.bottom_left
            };
            if let Some(ActiveView::LocalSession(session_id)) = bottom {
                self.request_close_local_session(session_id, window, cx);
            }
            return;
        }
        // 创建 bottom
        let Some(active_view) = self.workspace.active_view else {
            return;
        };
        // 创建时以前景会话的 project/cwd 为模板
        let Some(bottom_view) = self.create_split_terminal(active_view, cx) else {
            return;
        };
        if !self.workspace.begin_vertical_split(bottom_view) {
            self.rollback_split_terminal(bottom_view, cx);
            return;
        }
        let split = self
            .workspace
            .active_split()
            .expect("vertical split created");
        for pane in [
            Some(split.left),
            split.right,
            split.bottom_left,
            split.bottom_right,
        ]
        .into_iter()
        .flatten()
        {
            self.set_terminal_adjacent_available(pane, true, cx);
        }
        self.refocus_active_terminal(cx);
        cx.notify();
    }
    /// 批量清扫前拆掉与被清扫视图相关的分栏
    pub(crate) fn detach_splits_for(&mut self, closed: &[ActiveView], cx: &mut Context<Self>) {
        for split in self.workspace.take_splits_involving(closed) {
            for pane in [
                Some(split.left),
                split.right,
                split.bottom_left,
                split.bottom_right,
            ]
            .into_iter()
            .flatten()
            {
                self.set_terminal_adjacent_available(pane, false, cx);
            }
        }
        // 同步清理终端级的 compose 状态，避免已关闭视图的草稿残留
        let _ = self.workspace.take_composes_involving(closed);
        self.reset_split_ui_if_idle();
    }

    /// 处理退出分栏的 secondary 窗格：按活动风险决定销毁或保留为普通标签。
    fn retire_split_pane(&mut self, view: ActiveView, cx: &mut Context<Self>) {
        let retirement = match view {
            ActiveView::LocalSession(session_id) => {
                split_pane_retirement(self.local_session_close_risk(session_id, cx).as_ref())
            }
        };
        if retirement == SplitPaneRetirement::KeepAsTab {
            return;
        }
        match view {
            ActiveView::LocalSession(session_id) => self.close_local_session(session_id, cx),
        }
    }

    /// 分栏视图关闭前的状态处理：解除 adjacent 标志、让 registry 处理接管/退休
    /// 支持 2x2：属主关闭时遍历所有 secondary，按风险分别退休/保留
    pub(super) fn prepare_terminal_split_view_close(
        &mut self,
        view: ActiveView,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(split) = self.workspace.split_containing(view) else {
            return false;
        };
        let is_owner_close = split.left == view;
        let was_active_owner = self.workspace.active_view == Some(view);
        for pane in [
            Some(split.left),
            split.right,
            split.bottom_left,
            split.bottom_right,
        ]
        .into_iter()
        .flatten()
        {
            self.set_terminal_adjacent_available(pane, false, cx);
        }
        let outcome = self.workspace.prepare_split_view_close(view);
        match outcome {
            crate::features::workspace::registry::SplitViewCloseOutcome::Closed {
                retire_pane: Some(pane),
            } => {
                self.retire_split_pane(pane, cx);
            }
            crate::features::workspace::registry::SplitViewCloseOutcome::Closed {
                retire_pane: None,
            } if is_owner_close => {
                // 属主活跃关闭仅晋升，未返回退休；需处理剩余 secondary
                // was_active_owner 时晋升的 pane 已成为 active_view，避免重复退休
                let promoted = if was_active_owner {
                    self.workspace.active_view
                } else {
                    None
                };
                for extra in split.secondary_views() {
                    if Some(extra) == promoted {
                        continue;
                    }
                    // 隐藏态已退休首个，活跃态需退休其余
                    self.retire_split_pane(extra, cx);
                }
            }
            _ => {}
        }
        // 隐藏态属主关闭：outcome 已退休首个，仍需处理其余 secondary（除首个外）
        if is_owner_close
            && !was_active_owner
            && let crate::features::workspace::registry::SplitViewCloseOutcome::Closed {
                retire_pane: Some(first),
            } = outcome
        {
            for extra in split.secondary_views() {
                if extra == first {
                    continue;
                }
                self.retire_split_pane(extra, cx);
            }
        }
        self.reset_split_ui_if_idle();
        true
    }

    pub(super) fn send_to_adjacent_terminal(
        &mut self,
        source_terminal_id: EntityId,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(split) = self.workspace.active_split() else {
            return;
        };
        let panes = [
            (SplitSide::Left, Some(split.left)),
            (SplitSide::Right, split.right),
            (SplitSide::BottomLeft, split.bottom_left),
            (SplitSide::BottomRight, split.bottom_right),
        ];
        let Some((source_side, _)) = panes.iter().find(|(_, pane)| {
            pane.is_some_and(|v| self.terminal_entity_id_for_view(v) == Some(source_terminal_id))
        }) else {
            return;
        };
        let target_view = match source_side {
            SplitSide::Left => split.right.or(split.bottom_left).or(split.bottom_right),
            SplitSide::Right => split
                .bottom_right
                .or(Some(split.left))
                .or(split.bottom_left),
            SplitSide::BottomLeft => Some(split.left).or(split.bottom_right).or(split.right),
            SplitSide::BottomRight => split.right.or(split.bottom_left).or(Some(split.left)),
        };
        // 若按优先策略未找到且源不是单一，则回退到首个非源窗格
        let target_view = target_view.or_else(|| {
            panes
                .iter()
                .filter_map(|(_, pane)| *pane)
                .find(|v| self.terminal_entity_id_for_view(*v) != Some(source_terminal_id))
        });
        let Some(target_view) = target_view else {
            return;
        };
        match target_view {
            ActiveView::LocalSession(session_id) => {
                if let Some(session) = self.workspace.sessions.local_sessions.get(&session_id) {
                    session
                        .terminal
                        .update(cx, |terminal, cx| terminal.paste_raw_text(text, cx));
                }
            }
        }
    }
    pub(crate) fn focus_terminal_split(&mut self, side: SplitSide, cx: &mut Context<Self>) {
        if self.workspace.focus_terminal_split(side) {
            self.refocus_active_terminal(cx);
            cx.notify();
        }
    }

    /// 为分栏创建右窗格会话。分栏右窗格**总是新建**独立会话：
    /// 标签栏中的每个 Tab 都是独立一等公民，分栏是当前 Tab 的临时
    /// 布局，绝不能把已有 Tab/会话取走或改其存在性（曾经存在过按
    /// target/project+cwd 复用的实现，会吞掉同目标的独立 Tab，见 issue #5）。
    fn create_split_terminal(
        &mut self,
        active_view: ActiveView,
        cx: &mut Context<Self>,
    ) -> Option<ActiveView> {
        match active_view {
            ActiveView::LocalSession(session_id) => {
                let project_dir = self.local_session_project_dir(session_id);
                let cwd = self.local_session_cwd(session_id, cx);
                self.open_local_session_for_split(project_dir, cwd, cx)
            }
        }
    }

    fn rollback_split_terminal(&mut self, view: ActiveView, cx: &mut Context<Self>) {
        match view {
            ActiveView::LocalSession(session_id) => self.close_local_session(session_id, cx),
        }
    }

    /// 分栏全部关闭后复位拖拽标志；宽度/高度槽位已随各删除路径在 registry
    /// 中清理，这里不再触碰。
    fn reset_split_ui_if_idle(&mut self) {
        if self.workspace.terminal_splits.is_empty() {
            self.terminal_split_dragging.set(false);
            self.terminal_split_vertical_dragging.set(false);
            self.terminal_split_vertical_right_dragging.set(false);
        }
    }

    fn set_terminal_adjacent_available(
        &mut self,
        view: ActiveView,
        available: bool,
        cx: &mut Context<Self>,
    ) {
        match view {
            ActiveView::LocalSession(session_id) => {
                if let Some(session) = self.workspace.sessions.local_sessions.get(&session_id) {
                    session.terminal.update(cx, |terminal, term_cx| {
                        terminal.set_adjacent_terminal_available(available, term_cx);
                    });
                }
            }
        }
    }

    fn terminal_entity_id_for_view(&self, view: ActiveView) -> Option<EntityId> {
        match view {
            ActiveView::LocalSession(session_id) => self
                .workspace
                .sessions
                .local_sessions
                .get(&session_id)
                .map(|session| session.terminal.entity_id()),
        }
    }
}

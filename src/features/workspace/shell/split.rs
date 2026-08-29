//! AppShell terminal split creation, focus, and sizing state.

use std::path::PathBuf;

use gpui::{Context, EntityId, Window};

use crate::features::workspace::registry::SplitSide;

use super::tabs::{SplitPaneRetirement, split_pane_retirement};

use super::{ActiveView, AppShell};

impl AppShell {
    pub(crate) fn open_local_session_for_split(
        &mut self,
        project_dir: PathBuf,
        cwd: PathBuf,
        cx: &mut Context<Self>,
    ) -> Option<ActiveView> {
        let view = self.create_local_session(project_dir, cwd, cx)?;
        if let ActiveView::LocalSession(session_id) = view {
            self.refresh_git_status(session_id, false, cx);
        }
        self.status = None;
        cx.notify();
        Some(view)
    }

    pub(crate) fn toggle_terminal_split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 分栏绑定开启它的 Tab：只有属主 Tab 上的分栏按钮
        // 才表示「关闭本 Tab 的分栏」，其他 Tab 的分栏独立共存、互不干扰。
        if let Some(split) = self.workspace.active_split() {
            match split.right {
                ActiveView::RemoteTab(index) => {
                    self.request_close_remote_tab(index, window, cx);
                }
                ActiveView::LocalSession(session_id) => {
                    self.request_close_local_session(session_id, window, cx);
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
        self.set_terminal_adjacent_available(split.right, true, cx);
        self.workspace.focus_terminal_split(SplitSide::Right);
        self.refocus_active_terminal(cx);
        cx.notify();
    }

    /// 批量清扫（close all / close others）前拆掉与被清扫视图相关的分栏
    /// 状态并重置尺规，**不做退休**：右窗格会话与其余会话一视同仁地随清扫
    /// 关闭，避免退休路径提前删除导致后续索引漂移。
    pub(crate) fn detach_splits_for(&mut self, closed: &[ActiveView], cx: &mut Context<Self>) {
        for split in self.workspace.take_splits_involving(closed) {
            self.set_terminal_adjacent_available(split.left, false, cx);
            self.set_terminal_adjacent_available(split.right, false, cx);
        }
        // 同步清理终端级的 compose 状态，避免已关闭视图的草稿残留
        let _ = self.workspace.take_composes_involving(closed);
        self.reset_split_ui_if_idle();
    }

    /// 处理退出分栏的右窗格会话：按活动风险决定销毁或保留为普通标签。
    fn retire_split_pane(&mut self, view: ActiveView, cx: &mut Context<Self>) {
        let retirement = match view {
            ActiveView::RemoteTab(index) => {
                split_pane_retirement(self.remote_tab_close_risk(index, cx).as_ref())
            }
            ActiveView::LocalSession(session_id) => {
                split_pane_retirement(self.local_session_close_risk(session_id, cx).as_ref())
            }
        };
        if retirement == SplitPaneRetirement::KeepAsTab {
            return;
        }
        match view {
            ActiveView::RemoteTab(index) => self.close_remote_tab(index, cx),
            ActiveView::LocalSession(session_id) => self.close_local_session(session_id, cx),
        }
    }

    /// 分栏视图（属主 Tab 或右窗格）关闭前的状态处理：解除 adjacent 标志、
    /// 让 registry 处理接管/退休，最后一个分栏关闭时重置尺规。
    pub(super) fn prepare_terminal_split_view_close(
        &mut self,
        view: ActiveView,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(split) = self.workspace.split_containing(view) else {
            return false;
        };
        self.set_terminal_adjacent_available(split.left, false, cx);
        self.set_terminal_adjacent_available(split.right, false, cx);
        let outcome = self.workspace.prepare_split_view_close(view);
        if let crate::features::workspace::registry::SplitViewCloseOutcome::Closed {
            retire_pane: Some(pane),
        } = outcome
        {
            self.retire_split_pane(pane, cx);
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
        // 只有活动属主的分栏参与交互：隐藏分栏的窗格不可见，不会成为来源。
        let Some(split) = self.workspace.active_split() else {
            return;
        };
        let Some(source_view) = [split.left, split.right]
            .into_iter()
            .find(|view| self.terminal_entity_id_for_view(*view) == Some(source_terminal_id))
        else {
            return;
        };
        let target_view = if source_view == split.left {
            split.right
        } else {
            split.left
        };
        match target_view {
            ActiveView::RemoteTab(index) => {
                if let Some(tab) = self.workspace.sessions.remote_tabs.get(index) {
                    tab.pane.send_text(text, cx);
                }
            }
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
            ActiveView::RemoteTab(index) => {
                let tab = self.workspace.sessions.remote_tabs.get(index)?;
                tab.pane.terminal_entity_id()?;
                let target = tab.target.clone();
                Some(self.open_terminal_target_for_split(target, cx))
            }
        }
    }

    fn rollback_split_terminal(&mut self, view: ActiveView, cx: &mut Context<Self>) {
        match view {
            ActiveView::RemoteTab(index) => self.close_remote_tab(index, cx),
            ActiveView::LocalSession(session_id) => self.close_local_session(session_id, cx),
        }
    }

    /// 分栏全部关闭后复位拖拽标志；宽度槽位已随各删除路径在 registry
    /// 中清理，这里不再触碰。
    fn reset_split_ui_if_idle(&mut self) {
        if self.workspace.terminal_splits.is_empty() {
            self.terminal_split_dragging.set(false);
        }
    }

    fn set_terminal_adjacent_available(
        &mut self,
        view: ActiveView,
        available: bool,
        cx: &mut Context<Self>,
    ) {
        match view {
            ActiveView::RemoteTab(index) => {
                if let Some(tab) = self.workspace.sessions.remote_tabs.get(index) {
                    tab.pane.set_adjacent_terminal_available(available, cx);
                }
            }
            ActiveView::LocalSession(session_id) => {
                if let Some(session) = self.workspace.sessions.local_sessions.get(&session_id) {
                    session.terminal.update(cx, |terminal, cx| {
                        terminal.set_adjacent_terminal_available(available, cx)
                    });
                }
            }
        }
    }

    fn terminal_entity_id_for_view(&self, view: ActiveView) -> Option<EntityId> {
        match view {
            ActiveView::RemoteTab(index) => self
                .workspace
                .sessions
                .remote_tabs
                .get(index)
                .and_then(|tab| tab.pane.terminal_entity_id()),
            ActiveView::LocalSession(session_id) => self
                .workspace
                .sessions
                .local_sessions
                .get(&session_id)
                .map(|session| session.terminal.entity_id()),
        }
    }
}

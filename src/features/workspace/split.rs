//! AppShell terminal split creation, focus, and sizing state.

use std::path::{Path, PathBuf};

use gpui::{Context, EntityId, Window};

use crate::features::workspace::registry::SplitSide;

use super::{ActiveView, AppShell, normalize_local_cwd};

impl AppShell {
    pub(crate) fn open_local_session_for_split(
        &mut self,
        project_dir: PathBuf,
        cwd: PathBuf,
        cx: &mut Context<Self>,
    ) -> ActiveView {
        let view = self.create_local_session(project_dir, cwd, cx);
        if let ActiveView::LocalSession(session_id) = view {
            self.refresh_git_status(session_id, false, cx);
        }
        self.status = None;
        cx.notify();
        view
    }

    pub(crate) fn toggle_terminal_split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(split) = self.workspace.terminal_split {
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
        let Some((right_view, created)) = self.create_split_terminal(active_view, cx) else {
            return;
        };
        if !self.workspace.begin_terminal_split(right_view) {
            if created {
                self.rollback_split_terminal(right_view, cx);
            }
            return;
        }
        let split = self
            .workspace
            .terminal_split
            .expect("split state was created above");
        self.set_terminal_adjacent_available(split.left, true, cx);
        self.set_terminal_adjacent_available(split.right, true, cx);
        self.workspace.focus_terminal_split(SplitSide::Right);
        self.refocus_active_terminal(cx);
        cx.notify();
    }

    pub(crate) fn collapse_terminal_split(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(split) = self.workspace.terminal_split {
            self.set_terminal_adjacent_available(split.left, false, cx);
            self.set_terminal_adjacent_available(split.right, false, cx);
        }
        let collapsed = self.workspace.collapse_terminal_split();
        if collapsed {
            self.terminal_split_width.set(0.);
            self.terminal_split_dragging.set(false);
        }
        collapsed
    }

    pub(super) fn prepare_terminal_split_view_close(
        &mut self,
        view: ActiveView,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(split) = self.workspace.terminal_split else {
            return false;
        };
        if split.left != view && split.right != view {
            return false;
        }
        self.set_terminal_adjacent_available(split.left, false, cx);
        self.set_terminal_adjacent_available(split.right, false, cx);
        self.workspace.prepare_split_view_close(view)
    }

    pub(super) fn send_to_adjacent_terminal(
        &mut self,
        source_terminal_id: EntityId,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(split) = self.workspace.terminal_split else {
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

    fn create_split_terminal(
        &mut self,
        active_view: ActiveView,
        cx: &mut Context<Self>,
    ) -> Option<(ActiveView, bool)> {
        match active_view {
            ActiveView::LocalSession(session_id) => {
                let project_dir = self.local_session_project_dir(session_id);
                let cwd = self.local_session_cwd(session_id, cx);
                if let Some(reusable) =
                    self.find_reusable_local_session(active_view, &project_dir, &cwd, cx)
                {
                    return Some((reusable, false));
                }
                Some((
                    self.open_local_session_for_split(project_dir, cwd, cx),
                    true,
                ))
            }
            ActiveView::RemoteTab(index) => {
                let tab = self.workspace.sessions.remote_tabs.get(index)?;
                tab.pane.terminal_entity_id()?;
                let target = tab.target.clone();
                if let Some(reusable) = self.find_reusable_remote_terminal(active_view, &target) {
                    return Some((reusable, false));
                }
                Some((self.open_terminal_target_for_split(target, cx), true))
            }
        }
    }

    fn rollback_split_terminal(&mut self, view: ActiveView, cx: &mut Context<Self>) {
        match view {
            ActiveView::RemoteTab(index) => self.close_remote_tab(index, cx),
            ActiveView::LocalSession(session_id) => self.close_local_session(session_id, cx),
        }
    }

    fn find_reusable_remote_terminal(&self, left: ActiveView, target: &str) -> Option<ActiveView> {
        for (index, tab) in self.workspace.sessions.remote_tabs.iter().enumerate().rev() {
            let view = ActiveView::RemoteTab(index);
            if view != left && tab.target == target && tab.pane.terminal_entity_id().is_some() {
                return Some(view);
            }
        }
        None
    }

    fn find_reusable_local_session(
        &self,
        left: ActiveView,
        project_dir: &Path,
        cwd: &Path,
        cx: &Context<Self>,
    ) -> Option<ActiveView> {
        for (&session_id, session) in self.workspace.sessions.local_sessions.iter().rev() {
            let view = ActiveView::LocalSession(session_id);
            if view == left || session.project_dir.as_path() != project_dir {
                continue;
            }
            let session_cwd = session
                .terminal
                .read(cx)
                .cwd
                .as_deref()
                .map(|cwd| normalize_local_cwd(PathBuf::from(cwd)))
                .unwrap_or_else(|| session.cwd.clone());
            if session_cwd.as_path() == cwd {
                return Some(view);
            }
        }
        None
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

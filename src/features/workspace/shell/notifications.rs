//! 系统通知点击的归属与 pane 定位。
//!
//! 通知只由发出它的终端处理（`TerminalView` 内校验标签）；命中后把对应
//! 项目/tab/pane 激活并把焦点放回该 pane。

use super::AppShell;
use crossh_session::workspace::{PaneId, WorkspaceOp};
use crossh_terminal_view::TerminalItem as _;
use gpui::{Context, SystemNotificationResponse};

impl AppShell {
    pub(crate) fn handle_system_notification_response(
        &mut self,
        response: SystemNotificationResponse,
        cx: &mut Context<Self>,
    ) -> bool {
        let hit = self
            .workspace
            .pane_views
            .iter()
            .find_map(|(pane_id, view)| {
                view.handle_system_notification_response(&response, cx)
                    .map(|focus| (*pane_id, focus))
            });
        let Some((pane_id, focus)) = hit else {
            return false;
        };
        if focus {
            self.reveal_pane(pane_id, cx);
            if let Some(view) = self.workspace.pane_view(pane_id) {
                view.request_focus(cx);
            }
        }
        cx.notify();
        true
    }

    /// 依次激活 pane 所属项目、tab 与 pane，让通知对应的终端可见。
    fn reveal_pane(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        let Some((project_id, tab_id, tab_is_active, project_is_active)) =
            self.workspace.snapshot.projects.iter().find_map(|project| {
                let tab = project
                    .tabs
                    .iter()
                    .find(|tab| tab.layout.find_pane(pane_id).is_some())?;
                Some((
                    project.id,
                    tab.id,
                    project.active_tab == Some(tab.id),
                    self.workspace.snapshot.active_project == Some(project.id),
                ))
            })
        else {
            return;
        };
        if !project_is_active {
            self.send_workspace_op(WorkspaceOp::ActivateProject { project_id }, cx);
        }
        if !tab_is_active {
            self.send_workspace_op(WorkspaceOp::ActivateTab { tab_id }, cx);
        }
        self.send_workspace_op(WorkspaceOp::ActivatePane { pane_id }, cx);
    }
}

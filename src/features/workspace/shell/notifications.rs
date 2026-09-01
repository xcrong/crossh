//! 系统通知点击的归属与分栏恢复逻辑。
//!
//! 通知只应由发出它的终端处理(通过 `TerminalView::handle_system_notification_response`
//! 的标签校验);响应后若终端不处于当前活动 Tab,`jump_back_to_split_pane`
//! 负责把工作区切回其分栏属主 Tab —— 否则渲染契约(仅属主活动时渲染分栏)
//! 会让通知终端以全屏覆盖同属主的另一窗格。
//!
//! 与 shell.rs 拆分:shell 只管状态与窗口生命周期,通知契约独立成模块。

use super::AppShell;
use crate::features::workspace::registry::SplitSide;
use crate::features::workspace::view::ActiveView;
use gpui::{Context, SystemNotificationResponse};

impl AppShell {
    /// 分发系统通知响应:通知来自 local session 的终端时
    /// 返回 true(被处理),并确保焦点回到分栏场景下的属主窗格。
    pub(crate) fn handle_system_notification_response(
        &mut self,
        response: SystemNotificationResponse,
        cx: &mut Context<Self>,
    ) -> bool {
        let log_diag = |tag: &str,
                        handled: bool,
                        view: Option<ActiveView>,
                        active_view: Option<ActiveView>,
                        active_split: bool| {
            log::info!(
                "notification response tag={tag} handled={handled} view={view:?} active_view={active_view:?} active_split=({active_split})",
            );
        };

        let mut handled_local = false;
        let mut local_focus = None;
        for (&session_id, session) in &self.workspace.sessions.local_sessions {
            let handled = session.terminal.update(cx, |terminal, cx| {
                terminal.handle_system_notification_response(&response, cx)
            });
            let Some(focus) = handled else {
                continue;
            };
            handled_local = true;
            if focus {
                session
                    .terminal
                    .update(cx, |terminal, _cx| terminal.request_focus());
                local_focus = Some(ActiveView::LocalSession(session_id));
            }
            break;
        }
        if handled_local {
            if let Some(view) = local_focus
                && !self.workspace.focus_split_view(view)
            {
                self.jump_back_to_split_pane(view, cx);
            }
            if local_focus.is_some() {
                cx.notify();
            }
            log_diag(
                response.tag.as_ref(),
                true,
                local_focus,
                self.workspace.active_view,
                self.workspace.active_split().is_some(),
            );
            return true;
        }
        log_diag(
            response.tag.as_ref(),
            false,
            None,
            self.workspace.active_view,
            self.workspace.active_split().is_some(),
        );
        false
    }

    /// 通知点击的视图不在当前活动 Tab 时,若它属于某个分栏,
    /// 切回分栏属主 Tab 恢复分栏渲染,再把焦点放进发出通知的窗格;
    /// 否则退回通知视图本身。
    fn jump_back_to_split_pane(&mut self, view: ActiveView, cx: &mut Context<Self>) {
        let Some(split) = self.workspace.split_containing(view) else {
            self.workspace.active_view = Some(view);
            return;
        };
        self.workspace.active_view = Some(split.left);
        let side = if split.left == view {
            SplitSide::Left
        } else if split.right == Some(view) {
            SplitSide::Right
        } else if split.bottom_left == Some(view) {
            SplitSide::BottomLeft
        } else if split.bottom_right == Some(view) {
            SplitSide::BottomRight
        } else {
            // 防御：split_containing 已保证 view 在分栏中，理论不可达
            SplitSide::Left
        };
        self.workspace.focus_terminal_split(side);
        self.refocus_active_terminal(cx);
        cx.notify();
    }
}

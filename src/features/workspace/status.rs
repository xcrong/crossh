//! 工作区状态点/徽标的业务颜色映射。
//!
//! 组件库的 `StatusDot` 只消费颜色;状态到颜色的语义映射统一收敛在本模块，
//! 保证侧栏主机列表、远程/本地标签条与快捷命令栏的状态点颜色一致。

use gpui::Rgba;

use crate::features::terminal::ConnState;
use crate::shared::i18n;
use crossh_core::commands::BackgroundTaskStatus;
use crossh_ui::theme;

/// 后台任务的状态文字。
pub(crate) fn background_task_label(status: BackgroundTaskStatus) -> String {
    i18n::text(match status {
        BackgroundTaskStatus::Running => "quick_commands.running",
        BackgroundTaskStatus::Stopping => "quick_commands.stopping",
        BackgroundTaskStatus::Succeeded => "quick_commands.succeeded",
        BackgroundTaskStatus::Failed => "quick_commands.failed",
        BackgroundTaskStatus::Terminated => "quick_commands.terminated",
    })
}

/// 后台任务的状态点颜色。
pub(crate) fn background_task_color(status: BackgroundTaskStatus) -> Rgba {
    match status {
        BackgroundTaskStatus::Running => theme::warning(),
        BackgroundTaskStatus::Stopping | BackgroundTaskStatus::Terminated => theme::faint_text(),
        BackgroundTaskStatus::Succeeded => theme::accent(),
        BackgroundTaskStatus::Failed => theme::danger(),
    }
}

/// 连接状态的状态点颜色（侧栏主机列表与远程标签条共用）。
pub(crate) fn conn_state_dot_color(state: &Option<ConnState>) -> Rgba {
    match state {
        Some(ConnState::Connected) => theme::accent(),
        Some(ConnState::Connecting) => theme::warning(),
        Some(ConnState::Error(_)) => theme::danger(),
        Some(ConnState::Closed) | None => theme::faint_text(),
    }
}

/// 本地会话标签状态点颜色：有命令在运行时优先显示警示色（连接色不再生效）。
pub(crate) fn local_tab_dot_color(state: &Option<ConnState>, command_running: bool) -> Rgba {
    if command_running {
        return theme::warning();
    }
    conn_state_dot_color(state)
}

#[cfg(test)]
mod tests {
    use super::{
        background_task_color, background_task_label, conn_state_dot_color, local_tab_dot_color,
    };
    use crate::features::terminal::ConnState;
    use crossh_core::commands::BackgroundTaskStatus;
    use crossh_ui::theme;

    #[test]
    fn conn_state_dot_maps_all_branches() {
        assert_eq!(
            conn_state_dot_color(&Some(ConnState::Connected)),
            theme::accent()
        );
        assert_eq!(
            conn_state_dot_color(&Some(ConnState::Connecting)),
            theme::warning()
        );
        assert_eq!(
            conn_state_dot_color(&Some(ConnState::Error("failed".into()))),
            theme::danger()
        );
        assert_eq!(
            conn_state_dot_color(&Some(ConnState::Closed)),
            theme::faint_text()
        );
        assert_eq!(conn_state_dot_color(&None), theme::faint_text());
    }

    #[test]
    fn local_tab_dot_prefers_warning_while_command_runs() {
        let connected = Some(ConnState::Connected);
        let error = Some(ConnState::Error("failed".into()));

        assert_eq!(local_tab_dot_color(&connected, true), theme::warning());
        assert_eq!(local_tab_dot_color(&connected, false), theme::accent());
        assert_eq!(local_tab_dot_color(&error, true), theme::warning());
        assert_eq!(local_tab_dot_color(&error, false), theme::danger());
        assert_eq!(local_tab_dot_color(&None, false), theme::faint_text());
        assert_eq!(
            local_tab_dot_color(&Some(ConnState::Closed), true),
            theme::warning()
        );
    }

    #[test]
    fn background_task_maps_color_for_all_statuses() {
        assert_eq!(
            background_task_color(BackgroundTaskStatus::Running),
            theme::warning()
        );
        assert_eq!(
            background_task_color(BackgroundTaskStatus::Stopping),
            theme::faint_text()
        );
        assert_eq!(
            background_task_color(BackgroundTaskStatus::Terminated),
            theme::faint_text()
        );
        assert_eq!(
            background_task_color(BackgroundTaskStatus::Succeeded),
            theme::accent()
        );
        assert_eq!(
            background_task_color(BackgroundTaskStatus::Failed),
            theme::danger()
        );
    }

    #[test]
    fn background_task_maps_distinct_nonempty_labels() {
        let labels: Vec<String> = [
            BackgroundTaskStatus::Running,
            BackgroundTaskStatus::Stopping,
            BackgroundTaskStatus::Succeeded,
            BackgroundTaskStatus::Failed,
            BackgroundTaskStatus::Terminated,
        ]
        .iter()
        .map(|status| background_task_label(*status))
        .collect();
        assert!(labels.iter().all(|label| !label.is_empty()));
        for i in 0..labels.len() {
            assert!(!labels[i..].iter().skip(1).any(|other| other == &labels[i]));
        }
    }
}

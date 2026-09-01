//! 工作区状态点/徽标的业务颜色映射。
//!
//! 组件库的 `StatusDot` 只消费颜色;状态到颜色的语义映射统一收敛在本模块，
//! 保证侧栏主机列表与本地标签条的状态点颜色一致。

use gpui::Rgba;

use crate::features::terminal::ConnState;
use crossh_ui::theme;

/// 连接状态的状态点颜色（侧栏主机列表与远程标签条共用）。
pub(crate) fn conn_state_dot_color(state: &Option<ConnState>) -> Rgba {
    match state {
        None => theme::faint_text(),
        Some(ConnState::Connecting) => theme::warning(),
        Some(ConnState::Connected) => theme::accent(),
        Some(ConnState::Error(_)) => theme::danger(),
        Some(ConnState::Closed) => theme::faint_text(),
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
    use super::{conn_state_dot_color, local_tab_dot_color};

    use crate::features::terminal::ConnState;
    use crossh_ui::theme;

    #[test]
    fn conn_state_dot_maps_all_branches() {
        assert_eq!(conn_state_dot_color(&None), theme::faint_text());
        assert_eq!(
            conn_state_dot_color(&Some(ConnState::Connecting)),
            theme::warning()
        );
        assert_eq!(
            conn_state_dot_color(&Some(ConnState::Connected)),
            theme::accent()
        );
        assert_eq!(
            conn_state_dot_color(&Some(ConnState::Error("boom".into()))),
            theme::danger()
        );
        assert_eq!(
            conn_state_dot_color(&Some(ConnState::Closed)),
            theme::faint_text()
        );
    }

    #[test]
    fn local_tab_dot_prefers_warning_while_command_runs() {
        assert_eq!(
            local_tab_dot_color(&Some(ConnState::Connected), true),
            theme::warning()
        );
        assert_eq!(
            local_tab_dot_color(&Some(ConnState::Connected), false),
            theme::accent()
        );
        assert_eq!(local_tab_dot_color(&None, true), theme::warning());
    }
}

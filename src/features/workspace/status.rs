//! 工作区状态点/徽标的业务颜色映射。
//!
//! 组件库的 `StatusDot` 只消费颜色;状态到颜色的语义映射统一收敛在本模块，
//! 保证侧栏主机列表与本地标签条的状态点颜色一致。

use gpui::Rgba;

use crossh_core::i18n;
use crossh_terminal::{ConnState, ProgramState, ProgramStatus};
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

/// OSC 7501 程序状态的标签状态点颜色，优先级高于连接/命令运行色。
/// `idle`/无状态返回 `None`，调用方回落到 [`local_tab_dot_color`]。
pub(crate) fn program_status_dot_color(status: &Option<ProgramStatus>) -> Option<Rgba> {
    match status.as_ref().map(|s| s.state) {
        Some(ProgramState::Working) => Some(theme::accent()),
        Some(ProgramState::Blocked) => Some(theme::warning()),
        Some(ProgramState::Done) => Some(theme::accent()),
        Some(ProgramState::Error) => Some(theme::danger()),
        Some(ProgramState::Idle) | None => None,
    }
}

/// working 且带进度时拼在标签后的进度后缀（如 `" 40%"`）。
pub(crate) fn program_status_progress_suffix(status: &Option<ProgramStatus>) -> Option<String> {
    let status = status.as_ref()?;
    if status.state == ProgramState::Working {
        status.progress.map(|p| format!(" {p}%"))
    } else {
        None
    }
}

/// OSC 7501 状态的标签悬停提示：优先展示程序上报的 msg，否则退回状态名。
pub(crate) fn program_status_tooltip(status: &Option<ProgramStatus>) -> Option<String> {
    let status = status.as_ref()?;
    let label = match status.state {
        ProgramState::Working => i18n::text("tab.status.working"),
        ProgramState::Blocked => i18n::text("tab.status.blocked"),
        ProgramState::Done => i18n::text("tab.status.done"),
        ProgramState::Error => i18n::text("tab.status.error"),
        ProgramState::Idle => return None,
    };
    Some(match &status.msg {
        Some(msg) if !msg.is_empty() => format!("{label}: {msg}"),
        _ => label,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        conn_state_dot_color, local_tab_dot_color, program_status_dot_color,
        program_status_progress_suffix,
    };

    use crossh_terminal::{ConnState, ProgramState, ProgramStatus};
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

    fn status(state: ProgramState, progress: Option<u8>) -> Option<ProgramStatus> {
        Some(ProgramStatus {
            state,
            app: None,
            msg: None,
            progress,
        })
    }

    #[test]
    fn program_status_dot_maps_states() {
        assert_eq!(
            program_status_dot_color(&status(ProgramState::Working, None)),
            Some(theme::accent())
        );
        assert_eq!(
            program_status_dot_color(&status(ProgramState::Blocked, None)),
            Some(theme::warning())
        );
        assert_eq!(
            program_status_dot_color(&status(ProgramState::Done, None)),
            Some(theme::accent())
        );
        assert_eq!(
            program_status_dot_color(&status(ProgramState::Error, None)),
            Some(theme::danger())
        );
        // idle 与无状态回落到普通连接/命令色
        assert_eq!(
            program_status_dot_color(&status(ProgramState::Idle, None)),
            None
        );
        assert_eq!(program_status_dot_color(&None), None);
    }

    #[test]
    fn program_status_dot_overrides_command_running() {
        // 程序状态优先于命令运行色：blocked 即便 ALT_SCREEN 也不被覆盖
        let dot = program_status_dot_color(&status(ProgramState::Blocked, None));
        assert_eq!(dot, Some(theme::warning()));
    }

    #[test]
    fn program_status_progress_suffix_only_for_working() {
        assert_eq!(
            program_status_progress_suffix(&status(ProgramState::Working, Some(40))),
            Some(" 40%".to_string())
        );
        assert_eq!(
            program_status_progress_suffix(&status(ProgramState::Working, None)),
            None
        );
        assert_eq!(
            program_status_progress_suffix(&status(ProgramState::Blocked, Some(40))),
            None
        );
        assert_eq!(program_status_progress_suffix(&None), None);
    }
}

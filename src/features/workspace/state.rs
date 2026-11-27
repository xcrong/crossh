//! 工作区纯状态类型：compose 草稿、git 刷新合并与状态优先级。
//!
//! 布局与活动位置全部来自 daemon 快照（见 `projection.rs`），这里只保留
//! 不参与持久化、按 `PaneId` 归属的客户端状态。

use crossh_core::text_editing::TextEditingState;
use crossh_terminal::ConnState;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ComposeEntry {
    pub visible: bool,
    pub state: TextEditingState,
    pub last_visible: bool,
    /// 上次触发自动滚动的文本，仅文本变化时滚一次，避免每帧把视图拉回导致无法手动滚读。
    pub last_scrolled_value: String,
}

impl Default for ComposeEntry {
    fn default() -> Self {
        Self {
            visible: false,
            state: TextEditingState::new(String::new()),
            last_visible: false,
            last_scrolled_value: String::new(),
        }
    }
}

/// 每个 pane 最多运行一个 Git 状态查询；期间的刷新请求只合并为一次后续检查。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GitStatusRefresh {
    in_flight: bool,
    pending: bool,
}

impl GitStatusRefresh {
    pub(crate) fn request(&mut self) -> bool {
        if self.in_flight {
            self.pending = true;
            false
        } else {
            self.in_flight = true;
            true
        }
    }

    pub(crate) fn finish(&mut self) -> bool {
        self.in_flight = false;
        std::mem::take(&mut self.pending)
    }
}

/// 多 pane 项目取「最活跃」的状态用于侧栏/徽标。
pub(crate) fn preferred_state(left: ConnState, right: ConnState) -> ConnState {
    if state_priority(&right) > state_priority(&left) {
        right
    } else {
        left
    }
}

fn state_priority(state: &ConnState) -> u8 {
    match state {
        ConnState::Connected => 4,
        ConnState::Connecting => 3,
        ConnState::Error(_) => 2,
        ConnState::Closed => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_status_refresh_coalesces_overlapping_requests() {
        let mut refresh = GitStatusRefresh::default();
        assert!(refresh.request());
        assert!(!refresh.request());
        assert!(!refresh.request());
        assert!(refresh.finish());
        assert!(refresh.request());
        assert!(!refresh.finish());
    }

    #[test]
    fn project_group_prefers_a_live_session_state() {
        assert_eq!(
            preferred_state(ConnState::Closed, ConnState::Connecting),
            ConnState::Connecting
        );
        assert_eq!(
            preferred_state(ConnState::Connecting, ConnState::Connected),
            ConnState::Connected
        );
        assert_eq!(
            preferred_state(ConnState::Connected, ConnState::Error("failed".into())),
            ConnState::Connected
        );
    }
}

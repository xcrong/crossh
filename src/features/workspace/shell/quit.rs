//! AppShell shutdown: detach from the daemon, never confirm.
//!
//! 会话归属 daemon，关窗口/退出 GUI 不结束任何会话（只 detach），
//! 因此不再为"有命令在跑"弹关闭确认（标签关闭仍保留确认，
//! 见 `tabs.rs`，它确实会结束会话）。

use std::time::Duration;

use super::*;

impl AppShell {
    pub(super) fn handle_quit(
        &mut self,
        _: &crate::Quit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_app_quit(window, cx);
    }

    pub(crate) fn request_app_quit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.shutdown_in_progress {
            return;
        }
        let needs_grace = self.begin_shutdown(cx);
        self.quit_now_or_after_grace(cx, needs_grace);
    }

    pub(crate) fn request_close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.should_close_window(cx) {
            window.remove_window();
        }
    }

    pub(super) fn should_close_window(&mut self, cx: &mut Context<Self>) -> bool {
        if self.shutdown_in_progress {
            return true;
        }
        // 关窗口只 detach：pump 与 daemon 连接活在静态 tokio runtime 上，
        // 已入队的 Close 不受 `remove_window` 影响，无需宽限延迟（已证伪）。
        let _ = self.begin_shutdown(cx);
        true
    }

    /// 关机前置：标记状态并发出 scratch `Close`。
    /// 返回是否真正发出了 `Close`（调用方据此决定退出是否延迟）。
    pub(super) fn begin_shutdown(&mut self, cx: &mut Context<Self>) -> bool {
        if self.shutdown_in_progress {
            return false;
        }
        self.shutdown_in_progress = true;
        self.status = Some(i18n::text("quit.closing"));
        // 会话归属 daemon：关窗口/退出 GUI 只 detach，不断开远端，
        // 正在跑的命令随 daemon 存活。显式结束会话走 `close_local_session`。
        // 例外：临时终端是 AppShell 私有、不进工作区布局，退出必须显式 Close，
        // 否则 daemon 侧遗留 `ROLE=none` 会话；隐藏抽屉仍只 detach 复用同一 PTY。
        let sent = self.close_scratch_for_shutdown(cx);
        cx.notify();
        // 纯逻辑收敛到一处：发过 Close 才延迟退出，单测锁定该语义。
        shutdown_needs_close_grace(sent)
    }

    /// 真退出：无 `Close` 直接退；发过 `Close` 则等 1.5s 再退。
    /// `Close` 只是 mpsc 入队，真关闭需 pump 调度 + TCP roundtrip，
    /// 立即 `quit` 会在 daemon 回执前杀进程致 `ROLE=none` 遗留。
    pub(super) fn quit_now_or_after_grace(&self, cx: &mut Context<Self>, needs_grace: bool) {
        if !needs_grace {
            cx.quit();
            return;
        }
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(SHUTDOWN_CLOSE_GRACE).await;
            cx.update(|cx| cx.quit());
        })
        .detach();
    }
}

/// scratch `Close` 落盘宽限：覆盖 pump 调度 + 一次 TCP roundtrip，超时兜底退出。
pub(crate) const SHUTDOWN_CLOSE_GRACE: Duration = Duration::from_millis(1500);

/// 纯逻辑：只有真正发出 `Close` 才需要延迟退出（供单测锁定语义）。
pub(crate) fn shutdown_needs_close_grace(had_scratch_terminal: bool) -> bool {
    had_scratch_terminal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_delays_quit_only_when_close_sent() {
        // 无临时终端：无 Close，直接退。
        assert!(!shutdown_needs_close_grace(false));
        // 有临时终端：Close 已入队，等回执/超时再退。
        assert!(shutdown_needs_close_grace(true));
    }

    #[test]
    fn shutdown_grace_stays_within_one_to_two_seconds() {
        // 竞态窗口：pump 调度 + TCP roundtrip；太短盖不住，太长拖退出体验。
        assert!(SHUTDOWN_CLOSE_GRACE >= Duration::from_secs(1));
        assert!(SHUTDOWN_CLOSE_GRACE <= Duration::from_secs(2));
    }
}

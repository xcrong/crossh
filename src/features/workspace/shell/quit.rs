//! AppShell shutdown: detach from the daemon, never confirm.
//!
//! 会话归属 daemon，关窗口/退出 GUI 不结束任何会话（只 detach），
//! 因此不再为"有命令在跑"弹关闭确认（标签关闭仍保留确认，
//! 见 `tabs.rs`，它确实会结束会话）。

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
        self.begin_shutdown(cx);
        cx.quit();
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
        self.begin_shutdown(cx);
        true
    }

    pub(super) fn begin_shutdown(&mut self, cx: &mut Context<Self>) {
        if self.shutdown_in_progress {
            return;
        }
        self.shutdown_in_progress = true;
        self.status = Some(i18n::text("quit.closing"));
        // 会话归属 daemon：关窗口/退出 GUI 只 detach，不断开远端，
        // 正在跑的命令随 daemon 存活。显式结束会话走 `close_local_session`。
        cx.notify();
    }
}

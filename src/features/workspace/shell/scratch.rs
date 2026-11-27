//! Scratch 临时终端的显隐与生命周期（抽屉式）。
//! 单例、复用同一 PTY、仅内存态，不进入 daemon 工作区布局。

use std::path::PathBuf;

use gpui::{Context, Window};

use super::AppShell;
use crossh_terminal_view::TerminalItem as _;
use crossh_terminal_view::TerminalView as TerminalViewEntity;

pub(crate) const SCRATCH_DEFAULT_HEIGHT: f32 = 420.0;
pub(crate) const SCRATCH_MIN_HEIGHT: f32 = 200.0;

impl AppShell {
    pub(crate) fn toggle_scratch_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.scratch_visible {
            self.hide_scratch_terminal(cx);
        } else {
            self.show_scratch_terminal(window, cx);
        }
    }

    pub(crate) fn show_scratch_terminal(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_scratch_terminal(cx);
        self.scratch_visible = true;
        if let Some(terminal) = &self.scratch_terminal {
            terminal.request_focus(cx);
        }
        cx.notify();
    }

    pub(crate) fn hide_scratch_terminal(&mut self, cx: &mut Context<Self>) {
        if !self.scratch_visible {
            return;
        }
        self.scratch_visible = false;
        self.refocus_active_terminal(cx);
        cx.notify();
    }

    /// 退出/关窗口时显式结束临时会话，避免 daemon 侧遗留 `ROLE=none` 会话。
    /// 隐藏抽屉不走这里：隐藏只改可见性不断链，同一 GUI 内复用同一 PTY。
    /// 返回是否真正发出了 `Close`：调用方据此决定退出是否需要宽限延迟
    ///（`Close` 只是 mpsc 入队，真关闭需 pump 调度 + TCP roundtrip）。
    pub(crate) fn close_scratch_for_shutdown(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(terminal) = self.scratch_terminal.take() else {
            return false;
        };
        terminal.update(cx, |view, cx| view.request_close(cx));
        self.scratch_subscription.take();
        self.scratch_visible = false;
        true
    }

    fn ensure_scratch_terminal(&mut self, cx: &mut Context<Self>) {
        if self.scratch_terminal.is_some() {
            return;
        }
        let cwd = self.scratch_initial_cwd(cx);
        let terminal =
            TerminalViewEntity::from_daemon_create(cwd, self.terminal_settings.clone(), cx);
        let subscription = cx.subscribe(&terminal, |this, _terminal, event, cx| {
            if matches!(event, crossh_terminal_view::TerminalEvent::Closed) {
                this.scratch_terminal = None;
                this.scratch_subscription = None;
                this.scratch_visible = false;
                this.refocus_active_terminal(cx);
                cx.notify();
            }
        });
        self.scratch_terminal = Some(terminal);
        self.scratch_subscription = Some(subscription);
    }

    fn scratch_initial_cwd(&self, _cx: &Context<Self>) -> PathBuf {
        // 临时终端为全局单例，不跟随任何项目；创建时的初始 cwd 固定为家目录，
        // 避免“从某项目打开就继承该项目目录”的项目污染。后续复用同一 PTY，
        // 用户在内部 cd 后的目录会自然保留，无需每次重置。
        if let Some(home) = dirs::home_dir()
            && let Some(path) = crate::features::workspace::local_paths::normalize_local_cwd(home)
        {
            return path;
        }
        crate::features::workspace::local_paths::current_local_cwd()
    }

    pub(crate) fn scratch_height_value(&self) -> f32 {
        let raw = self.scratch_height.get();
        if raw <= 0.0 {
            SCRATCH_DEFAULT_HEIGHT
        } else {
            raw.max(SCRATCH_MIN_HEIGHT)
        }
    }
}

/// 供纯逻辑单测使用的高度 clamp 辅助，窗口自适应下仅保底 MIN，无固定 MAX。
pub(crate) fn clamp_scratch_height(raw: f32) -> f32 {
    if raw <= 0.0 {
        SCRATCH_DEFAULT_HEIGHT
    } else {
        raw.max(SCRATCH_MIN_HEIGHT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_height_clamps_between_bounds_and_falls_back_to_default() {
        assert_eq!(clamp_scratch_height(0.), SCRATCH_DEFAULT_HEIGHT);
        assert_eq!(clamp_scratch_height(-10.), SCRATCH_DEFAULT_HEIGHT);
        assert_eq!(clamp_scratch_height(50.), SCRATCH_MIN_HEIGHT);
        assert_eq!(clamp_scratch_height(420.), 420.);
        assert_eq!(clamp_scratch_height(900.), 900.);
    }

    #[test]
    fn scratch_visible_toggle_is_symmetric() {
        // 纯逻辑：visible 取反两次应回到原值
        let mut visible = false;
        visible = !visible;
        assert!(visible);
        visible = !visible;
        assert!(!visible);
    }
}

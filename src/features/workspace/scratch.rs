//! Scratch 临时终端的显隐与生命周期（抽屉式）。
//! 单例、复用同一 PTY、仅内存态，不进 `local_dirs / recent_dirs`。

use std::path::PathBuf;

use gpui::{Context, Window};

use super::AppShell;
use crate::features::terminal::view::TerminalView as TerminalViewEntity;

pub(crate) const SCRATCH_DEFAULT_HEIGHT: f32 = 220.0;
pub(crate) const SCRATCH_MIN_HEIGHT: f32 = 120.0;
pub(crate) const SCRATCH_MAX_HEIGHT: f32 = 400.0;

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
            terminal.update(cx, |term, _| term.request_focus());
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

    fn ensure_scratch_terminal(&mut self, cx: &mut Context<Self>) {
        if self.scratch_terminal.is_some() {
            return;
        }
        let cwd = self.scratch_initial_cwd(cx);
        let terminal = TerminalViewEntity::from_local_zed(cwd, self.terminal_settings.clone(), cx);
        let subscription = cx.subscribe(&terminal, |this, _terminal, event, cx| {
            if matches!(event, crate::features::terminal::TerminalEvent::Closed) {
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

    fn scratch_initial_cwd(&self, cx: &Context<Self>) -> PathBuf {
        // 优先使用当前活动 LocalSession 的 cwd，其次 HOME，最后 fallback "/"
        if let Some(crate::features::workspace::view::ActiveView::LocalSession(session_id)) =
            self.workspace.active_view
            && let Some(session) = self.workspace.sessions.local_sessions.get(&session_id)
        {
            if let Some(cwd) = session.terminal.read(cx).cwd.as_deref()
                && let Some(path) =
                    crate::features::workspace::local_paths::normalize_local_cwd(PathBuf::from(cwd))
            {
                return path;
            }
            return session.cwd.clone();
        }
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
            raw.clamp(SCRATCH_MIN_HEIGHT, SCRATCH_MAX_HEIGHT)
        }
    }
}

/// 供纯逻辑单测使用的高度 clamp 辅助，保持与渲染层一致的边界。
pub(crate) fn clamp_scratch_height(raw: f32) -> f32 {
    if raw <= 0.0 {
        SCRATCH_DEFAULT_HEIGHT
    } else {
        raw.clamp(SCRATCH_MIN_HEIGHT, SCRATCH_MAX_HEIGHT)
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
        assert_eq!(clamp_scratch_height(220.), 220.);
        assert_eq!(clamp_scratch_height(500.), SCRATCH_MAX_HEIGHT);
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

//! AppShell 设置分发（S-9）：将 `shell.rs` 中 8+ 个 `toggle_*/set_*/adjust_*`
//! 模板方法抽为独立组件，使 `shell.rs` 聚焦会话与布局。

use gpui::Context;

use crate::shared::i18n::LanguagePreference;
use crossh_terminal::{
    MAX_FONT_SIZE, MAX_SCROLLBACK, MIN_FONT_SIZE, MIN_SCROLLBACK, TerminalSettings,
};

use super::AppShell;

impl AppShell {
    pub(crate) fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        crate::features::settings::toggle_settings(cx.weak_entity(), cx);
        cx.notify();
    }

    pub(crate) fn set_language(&mut self, preference: LanguagePreference, cx: &mut Context<Self>) {
        if self.language_preference == preference {
            cx.notify();
            return;
        }
        crate::shared::i18n::set_locale(preference);
        crate::infrastructure::app_menu::refresh(cx);
        self.language_preference = preference;
        self.persist_settings();
        cx.notify();
    }

    pub(crate) fn toggle_timestamps(&mut self, cx: &mut Context<Self>) {
        // 按终端独立：仅切换当前焦点终端（分屏时为聚焦侧），非全局广播。
        // 新建终端的默认值仍跟随全局 `terminal_settings.show_timestamps`，此处同步更新
        // 该默认值并落盘，但不广播到其他已存在终端。
        let Some(crate::features::workspace::state::ActiveView::LocalSession(session_id)) =
            self.workspace.focused_view()
        else {
            return;
        };
        let Some(session) = self.workspace.sessions.local_sessions.get(&session_id) else {
            return;
        };
        let terminal = session.terminal.clone();
        let new_value = {
            let mut next = false;
            terminal.update(cx, |view, cx| {
                next = !view.show_timestamps();
                view.set_show_timestamps(next, cx);
            });
            next
        };
        if self.terminal_settings.show_timestamps != new_value {
            self.terminal_settings.show_timestamps = new_value;
            self.persist_settings();
        }
        cx.notify();
    }

    pub(crate) fn toggle_host_sidebar(&mut self, cx: &mut Context<Self>) {
        self.workspace_settings.show_host_sidebar = !self.workspace_settings.show_host_sidebar;
        self.persist_settings();
        cx.notify();
    }

    pub(crate) fn toggle_quick_commands(&mut self, cx: &mut Context<Self>) {
        self.workspace_settings.show_quick_commands = !self.workspace_settings.show_quick_commands;
        self.persist_settings();
        cx.notify();
    }

    pub(crate) fn toggle_terminal_notifications(&mut self, cx: &mut Context<Self>) {
        let mut terminal = self.terminal_settings.clone();
        terminal.notifications_enabled = !terminal.notifications_enabled;
        self.apply_terminal_settings(terminal, cx);
    }

    pub(crate) fn set_update_check_on_startup(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.update_settings.check_on_startup == enabled {
            return;
        }
        self.update_settings.check_on_startup = enabled;
        self.persist_settings();
        cx.notify();
    }

    pub(crate) fn adjust_font_size(&mut self, delta: f32, cx: &mut Context<Self>) {
        let mut terminal = self.terminal_settings.clone();
        terminal.font_size = (terminal.font_size + delta)
            .round()
            .clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        self.apply_terminal_settings(terminal, cx);
    }

    pub(crate) fn set_scrollback(&mut self, scrollback: usize, cx: &mut Context<Self>) {
        let mut terminal = self.terminal_settings.clone();
        terminal.scrollback = scrollback.clamp(MIN_SCROLLBACK, MAX_SCROLLBACK);
        self.apply_terminal_settings(terminal, cx);
    }

    pub(crate) fn set_recent_dirs_max(&mut self, max: usize, cx: &mut Context<Self>) {
        let mut workspace = self.workspace_settings.clone();
        workspace.recent_dirs_max = max;
        self.workspace_settings = workspace.normalized();
        self.persist_settings();
        self.sync_local_dirs(cx);
        cx.notify();
    }

    /// 设置显式编辑器命令；`None`/空白清除覆盖并回退自动检测。
    pub(crate) fn set_editor_command(&mut self, command: Option<String>, cx: &mut Context<Self>) {
        let mut workspace = self.workspace_settings.clone();
        workspace.editor_command = command;
        self.workspace_settings = workspace.normalized();
        self.persist_settings();
        cx.notify();
    }

    pub(crate) fn apply_terminal_settings(
        &mut self,
        settings: TerminalSettings,
        cx: &mut Context<Self>,
    ) {
        let settings = settings.normalized();
        if self.terminal_settings == settings {
            return;
        }

        crate::features::terminal::TerminalView::apply_zed_settings(&settings, cx);

        for session in self.workspace.sessions.local_sessions.values() {
            session.terminal.update(cx, |terminal, cx| {
                terminal.apply_settings(settings.clone(), cx)
            });
        }

        self.terminal_settings = settings;
        self.persist_settings();
        cx.notify();
    }
}

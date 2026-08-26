//! AppShell 设置分发（S-9）：将 `shell.rs` 中 8+ 个 `toggle_*/set_*/adjust_*`
//! 模板方法抽为独立组件，使 `shell.rs` 聚焦会话与布局。

use gpui::Context;

use crate::features::workspace::empty_state::EmptyStateFilter;
use crate::shared::i18n::LanguagePreference;
use crossh_terminal::settings::{
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
        crate::features::settings::locale_state::set_language(cx, preference);
        crate::infrastructure::app_menu::refresh(cx);
        self.language_preference = preference;
        for tab in &self.workspace.sessions.remote_tabs {
            tab.pane.notify_language(cx);
        }
        self.persist_settings();
        cx.notify();
    }

    pub(crate) fn toggle_timestamps(&mut self, cx: &mut Context<Self>) {
        let mut terminal = self.terminal_settings.clone();
        terminal.show_timestamps = !terminal.show_timestamps;
        self.apply_terminal_settings(terminal, cx);
    }

    pub(crate) fn toggle_host_sidebar(&mut self, cx: &mut Context<Self>) {
        self.workspace_settings.show_host_sidebar = !self.workspace_settings.show_host_sidebar;
        self.persist_settings();
        cx.notify();
    }

    pub(crate) fn set_empty_state_filter(
        &mut self,
        filter: EmptyStateFilter,
        cx: &mut Context<Self>,
    ) {
        if self.empty_state_filter != filter {
            self.empty_state_filter = filter;
            cx.notify();
        }
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

        for tab in &self.workspace.sessions.remote_tabs {
            tab.pane.apply_terminal_settings(settings.clone(), cx);
        }
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

//! Workspace-facing abstraction over a terminal pane.
//!
//! Mirrors Zed's `terminal_view::TerminalItem` idea: the workspace talks to the
//! terminal through this trait instead of reaching into `TerminalView`'s
//! inherent API, so a workspace container never has to match on the concrete
//! view type. Only the operations the workspace actually calls are exposed;
//! terminal-internal helpers (`update_timestamp_state`,
//! `set_right_mouse_forwarded`, `take_right_mouse_down`, `handle_right_click`)
//! deliberately stay off the trait.
//!
//! The blanket impl targets `Entity<TerminalView>` because the workspace stores
//! handles, and GPUI's typed render/subscribe need the concrete entity.

use gpui::{App, Entity, SystemNotificationResponse};

use crossh_terminal::{ConnState, TerminalSettings};

use crate::view::TerminalView;

pub trait TerminalItem {
    fn state(&self, cx: &App) -> ConnState;
    fn cwd(&self, cx: &App) -> Option<String>;
    fn title(&self, cx: &App) -> Option<String>;
    fn tab_title(&self, cx: &App, fallback: &str) -> String;
    fn is_command_running(&self, cx: &App) -> bool;
    fn show_timestamps(&self, cx: &App) -> bool;
    fn set_show_timestamps(&self, show: bool, cx: &mut App);
    fn apply_settings(&self, settings: TerminalSettings, cx: &mut App);
    fn request_focus(&self, cx: &mut App);
    fn run_command(&self, command: &str, cx: &mut App);
    fn run_command_without_focus(&self, command: &str, cx: &mut App);
    fn request_close(&self, cx: &mut App);
    fn handle_system_notification_response(
        &self,
        response: &SystemNotificationResponse,
        cx: &mut App,
    ) -> Option<bool>;
}

impl TerminalItem for Entity<TerminalView> {
    fn state(&self, cx: &App) -> ConnState {
        self.read(cx).state.clone()
    }

    fn cwd(&self, cx: &App) -> Option<String> {
        self.read(cx).cwd.clone()
    }

    fn title(&self, cx: &App) -> Option<String> {
        self.read(cx).title().map(str::to_owned)
    }

    fn tab_title(&self, cx: &App, fallback: &str) -> String {
        self.read(cx).tab_title(fallback)
    }

    fn is_command_running(&self, cx: &App) -> bool {
        self.read(cx).is_command_running(cx)
    }

    fn show_timestamps(&self, cx: &App) -> bool {
        self.read(cx).show_timestamps()
    }

    fn set_show_timestamps(&self, show: bool, cx: &mut App) {
        self.update(cx, |terminal, cx| terminal.set_show_timestamps(show, cx));
    }

    fn apply_settings(&self, settings: TerminalSettings, cx: &mut App) {
        self.update(cx, |terminal, cx| terminal.apply_settings(settings, cx));
    }

    fn request_focus(&self, cx: &mut App) {
        self.update(cx, |terminal, _| terminal.request_focus());
    }

    fn run_command(&self, command: &str, cx: &mut App) {
        self.update(cx, |terminal, cx| terminal.run_command(command, cx));
    }

    fn run_command_without_focus(&self, command: &str, cx: &mut App) {
        self.update(cx, |terminal, cx| {
            terminal.run_command_without_focus(command, cx)
        });
    }

    fn request_close(&self, cx: &mut App) {
        self.update(cx, |terminal, cx| terminal.request_close(cx));
    }

    fn handle_system_notification_response(
        &self,
        response: &SystemNotificationResponse,
        cx: &mut App,
    ) -> Option<bool> {
        self.update(cx, |terminal, cx| {
            terminal.handle_system_notification_response(response, cx)
        })
    }
}

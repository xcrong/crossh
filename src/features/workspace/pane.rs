//! Common pane boundary consumed by the workspace container.

use gpui::{AnyElement, App, EntityId, SystemNotificationResponse};

use crossh_terminal::settings::TerminalSettings;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerminalPaneInfo {
    pub(crate) low_latency_enabled: bool,
    pub(crate) low_latency_available: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PaneRisk {
    pub(crate) sftp_writes: usize,
    pub(crate) unsaved_editors: usize,
    pub(crate) active_forwards: usize,
}

/// The workspace knows only these capabilities; pane implementations own the
/// concrete GPUI entities and feature-specific behavior.
pub(crate) trait WorkspacePane {
    fn render(&self) -> AnyElement;
    fn title(&self, cx: &App) -> String;
    fn terminal_info(&self, cx: &App) -> Option<TerminalPaneInfo>;
    fn terminal_entity_id(&self) -> Option<EntityId>;
    fn cwd(&self, cx: &App) -> Option<String>;
    fn is_command_running(&self, cx: &App) -> bool;
    fn toggle_low_latency(&self, cx: &mut App);
    fn run_command(&self, command: &str, cx: &mut App);
    fn send_text(&self, _text: &str, _cx: &mut App) {}
    fn set_adjacent_terminal_available(&self, _available: bool, _cx: &mut App) {}
    fn handle_system_notification_response(
        &self,
        response: &SystemNotificationResponse,
        cx: &mut App,
    ) -> Option<bool>;
    fn request_focus(&self, cx: &mut App);
    fn request_close(&self, cx: &mut App);
    /// Release feature-owned resources when a remote tab is removed.
    fn cleanup(&self, _cx: &mut App) {}
    fn apply_terminal_settings(&self, settings: TerminalSettings, cx: &mut App);
    fn notify_language(&self, cx: &mut App);
    fn risk(&self, cx: &App) -> PaneRisk;
}

//! Common pane boundary consumed by the workspace container.

use gpui::{AnyElement, App, EntityId, SystemNotificationResponse};

use crossh_terminal::TerminalSettings;

// 保留：本地优先下仅单 pane，但抽象为未来 pane 类型保留
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PaneRisk {
    pub(crate) unsaved_editors: usize,
}

// 保留：本地优先下仅单 pane，但抽象为未来 pane 类型保留
#[allow(dead_code)]
/// The workspace knows only these capabilities; pane implementations own the
/// concrete GPUI entities and feature-specific behavior.
pub(crate) trait WorkspacePane {
    fn render(&self) -> AnyElement;
    fn title(&self, cx: &App) -> String;
    fn terminal_entity_id(&self) -> Option<EntityId>;
    fn cwd(&self, cx: &App) -> Option<String>;
    fn is_command_running(&self, cx: &App) -> bool;
    fn run_command(&self, command: &str, cx: &mut App);
    fn run_command_without_focus(&self, command: &str, cx: &mut App) {
        // 默认回退到普通发送；终端 pane 会覆盖为无焦点抢占的版本
        self.run_command(command, cx);
    }
    fn send_text(&self, _text: &str, _cx: &mut App) {}
    fn set_adjacent_terminal_available(&self, _available: bool, _cx: &mut App) {}
    fn handle_system_notification_response(
        &self,
        response: &SystemNotificationResponse,
        cx: &mut App,
    ) -> Option<bool>;
    fn request_focus(&self, cx: &mut App);
    fn request_close(&self, _cx: &mut App) {}
    /// Release feature-owned resources when a pane is removed.
    fn cleanup(&self, _cx: &mut App) {}
    fn apply_terminal_settings(&self, _settings: TerminalSettings, _cx: &mut App) {}
    fn notify_language(&self, _cx: &mut App) {}
    fn risk(&self, cx: &App) -> PaneRisk;
}

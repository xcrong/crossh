//! Crossh's thin host for the Zed terminal-view foundation.
//!
//! The terminal emulator, PTY lifecycle, resize protocol, mouse handling,
//! selection, IME, scrolling, and painting are owned by Zed's terminal crate
//! plus the local terminal element fork. This module only supplies the host
//! entity and focus/event wiring for the workspace.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{
    Action, AnyElement, App, AppContext, Bounds, ClipboardEntry, Context, Entity, EntityId,
    EventEmitter, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, Keystroke,
    ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled, Subscription,
    SystemNotification, SystemNotificationResponse, Task, Window, div, point, px, size,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task::Shell;
use terminal as zed_terminal;
use terminal::terminal_settings::CursorShape;
use theme::ActiveTheme;

use crate::shared::i18n;
use crossh_core::terminal::{
    LocalShellEnvironment, ShellCommandMarker, ShellPromptMarker, command_marker_from_title,
    local_terminal_tab_title, local_terminal_title, prompt_marker_from_title,
    strip_shell_host_prefix,
};
use crossh_terminal::timestamps::{TerminalRow, TerminalTimestampState, timestamp_now};
use crossh_terminal::{ConnState, TerminalEvent, TerminalSettings};

#[path = "zed_view/terminal_element.rs"]
mod terminal_element;
use terminal_element::TerminalElement;

/// Sends the specified text directly to the terminal.
///
/// Mirrors Zed's `terminal::SendText` action, which its default keymap uses
/// for word-wise navigation and word deletion (e.g. `alt-left` sends `\x1bb`).
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Action)]
#[action(namespace = terminal)]
pub struct SendText(pub String);

/// Sends a keystroke sequence to the terminal.
///
/// Mirrors Zed's `terminal::SendKeystroke` action used by its default keymap
/// for line editing conveniences (e.g. `cmd-backspace` sends `ctrl-u`).
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Action)]
#[action(namespace = terminal)]
pub struct SendKeystroke(pub String);

const INITIAL_COLUMNS: usize = 100;
const INITIAL_ROWS: usize = 30;
const DEFAULT_CELL_WIDTH: f32 = 8.0;
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(530);
const TERMINAL_NOTIFICATION_TAG_PREFIX: &str = "crossh-terminal";

fn terminal_notification_tag(entity_id: EntityId, serial: u64) -> String {
    format!("{TERMINAL_NOTIFICATION_TAG_PREFIX}-{entity_id}-bell-{serial}")
}

fn terminal_notification_tag_matches(entity_id: EntityId, tag: &str) -> bool {
    tag.strip_prefix(TERMINAL_NOTIFICATION_TAG_PREFIX)
        .and_then(|tag| tag.strip_prefix('-'))
        .and_then(|tag| tag.strip_prefix(&entity_id.to_string()))
        .is_some_and(|tag| {
            tag.strip_prefix("-bell-").is_some_and(|serial| {
                !serial.is_empty() && serial.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
}

fn should_show_terminal_notification(enabled: bool, focused: bool) -> bool {
    enabled && !focused
}

fn terminal_bounds_for_grid(
    columns: usize,
    rows: usize,
    cell_width: Pixels,
    line_height: Pixels,
) -> zed_terminal::TerminalBounds {
    zed_terminal::TerminalBounds::new(
        line_height.max(px(1.)),
        cell_width.max(px(1.)),
        Bounds {
            origin: point(px(0.), px(0.)),
            size: size(
                cell_width.max(px(1.)) * columns as f32,
                line_height.max(px(1.)) * rows as f32,
            ),
        },
    )
}

/// 右键应复制（true）还是粘贴（false）：仅当存在非空选中文本时复制。
fn has_copyable_selection(has_selection: bool, selection_text: Option<&str>) -> bool {
    has_selection && selection_text.is_some_and(|text| !text.is_empty())
}

fn status_view(message: &str, focus: &FocusHandle) -> AnyElement {
    div()
        .id("terminal-status")
        .size_full()
        .track_focus(focus)
        .flex()
        .items_center()
        .justify_center()
        .text_sm()
        .child(SharedString::from(message.to_owned()))
        .into_any_element()
}

fn shell_lifecycle_markers(
    event: &zed_terminal::Event,
    breadcrumb: &str,
) -> (Option<ShellCommandMarker>, Option<ShellPromptMarker>) {
    if !matches!(event, zed_terminal::Event::BreadcrumbsChanged) {
        return (None, None);
    }

    (
        command_marker_from_title(breadcrumb),
        prompt_marker_from_title(breadcrumb),
    )
}

fn open_navigation_target(target: &zed_terminal::MaybeNavigationTarget, cx: &mut App) {
    if let zed_terminal::MaybeNavigationTarget::Url(url) = target {
        cx.open_url(url);
    }
}

impl EventEmitter<TerminalEvent> for TerminalView {}

pub struct TerminalView {
    /// The display-only entity is replaced by the PTY-backed entity once the
    /// Zed builder finishes. The element reads this field directly.
    pub(crate) zed_terminal: Entity<zed_terminal::Terminal>,
    pending_builder: Option<zed_terminal::TerminalBuilder>,
    builder_error: Option<String>,
    builder_task: Option<Task<()>>,
    shell_environment: Option<LocalShellEnvironment>,
    blink_task: Option<Task<()>>,
    terminal_subscription: Option<Subscription>,
    focus: FocusHandle,
    focus_in_subscription: Option<Subscription>,
    focus_out_subscription: Option<Subscription>,
    focused_once: bool,
    focused: bool,
    cursor_blink_on: bool,
    cursor_blink_pause_until: Instant,
    blinking_terminal_enabled: bool,
    cursor_shape: CursorShape,
    pub state: ConnState,
    pub cwd: Option<String>,
    title: Option<String>,
    ime_marked_text: String,
    /// Set when the terminal emitted a bell since the last frame; consumed by
    /// the renderer so the system bell fires exactly once per event.
    bell_pending: bool,
    /// Whether the current right-button press was forwarded to the PTY.
    right_mouse_down: bool,
    show_timestamps: bool,
    notifications_enabled: bool,
    notification_serial: u64,
    timestamp_state: TerminalTimestampState,
    pending_timestamp: Option<String>,
}

impl TerminalView {
    /// Apply the small Crossh settings surface to the forked terminal settings.
    ///
    /// The renderer reads the global `terminal::terminal_settings::TerminalSettings`
    /// exactly as Zed's TerminalElement did, so there is one font / scrollback
    /// configuration path.
    pub fn apply_zed_settings(settings: &TerminalSettings, cx: &mut App) {
        let zed_settings = terminal::terminal_settings::TerminalSettings::from_crossh(settings);
        cx.set_global(zed_settings);
    }

    pub fn from_local_zed(cwd: PathBuf, settings: TerminalSettings, cx: &mut App) -> Entity<Self> {
        let initial_cwd = cwd.to_string_lossy().into_owned();
        let system_shell = util::shell::get_system_shell();
        let shell_environment = match LocalShellEnvironment::create(&system_shell) {
            Ok(environment) => environment,
            Err(error) => {
                log::warn!("failed to prepare local shell integration: {error}");
                None
            }
        };
        let shell = shell_environment
            .as_ref()
            .map_or(Shell::System, |environment| {
                if environment.use_system_shell() {
                    return Shell::System;
                }
                Shell::WithArguments {
                    program: environment.program().to_string(),
                    args: environment.args().to_vec(),
                    title_override: None,
                }
            });
        Self::from_zed_shell_with_environment(
            Some(cwd),
            Some(initial_cwd),
            shell,
            settings,
            shell_environment,
            cx,
        )
    }

    fn from_zed_shell_with_environment(
        working_directory: Option<PathBuf>,
        initial_cwd: Option<String>,
        shell: Shell,
        settings: TerminalSettings,
        shell_environment: Option<LocalShellEnvironment>,
        cx: &mut App,
    ) -> Entity<Self> {
        let settings = settings.normalized();
        Self::apply_zed_settings(&settings, cx);
        let zed_settings = terminal::terminal_settings::TerminalSettings::get_global(cx).clone();

        let display_builder = zed_terminal::TerminalBuilder::new_display_only_with_bounds(
            zed_settings.cursor_shape,
            zed_settings.alternate_scroll,
            Some(settings.scrollback),
            0,
            cx.background_executor(),
            util::paths::PathStyle::local(),
            terminal_bounds_for_grid(
                INITIAL_COLUMNS,
                INITIAL_ROWS,
                px(DEFAULT_CELL_WIDTH),
                px(settings.font_size * zed_settings.line_height.value()),
            ),
        );

        let shell_env = shell_environment
            .as_ref()
            .map(|environment| environment.env().iter().cloned().collect())
            .unwrap_or_default();
        let builder = zed_terminal::TerminalBuilder::new(
            working_directory,
            zed_terminal::TerminalMode::interactive(),
            shell,
            shell_env,
            zed_settings.cursor_shape,
            zed_settings.alternate_scroll,
            Some(settings.scrollback),
            Vec::new(),
            Duration::from_millis(0),
            false,
            0,
            cx,
            Vec::new(),
            util::paths::PathStyle::local(),
        );

        let entity = cx.new(|cx| {
            let zed_terminal = cx.new(|terminal_cx| display_builder.subscribe(terminal_cx));
            Self {
                zed_terminal,
                pending_builder: None,
                builder_error: None,
                builder_task: None,
                shell_environment,
                blink_task: None,
                terminal_subscription: None,
                focus: cx.focus_handle(),
                focus_in_subscription: None,
                focus_out_subscription: None,
                focused_once: false,
                focused: false,
                cursor_blink_on: true,
                cursor_blink_pause_until: Instant::now(),
                blinking_terminal_enabled: false,
                cursor_shape: zed_settings.cursor_shape,
                state: ConnState::default(),
                cwd: initial_cwd,
                title: None,
                ime_marked_text: String::new(),
                bell_pending: false,
                right_mouse_down: false,
                show_timestamps: settings.show_timestamps,
                notifications_enabled: settings.notifications_enabled,
                notification_serial: 0,
                timestamp_state: TerminalTimestampState::default(),
                pending_timestamp: None,
            }
        });

        let weak = entity.downgrade();
        let builder_task = cx.spawn(async move |cx| match builder.await {
            Ok(builder) => {
                let _ = weak.update(cx, |this, cx| {
                    this.pending_builder = Some(builder);
                    cx.notify();
                });
            }
            Err(error) => {
                let message = error.to_string();
                let _ = weak.update(cx, |this, cx| {
                    this.builder_error = Some(message.clone());
                    this.shell_environment = None;
                    this.state = ConnState::Error(message);
                    cx.notify();
                });
            }
        });

        let weak = entity.downgrade();
        let blink_task = cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(CURSOR_BLINK_INTERVAL).await;
                if weak
                    .update(cx, |this, cx| {
                        if this.focused
                            && this.blinking_terminal_enabled
                            && Instant::now() >= this.cursor_blink_pause_until
                        {
                            this.cursor_blink_on = !this.cursor_blink_on;
                        } else {
                            this.cursor_blink_on = true;
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        entity.update(cx, |this, _| {
            this.builder_task = Some(builder_task);
            this.blink_task = Some(blink_task);
        });
        entity
    }

    fn attach_terminal(
        &mut self,
        terminal: Entity<zed_terminal::Terminal>,
        cx: &mut Context<Self>,
    ) {
        let subscription = cx.subscribe(&terminal, |this, terminal, event, cx| {
            let (title, cwd, breadcrumb) = {
                let terminal = terminal.read(cx);
                (
                    terminal.title(false),
                    terminal
                        .working_directory()
                        .map(|path| path.to_string_lossy().into_owned()),
                    terminal.breadcrumb_text.clone(),
                )
            };

            let (command_marker, prompt_marker) = shell_lifecycle_markers(event, &breadcrumb);
            let internal_marker = command_marker.is_some() || prompt_marker.is_some();

            if !internal_marker && this.title.as_deref() != Some(title.as_str()) {
                this.title = Some(title);
                cx.emit(TerminalEvent::TitleChanged);
            }

            let reported_cwd = command_marker
                .as_ref()
                .map(|marker| &marker.cwd)
                .or_else(|| prompt_marker.as_ref().map(|marker| &marker.cwd))
                .or(cwd.as_ref());
            if let Some(cwd) = reported_cwd
                && this.cwd.as_deref() != Some(cwd.as_str())
            {
                this.cwd = Some(cwd.clone());
                cx.emit(TerminalEvent::CwdChanged);
            }

            if let Some(marker) = command_marker {
                cx.emit(TerminalEvent::CommandStarted {
                    command: marker.command,
                    cwd: Some(marker.cwd),
                });
            }
            if let Some(marker) = prompt_marker {
                cx.emit(TerminalEvent::CommandFinished {
                    status: Some(marker.status),
                });
                cx.emit(TerminalEvent::PromptReached);
            }

            match event {
                zed_terminal::Event::CloseTerminal => {
                    if this.state != ConnState::Closed {
                        this.shell_environment = None;
                        this.state = ConnState::Closed;
                        cx.emit(TerminalEvent::Closed);
                    }
                }
                zed_terminal::Event::Bell => {
                    this.bell_pending = true;
                    this.notify_bell(cx);
                }
                zed_terminal::Event::TitleChanged | zed_terminal::Event::BreadcrumbsChanged => {
                    if !internal_marker {
                        cx.emit(TerminalEvent::TitleChanged);
                    }
                }
                zed_terminal::Event::BlinkChanged(blinking) => {
                    this.blinking_terminal_enabled = *blinking;
                    if !*blinking {
                        this.cursor_blink_on = true;
                    }
                }
                zed_terminal::Event::Wakeup => {
                    this.pending_timestamp = Some(timestamp_now());
                }
                zed_terminal::Event::Open(target) => open_navigation_target(target, cx),
                zed_terminal::Event::SelectionsChanged
                | zed_terminal::Event::NewNavigationTarget(_) => {}
            }
            cx.notify();
        });

        self.title = Some(terminal.read(cx).title(false));
        self.zed_terminal = terminal;
        self.terminal_subscription = Some(subscription);
        self.builder_task = None;
        self.builder_error = None;
        self.focused_once = false;
        self.state = ConnState::Connected;
        self.pending_timestamp = Some(timestamp_now());
    }

    fn process_keystroke(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let (handled, vi_mode_enabled) = self.zed_terminal.update(cx, |terminal, cx| {
            (
                terminal.try_keystroke(
                    keystroke,
                    terminal::terminal_settings::TerminalSettings::get_global(cx).option_as_meta,
                ),
                terminal.vi_mode_enabled(),
            )
        });
        if handled && vi_mode_enabled {
            cx.notify();
        }
        handled
    }

    fn clear(&mut self, _: &zed_terminal::Clear, _: &mut Window, cx: &mut Context<Self>) {
        self.zed_terminal.update(cx, |terminal, _| terminal.clear());
        cx.notify();
    }

    fn copy(&mut self, _: &zed_terminal::Copy, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(cx);
        cx.notify();
    }

    fn paste(&mut self, _: &zed_terminal::Paste, _: &mut Window, cx: &mut Context<Self>) {
        self.paste_clipboard(cx);
    }

    fn paste_text(&mut self, _: &zed_terminal::PasteText, _: &mut Window, cx: &mut Context<Self>) {
        self.paste_text_clipboard(cx);
    }

    fn select_all(&mut self, _: &zed_terminal::SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.select_all_terminal(cx);
        cx.notify();
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        self.zed_terminal
            .update(cx, |terminal, _| terminal.copy(None));
    }

    fn paste_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(clipboard) = cx.read_from_clipboard() else {
            return;
        };

        match clipboard.entries().first() {
            Some(ClipboardEntry::Image(image)) if !image.bytes.is_empty() => {
                self.forward_ctrl_v(cx);
            }
            Some(ClipboardEntry::ExternalPaths(paths)) => {
                self.add_paths_to_terminal(paths.paths(), cx);
            }
            _ => {
                self.paste_text_clipboard(cx);
            }
        }
    }

    /// Pastes only the textual representation of the clipboard, mirroring
    /// Zed's `PasteText` action. Unlike `paste_clipboard`, images and
    /// external paths are ignored.
    fn paste_text_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(clipboard) = cx.read_from_clipboard() else {
            return;
        };
        if let Some(text) = clipboard.text() {
            self.zed_terminal
                .update(cx, |terminal, _| terminal.paste(&text));
        }
    }

    /// Emits a raw Ctrl+V so TUI agents can read the OS clipboard directly
    /// and attach images using their native workflows.
    fn forward_ctrl_v(&mut self, cx: &mut Context<Self>) {
        self.zed_terminal
            .update(cx, |terminal, _| terminal.input(vec![0x16]));
    }

    /// Pastes external file paths as shell-quoted arguments, mirroring Zed's
    /// terminal view behavior.
    fn add_paths_to_terminal(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let mut text = paths
            .iter()
            .filter_map(|path| Some(format!(" {}", shlex::try_quote(path.to_str()?).ok()?)))
            .collect::<String>();
        text.push(' ');
        self.zed_terminal
            .update(cx, |terminal, _| terminal.paste(&text));
    }

    fn send_text(&mut self, text: &SendText, _: &mut Window, cx: &mut Context<Self>) {
        self.send_input(text.0.clone().into_bytes(), cx);
    }

    fn send_keystroke(&mut self, text: &SendKeystroke, _: &mut Window, cx: &mut Context<Self>) {
        if let Ok(keystroke) = Keystroke::parse(&text.0) {
            self.process_keystroke(&keystroke, cx);
        }
    }

    fn select_all_terminal(&mut self, cx: &mut Context<Self>) {
        self.zed_terminal
            .update(cx, |terminal, _| terminal.select_all());
    }

    fn scroll_line_up(
        &mut self,
        _: &zed_terminal::ScrollLineUp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_alt_screen(cx) {
            cx.propagate();
            return;
        }
        self.zed_terminal
            .update(cx, |terminal, _| terminal.scroll_line_up());
        cx.notify();
    }

    fn scroll_line_down(
        &mut self,
        _: &zed_terminal::ScrollLineDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_alt_screen(cx) {
            cx.propagate();
            return;
        }
        self.zed_terminal
            .update(cx, |terminal, _| terminal.scroll_line_down());
        cx.notify();
    }

    fn scroll_page_up(
        &mut self,
        _: &zed_terminal::ScrollPageUp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_alt_screen(cx) {
            cx.propagate();
            return;
        }
        self.zed_terminal
            .update(cx, |terminal, _| terminal.scroll_page_up());
        cx.notify();
    }

    fn scroll_page_down(
        &mut self,
        _: &zed_terminal::ScrollPageDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_alt_screen(cx) {
            cx.propagate();
            return;
        }
        self.zed_terminal
            .update(cx, |terminal, _| terminal.scroll_page_down());
        cx.notify();
    }

    fn scroll_to_top(
        &mut self,
        _: &zed_terminal::ScrollToTop,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_alt_screen(cx) {
            cx.propagate();
            return;
        }
        self.zed_terminal
            .update(cx, |terminal, _| terminal.scroll_to_top());
        cx.notify();
    }

    fn scroll_to_bottom(
        &mut self,
        _: &zed_terminal::ScrollToBottom,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_alt_screen(cx) {
            cx.propagate();
            return;
        }
        self.zed_terminal
            .update(cx, |terminal, _| terminal.scroll_to_bottom());
        cx.notify();
    }

    fn toggle_vi_mode(
        &mut self,
        _: &zed_terminal::ToggleViMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.zed_terminal
            .update(cx, |terminal, _| terminal.toggle_vi_mode());
        cx.notify();
    }

    fn is_alt_screen(&self, cx: &App) -> bool {
        self.zed_terminal
            .read(cx)
            .last_content()
            .mode
            .contains(zed_terminal::Modes::ALT_SCREEN)
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.cursor_blink_on = true;
        self.cursor_blink_pause_until = Instant::now() + Duration::from_millis(500);
        if self.process_keystroke(&event.keystroke, cx) {
            cx.stop_propagation();
        }
        window.refresh();
    }

    fn focus_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focused = true;
        self.cursor_blink_on = true;
        self.cursor_blink_pause_until = Instant::now() + Duration::from_millis(500);
        self.zed_terminal.update(cx, |terminal, _| {
            terminal.set_cursor_shape(self.cursor_shape);
            terminal.focus_in();
        });
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn focus_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focused = false;
        self.cursor_blink_on = true;
        self.right_mouse_down = false;
        self.zed_terminal.update(cx, |terminal, _| {
            terminal.focus_out();
            terminal.set_cursor_shape(CursorShape::Hollow);
        });
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn notify_bell(&mut self, cx: &mut Context<Self>) {
        cx.emit(TerminalEvent::Notification);
        if !should_show_terminal_notification(self.notifications_enabled, self.focused) {
            return;
        }

        let tag = terminal_notification_tag(cx.entity_id(), self.notification_serial);
        self.notification_serial = self.notification_serial.wrapping_add(1);
        cx.show_system_notification(SystemNotification {
            tag: tag.into(),
            title: self.tab_title("Terminal").into(),
            body: i18n::text("terminal.bell").into(),
            actions: Vec::new(),
        });
    }

    pub(crate) fn request_focus(&mut self) {
        self.focused_once = false;
    }

    fn send_input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        if bytes.is_empty() {
            return;
        }
        self.zed_terminal
            .update(cx, |terminal, _| terminal.input(bytes));
    }

    pub(crate) fn run_command(&mut self, command: &str, cx: &mut Context<Self>) {
        let command = command.trim();
        if command.is_empty() {
            return;
        }
        self.ime_marked_text.clear();
        self.send_input(format!("{command}\r").into_bytes(), cx);
        self.request_focus();
    }

    pub(crate) fn run_command_without_focus(&mut self, command: &str, cx: &mut Context<Self>) {
        let command = command.trim();
        if command.is_empty() {
            return;
        }
        self.ime_marked_text.clear();
        self.send_input(format!("{command}\r").into_bytes(), cx);
        // 不请求焦点，保持外层输入框（compose 等批量输入）持有焦点
    }

    pub(crate) fn request_close(&mut self, cx: &mut Context<Self>) {
        if self.state == ConnState::Closed {
            return;
        }
        self.state = ConnState::Closed;
        self.zed_terminal
            .update(cx, |terminal, _| terminal.input(vec![0x04]));
        cx.notify();
    }

    pub(crate) fn update_timestamp_state(
        &mut self,
        rows: &[TerminalRow],
        display_offset: usize,
        cursor_row: Option<usize>,
        alternate_screen: bool,
    ) -> (bool, Vec<Option<String>>) {
        let timestamp = self.pending_timestamp.take();
        let timestamps = self.timestamp_state.observe(
            rows,
            display_offset,
            cursor_row,
            timestamp,
            alternate_screen,
        );
        (self.show_timestamps, timestamps)
    }

    pub(crate) fn show_timestamps(&self) -> bool {
        self.show_timestamps
    }

    pub(crate) fn set_show_timestamps(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.show_timestamps != show {
            self.show_timestamps = show;
            cx.notify();
        }
    }

    pub(crate) fn apply_settings(&mut self, settings: TerminalSettings, cx: &mut Context<Self>) {
        // `show_timestamps` is per-terminal (per `TerminalView`) since split panes must be
        // independently toggleable. Global `TerminalSettings` only provides the default for
        // new terminals and for font/scrollback propagated via Zed's global settings.
        self.notifications_enabled = settings.notifications_enabled;
        // The workspace updates Zed's global font/scrollback settings before
        // calling this method. Host-only settings are applied above.
        cx.notify();
    }

    /// 记录当前右键按下是否已转发给 PTY（应用接管鼠标时），供对称的弹起处理消费。
    pub(crate) fn set_right_mouse_forwarded(&mut self, forwarded: bool) {
        self.right_mouse_down = forwarded;
    }

    pub(crate) fn take_right_mouse_down(&mut self) -> bool {
        let forwarded = self.right_mouse_down;
        self.right_mouse_down = false;
        forwarded
    }

    /// Windows Terminal 风格右键：有选区则复制，无选区则粘贴。
    /// 不弹菜单，也不会为凑复制条件而合成词选中。
    pub(crate) fn handle_right_click(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let has_selection = {
            let terminal = self.zed_terminal.read(cx);
            let content = terminal.last_content();
            has_copyable_selection(
                content.selection.is_some(),
                content.selection_text.as_deref(),
            )
        };
        if has_selection {
            self.copy_selection(cx);
        } else if self.state == ConnState::Connected {
            self.paste_clipboard(cx);
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(crate) fn is_command_running(&self, cx: &App) -> bool {
        self.state == ConnState::Connected
            && self
                .zed_terminal
                .read(cx)
                .last_content()
                .mode
                .contains(zed_terminal::Modes::ALT_SCREEN)
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn tab_title(&self, fallback: &str) -> String {
        if let Some(title) = self.title().filter(|title| !title.trim().is_empty()) {
            let title = strip_shell_host_prefix(title);
            return local_terminal_tab_title(title, self.cwd.as_deref());
        }

        let title = local_terminal_title(
            self.cwd.as_deref(),
            None,
            Some(util::shell::get_system_shell().as_str()),
        );
        if title != "Terminal" {
            return title;
        }

        fallback.to_owned()
    }

    pub(crate) fn handle_system_notification_response(
        &mut self,
        response: &SystemNotificationResponse,
        cx: &mut Context<Self>,
    ) -> Option<bool> {
        if !terminal_notification_tag_matches(cx.entity_id(), response.tag.as_ref()) {
            return None;
        }
        cx.dismiss_system_notification(response.tag.as_ref());
        Some(true)
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.bell_pending {
            self.bell_pending = false;
            // crossh: always play system bell for now, no Zed settings
            window.play_system_bell();
        }

        if let Some(builder) = self.pending_builder.take() {
            if self.state == ConnState::Closed {
                self.builder_task = None;
                drop(builder);
            } else {
                let terminal = cx.new(|terminal_cx| builder.subscribe(terminal_cx));
                self.attach_terminal(terminal, cx);
            }
        }

        if self.builder_task.is_some() {
            let message = self
                .builder_error
                .clone()
                .unwrap_or_else(|| i18n::text("terminal.connecting"));
            return status_view(&message, &self.focus);
        }

        if self.focus_in_subscription.is_none() {
            let focus = self.focus.clone();
            self.focus_in_subscription =
                Some(cx.on_focus_in(&focus, window, |this, window, cx| {
                    this.focus_in(window, cx);
                }));
            let focus = self.focus.clone();
            self.focus_out_subscription =
                Some(cx.on_focus_out(&focus, window, |this, _event, window, cx| {
                    this.focus_out(window, cx);
                }));
        }

        if !self.focused_once && !matches!(self.state, ConnState::Error(_) | ConnState::Closed) {
            self.focus.focus(window, cx);
            self.focused_once = true;
        }

        let cursor_visible = {
            let terminal = self.zed_terminal.read(cx);
            let content = terminal.last_content();
            (!self.focused || !self.blinking_terminal_enabled || self.cursor_blink_on)
                && content.mode.contains(zed_terminal::Modes::SHOW_CURSOR)
                && content.cursor.shape != zed_terminal::CursorShape::Hidden
        };
        let background = cx.theme().colors().terminal_background;
        let focus = self.focus.clone();
        let terminal_element = TerminalElement::new(
            self.zed_terminal.clone(),
            cx.entity(),
            focus.clone(),
            self.focused,
            cursor_visible,
        );

        let mut root = div()
            .id("terminal-view")
            .size_full()
            .relative()
            .bg(background)
            .track_focus(&focus)
            .key_context("Terminal")
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::paste_text))
            .on_action(cx.listener(Self::clear))
            .on_action(cx.listener(Self::scroll_line_up))
            .on_action(cx.listener(Self::scroll_line_down))
            .on_action(cx.listener(Self::scroll_page_up))
            .on_action(cx.listener(Self::scroll_page_down))
            .on_action(cx.listener(Self::scroll_to_top))
            .on_action(cx.listener(Self::scroll_to_bottom))
            .on_action(cx.listener(Self::toggle_vi_mode))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::send_text))
            .on_action(cx.listener(Self::send_keystroke))
            .on_key_down(cx.listener(Self::key_down))
            .on_click({
                let focus = focus.clone();
                move |_event, window, cx| window.focus(&focus, cx)
            })
            .child(
                div()
                    .id("terminal-view-container")
                    .size_full()
                    .p(px(8.))
                    .child(terminal_element),
            );

        if let Some(message) = match &self.state {
            ConnState::Error(message) => Some(message.clone()),
            ConnState::Closed => Some(i18n::text("terminal.closed")),
            ConnState::Connecting | ConnState::Connected => None,
        } {
            root = root.child(
                div()
                    .absolute()
                    .top_2()
                    .left_2()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .child(SharedString::from(message)),
            );
        }

        root.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_click_copies_only_with_non_empty_selection() {
        assert!(has_copyable_selection(true, Some("ls -la")));
        assert!(!has_copyable_selection(false, None));
        assert!(!has_copyable_selection(false, Some("stale")));
        assert!(!has_copyable_selection(true, None));
        assert!(!has_copyable_selection(true, Some("")));
    }

    #[gpui::test]
    fn terminal_url_navigation_uses_platform_opener(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            open_navigation_target(
                &zed_terminal::MaybeNavigationTarget::Url("https://example.com/docs".into()),
                cx,
            );
        });

        assert_eq!(cx.opened_url().as_deref(), Some("https://example.com/docs"));
    }

    #[gpui::test]
    fn terminal_path_navigation_does_not_use_url_opener(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            open_navigation_target(
                &zed_terminal::MaybeNavigationTarget::PathLike(zed_terminal::PathLikeTarget {
                    maybe_path: "/tmp/example.rs:12".into(),
                    working_directory: Some(PathBuf::from("/tmp")),
                }),
                cx,
            );
        });

        assert_eq!(cx.opened_url(), None);
    }

    #[test]
    fn lifecycle_markers_are_only_consumed_for_breadcrumb_events() {
        let breadcrumb = "crossh-command=ZWNobyBoaQ==:L3RtcA==";

        let (command, prompt) =
            shell_lifecycle_markers(&zed_terminal::Event::BreadcrumbsChanged, breadcrumb);
        assert_eq!(command.unwrap().command, "echo hi");
        assert!(prompt.is_none());

        for event in [
            zed_terminal::Event::Wakeup,
            zed_terminal::Event::Bell,
            zed_terminal::Event::BlinkChanged(true),
            zed_terminal::Event::SelectionsChanged,
            zed_terminal::Event::TitleChanged,
        ] {
            let (command, prompt) = shell_lifecycle_markers(&event, breadcrumb);
            assert!(command.is_none());
            assert!(prompt.is_none());
        }
    }

    #[test]
    fn lifecycle_markers_include_remote_working_directory() {
        let breadcrumb = "crossh-command=ZWNobyBoaQ==:L3RtcC9yZW1vdGU=";
        let (command, _) =
            shell_lifecycle_markers(&zed_terminal::Event::BreadcrumbsChanged, breadcrumb);

        assert_eq!(command.unwrap().cwd, "/tmp/remote");
    }

    #[test]
    fn terminal_notifications_respect_settings_and_focus() {
        assert!(should_show_terminal_notification(true, false));
        assert!(!should_show_terminal_notification(false, false));
        assert!(!should_show_terminal_notification(true, true));
    }

    #[test]
    fn terminal_notification_tags_are_scoped_to_the_source_entity() {
        let source = EntityId::from(7);
        let tag = terminal_notification_tag(source, 42);

        assert_eq!(tag, format!("crossh-terminal-{source}-bell-42"));
        assert!(terminal_notification_tag_matches(source, &tag));
        assert!(!terminal_notification_tag_matches(EntityId::from(8), &tag));
        let invalid_serial = format!("crossh-terminal-{source}-bell-invalid");
        assert!(!terminal_notification_tag_matches(source, &invalid_serial));
    }
}

//! Crossh's thin host for the Zed terminal-view foundation.
//!
//! The terminal emulator, PTY lifecycle, resize protocol, mouse handling,
//! selection, IME, scrolling, and painting are owned by Zed's terminal crate
//! plus the local terminal element fork. This module only supplies the host
//! entity, focus/event wiring, and the workspace pane boundary.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Entity, EventEmitter, FocusHandle,
    InteractiveElement, IntoElement, KeyDownEvent, Keystroke, MouseDownEvent, ParentElement,
    Pixels, Render, SharedString, StatefulInteractiveElement, Styled, Subscription,
    SystemNotificationResponse, Task, Window, canvas, div, point, px, size,
};
use settings::Settings as _;
use task::Shell;
use terminal as zed_terminal;
use terminal::terminal_settings::{
    AlternateScroll, CursorShape, TerminalSettings as ZedTerminalSettings,
};
use theme::ActiveTheme;

use crate::features::workspace::pane::{PaneRisk, TerminalPaneInfo, WorkspacePane};
use crate::shared::i18n;
use crossh_core::terminal::{
    LocalShellEnvironment, ShellCommandMarker, ShellPromptMarker, command_marker_from_title,
    local_terminal_tab_title, local_terminal_title, prompt_marker_from_title,
    remote_terminal_title, strip_shell_host_prefix, truncate_path_title,
};
use crossh_terminal::settings::TerminalSettings;
use crossh_terminal::timestamps::{TerminalRow, TerminalTimestampState, timestamp_now};
use crossh_ui::context_menu::{
    CONTEXT_MENU_WIDTH, ContextMenuState, clamp_menu_position, estimate_menu_height,
    render_context_menu,
};

use crossh_terminal::events::{ConnState, TerminalEvent};

use super::context_menu::{TerminalMenuAction, menu_entries};

#[path = "zed_view/terminal_element.rs"]
mod terminal_element;
use terminal_element::TerminalElement;

const INITIAL_COLUMNS: usize = 100;
const INITIAL_ROWS: usize = 30;
const DEFAULT_CELL_WIDTH: f32 = 8.0;
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(530);

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
    is_local: bool,
    ime_marked_text: String,
    context_menu: Option<ContextMenuState<TerminalMenuAction>>,
    /// Whether the current right-button press was forwarded to the PTY.
    right_mouse_down: bool,
    /// The terminal view origin in window coordinates, used to place the menu.
    anchor_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    show_timestamps: bool,
    timestamp_state: TerminalTimestampState,
    pending_timestamp: Option<String>,
}

impl TerminalView {
    /// Apply the small Crossh settings surface to Zed's terminal settings.
    ///
    /// The renderer reads these globals exactly as Zed's TerminalElement does,
    /// so there is one font, line-height, and scrollback configuration path.
    pub fn apply_zed_settings(settings: &TerminalSettings, cx: &mut App) {
        let mut zed_settings = ZedTerminalSettings::get_global(cx).clone();
        zed_settings.font_size = Some(px(settings.font_size));
        zed_settings.max_scroll_history_lines = Some(settings.scrollback);
        ZedTerminalSettings::override_global(zed_settings, cx);
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
            false,
            settings,
            shell_environment,
            cx,
        )
    }

    /// Create a local or remote interactive shell through Zed's TerminalBuilder.
    ///
    /// Crossh deliberately does not add a second PTY/event loop here. Remote
    /// sessions are represented by an interactive ssh shell, which gives them
    /// the same terminal behavior as local sessions.
    pub fn from_zed_shell(
        working_directory: Option<PathBuf>,
        initial_cwd: Option<String>,
        shell: Shell,
        is_remote_terminal: bool,
        settings: TerminalSettings,
        cx: &mut App,
    ) -> Entity<Self> {
        Self::from_zed_shell_with_environment(
            working_directory,
            initial_cwd,
            shell,
            is_remote_terminal,
            settings,
            None,
            cx,
        )
    }

    fn from_zed_shell_with_environment(
        working_directory: Option<PathBuf>,
        initial_cwd: Option<String>,
        shell: Shell,
        is_remote_terminal: bool,
        settings: TerminalSettings,
        shell_environment: Option<LocalShellEnvironment>,
        cx: &mut App,
    ) -> Entity<Self> {
        let settings = settings.normalized();
        Self::apply_zed_settings(&settings, cx);
        let zed_settings = ZedTerminalSettings::get_global(cx).clone();

        let display_builder = zed_terminal::TerminalBuilder::new_display_only_with_bounds(
            zed_settings.cursor_shape,
            AlternateScroll::On,
            Some(settings.scrollback),
            0,
            cx.background_executor(),
            util::paths::PathStyle::local(),
            terminal_bounds_for_grid(
                INITIAL_COLUMNS,
                INITIAL_ROWS,
                px(DEFAULT_CELL_WIDTH),
                px(settings.font_size * 1.3),
            ),
        );

        let shell_env = shell_environment
            .as_ref()
            .map(|environment| environment.env().iter().cloned().collect())
            .unwrap_or_default();
        let builder = zed_terminal::TerminalBuilder::new(
            working_directory,
            None,
            shell,
            shell_env,
            zed_settings.cursor_shape,
            AlternateScroll::On,
            Some(settings.scrollback),
            Vec::new(),
            0,
            is_remote_terminal,
            0,
            None,
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
                state: ConnState::Connecting,
                cwd: initial_cwd,
                title: None,
                is_local: !is_remote_terminal,
                ime_marked_text: String::new(),
                context_menu: None,
                right_mouse_down: false,
                anchor_bounds: Rc::new(Cell::new(None)),
                show_timestamps: settings.show_timestamps,
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
                zed_terminal::Event::Bell => cx.emit(TerminalEvent::Notification),
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
                zed_terminal::Event::SelectionsChanged
                | zed_terminal::Event::NewNavigationTarget(_)
                | zed_terminal::Event::Open(_) => {}
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
                    ZedTerminalSettings::get_global(cx).option_as_meta,
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
        self.paste_clipboard(cx);
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
        if let Some(text) = clipboard.text() {
            self.zed_terminal
                .update(cx, |terminal, _| terminal.paste(&text));
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
        if self.context_menu.is_some() {
            if event.keystroke.key == "escape" {
                self.close_context_menu(cx);
            }
            cx.stop_propagation();
            return;
        }
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
        self.context_menu = None;
        self.right_mouse_down = false;
        self.zed_terminal.update(cx, |terminal, _| {
            terminal.focus_out();
            terminal.set_cursor_shape(CursorShape::Hollow);
        });
        window.invalidate_character_coordinates();
        cx.notify();
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

    pub(crate) fn apply_settings(&mut self, settings: TerminalSettings, cx: &mut Context<Self>) {
        self.show_timestamps = settings.show_timestamps;
        // The workspace updates Zed's global font/scrollback settings before
        // calling this method. Timestamp visibility is owned by this host.
        cx.notify();
    }

    pub(crate) fn begin_right_mouse_down(
        &mut self,
        position: gpui::Point<Pixels>,
        forward_to_terminal: bool,
        cx: &mut Context<Self>,
    ) {
        self.right_mouse_down = forward_to_terminal;
        if !forward_to_terminal {
            self.open_context_menu(position, cx);
        }
    }

    pub(crate) fn take_right_mouse_down(&mut self) -> bool {
        let forwarded = self.right_mouse_down;
        self.right_mouse_down = false;
        forwarded
    }

    pub(crate) fn open_context_menu(
        &mut self,
        position: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.state != ConnState::Connected {
            return;
        }

        let (has_selection, hovered_word) = {
            let terminal = self.zed_terminal.read(cx);
            let content = terminal.last_content();
            (
                content.selection.is_some(),
                content
                    .last_hovered_word
                    .as_ref()
                    .map(|word| word.word.clone()),
            )
        };
        let entries = menu_entries(true, has_selection, hovered_word);
        self.context_menu = Some(ContextMenuState { position, entries });
        cx.notify();
    }

    fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu.take().is_some() {
            cx.notify();
        }
    }

    fn dispatch_menu_action(
        &mut self,
        action: TerminalMenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            TerminalMenuAction::Copy => self.copy_selection(cx),
            TerminalMenuAction::Paste => self.paste_clipboard(cx),
            TerminalMenuAction::SelectAll => self.select_all_terminal(cx),
            TerminalMenuAction::OpenUrl(url) => cx.open_url(&url),
        }
        window.focus(&self.focus, cx);
        self.close_context_menu(cx);
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
            return if self.is_local {
                local_terminal_tab_title(title, self.cwd.as_deref())
            } else {
                truncate_path_title(title)
            };
        }

        if self.is_local {
            let title = local_terminal_title(
                self.cwd.as_deref(),
                None,
                Some(util::shell::get_system_shell().as_str()),
            );
            if title != "Terminal" {
                return title;
            }
        }

        if self.is_local {
            fallback.to_owned()
        } else {
            remote_terminal_title(None)
        }
    }

    pub fn is_local(&self) -> bool {
        self.is_local
    }

    pub(crate) fn handle_system_notification_response(
        &mut self,
        _response: &SystemNotificationResponse,
        _cx: &mut Context<Self>,
    ) -> Option<bool> {
        None
    }

    pub(crate) fn notify_language(&mut self, _cx: &mut Context<Self>) {}
}

pub(crate) struct TerminalWorkspacePane(pub(crate) Entity<TerminalView>);

pub(crate) fn workspace_pane(entity: Entity<TerminalView>) -> Box<dyn WorkspacePane> {
    Box::new(TerminalWorkspacePane(entity))
}

impl WorkspacePane for TerminalWorkspacePane {
    fn render(&self) -> AnyElement {
        self.0.clone().into_any_element()
    }

    fn title(&self, cx: &App) -> String {
        self.0.read(cx).tab_title("Terminal")
    }

    fn terminal_info(&self, _cx: &App) -> Option<TerminalPaneInfo> {
        Some(TerminalPaneInfo {
            low_latency_enabled: false,
            low_latency_available: false,
        })
    }

    fn terminal_entity_id(&self) -> Option<gpui::EntityId> {
        Some(self.0.entity_id())
    }

    fn cwd(&self, cx: &App) -> Option<String> {
        self.0.read(cx).cwd.clone()
    }

    fn is_command_running(&self, cx: &App) -> bool {
        self.0.read(cx).is_command_running(cx)
    }

    fn toggle_low_latency(&self, _cx: &mut App) {}

    fn run_command(&self, command: &str, cx: &mut App) {
        self.0
            .update(cx, |terminal, cx| terminal.run_command(command, cx));
    }

    fn handle_system_notification_response(
        &self,
        response: &SystemNotificationResponse,
        cx: &mut App,
    ) -> Option<bool> {
        self.0.update(cx, |terminal, cx| {
            terminal.handle_system_notification_response(response, cx)
        })
    }

    fn request_focus(&self, cx: &mut App) {
        self.0.update(cx, |terminal, _| terminal.request_focus());
    }

    fn request_close(&self, cx: &mut App) {
        self.0.update(cx, |terminal, cx| terminal.request_close(cx));
    }

    fn apply_terminal_settings(&self, settings: TerminalSettings, cx: &mut App) {
        self.0
            .update(cx, |terminal, cx| terminal.apply_settings(settings, cx));
    }

    fn notify_language(&self, cx: &mut App) {
        self.0
            .update(cx, |terminal, cx| terminal.notify_language(cx));
    }

    fn risk(&self, _cx: &App) -> PaneRisk {
        PaneRisk::default()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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

        let anchor_bounds = self.anchor_bounds.clone();
        let bounds_capture = anchor_bounds.clone();
        let outside_click_anchor = anchor_bounds.clone();
        let context_menu = self.context_menu.clone();
        let context_menu_weak = cx.entity().downgrade();
        let bounds_canvas = canvas(
            move |bounds, _window, _cx| {
                bounds_capture.set(Some(bounds));
                bounds
            },
            move |_bounds, _state, window, _cx| {
                let Some(menu) = context_menu.as_ref() else {
                    return;
                };
                let menu_position = clamp_menu_position(menu.position, window, &menu.entries);
                let menu_bounds = Bounds {
                    origin: menu_position,
                    size: size(
                        px(CONTEXT_MENU_WIDTH + 32.0),
                        px(estimate_menu_height(&menu.entries) + 32.0),
                    ),
                };
                let anchor = outside_click_anchor.clone();
                let weak = context_menu_weak.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, event_window, cx| {
                    if phase != gpui::DispatchPhase::Capture {
                        return;
                    }
                    let outside = anchor
                        .get()
                        .is_some_and(|bounds| !bounds.contains(&event.position));
                    if !outside || menu_bounds.contains(&event.position) {
                        return;
                    }
                    let closed = weak
                        .update(cx, |this, cx| {
                            if this.context_menu.take().is_some() {
                                cx.notify();
                                true
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false);
                    if closed {
                        cx.stop_propagation();
                        event_window.refresh();
                    }
                });
            },
        )
        .absolute()
        .left_0()
        .top_0()
        .size_full();

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
            .on_key_down(cx.listener(Self::key_down))
            .on_click({
                let focus = focus.clone();
                move |_event, window, cx| window.focus(&focus, cx)
            })
            .child(bounds_canvas)
            .child(
                div()
                    .id("terminal-view-container")
                    .size_full()
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

        if let Some(menu) = self.context_menu.clone() {
            let anchor = anchor_bounds
                .get()
                .map(|bounds| bounds.origin)
                .unwrap_or_else(|| point(px(0.), px(0.)));
            root = root.child(render_context_menu(
                &menu,
                anchor,
                window,
                cx,
                |this, action, window, cx| this.dispatch_menu_action(action, window, cx),
                |this, cx| this.close_context_menu(cx),
            ));
        }

        root.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

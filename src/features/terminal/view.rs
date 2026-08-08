//! Crossh's thin host for the Zed terminal-view foundation.
//!
//! The terminal emulator, PTY lifecycle, resize protocol, mouse handling,
//! selection, IME, scrolling, and painting are owned by Zed's terminal crate
//! plus the local terminal element fork. This module only supplies the host
//! entity, focus/event wiring, and the workspace pane boundary.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Entity, EventEmitter, FocusHandle,
    InteractiveElement, IntoElement, KeyDownEvent, Keystroke, ParentElement, Pixels, Render,
    SharedString, StatefulInteractiveElement, Styled, Subscription, SystemNotificationResponse,
    Task, Window, div, point, px, size,
};
use settings::Settings as _;
use task::Shell;
use terminal as zed_terminal;
use terminal::terminal_settings::{
    AlternateScroll, CursorShape, TerminalSettings as ZedTerminalSettings,
};
use theme::ActiveTheme;

use crate::features::terminal::settings::TerminalSettings;
use crate::features::workspace::pane::{PaneRisk, TerminalPaneInfo, WorkspacePane};
use crate::shared::i18n;
use crate::shared::terminal::{
    local_terminal_tab_title, local_terminal_title, remote_terminal_title, strip_shell_host_prefix,
    truncate_path_title,
};

use super::events::{ConnState, TerminalEvent};

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

impl EventEmitter<TerminalEvent> for TerminalView {}

pub struct TerminalView {
    /// The display-only entity is replaced by the PTY-backed entity once the
    /// Zed builder finishes. The element reads this field directly.
    pub(crate) zed_terminal: Entity<zed_terminal::Terminal>,
    pending_builder: Option<zed_terminal::TerminalBuilder>,
    builder_error: Option<String>,
    builder_task: Option<Task<()>>,
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
        Self::from_zed_shell(Some(cwd), None, Shell::System, false, settings, cx)
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

        let builder = zed_terminal::TerminalBuilder::new(
            working_directory,
            None,
            shell,
            HashMap::default(),
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
            let (title, cwd) = {
                let terminal = terminal.read(cx);
                (
                    terminal.title(false),
                    terminal
                        .working_directory()
                        .map(|path| path.to_string_lossy().into_owned()),
                )
            };
            if this.title.as_deref() != Some(title.as_str()) {
                this.title = Some(title);
                cx.emit(TerminalEvent::TitleChanged);
            }

            if let Some(cwd) = cwd
                && this.cwd.as_deref() != Some(cwd.as_str())
            {
                this.cwd = Some(cwd);
                cx.emit(TerminalEvent::CwdChanged);
            }

            match event {
                zed_terminal::Event::CloseTerminal => {
                    if this.state != ConnState::Closed {
                        this.state = ConnState::Closed;
                        cx.emit(TerminalEvent::Closed);
                    }
                }
                zed_terminal::Event::Bell => cx.emit(TerminalEvent::Notification),
                zed_terminal::Event::TitleChanged | zed_terminal::Event::BreadcrumbsChanged => {
                    cx.emit(TerminalEvent::TitleChanged);
                }
                zed_terminal::Event::BlinkChanged(blinking) => {
                    this.blinking_terminal_enabled = *blinking;
                    if !*blinking {
                        this.cursor_blink_on = true;
                    }
                }
                zed_terminal::Event::Wakeup => {}
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
        self.zed_terminal
            .update(cx, |terminal, _| terminal.copy(None));
        cx.notify();
    }

    fn paste(&mut self, _: &zed_terminal::Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(clipboard) = cx.read_from_clipboard() else {
            return;
        };
        if let Some(text) = clipboard.text() {
            self.zed_terminal
                .update(cx, |terminal, _| terminal.paste(&text));
        }
    }

    fn paste_text(&mut self, _: &zed_terminal::PasteText, _: &mut Window, cx: &mut Context<Self>) {
        let Some(clipboard) = cx.read_from_clipboard() else {
            return;
        };
        if let Some(text) = clipboard.text() {
            self.zed_terminal
                .update(cx, |terminal, _| terminal.paste(&text));
        }
    }

    fn select_all(&mut self, _: &zed_terminal::SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.zed_terminal
            .update(cx, |terminal, _| terminal.select_all());
        cx.notify();
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

    pub(crate) fn apply_settings(&mut self, _settings: TerminalSettings, cx: &mut Context<Self>) {
        // The workspace updates Zed's global terminal settings before calling
        // this method. TerminalElement reads that same global on its next
        // layout, so no second renderer-local settings state is needed.
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

        root.into_any_element()
    }
}

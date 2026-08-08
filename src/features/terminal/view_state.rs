//! Terminal entity lifecycle, session state, and event handling.

use settings::Settings as _;
use std::path::PathBuf;
use task::Shell;

use super::*;

struct TerminalBridge {
    input_tx: Sender<InputCmd>,
    event_rx: Receiver<SessionEvent>,
    cols: usize,
    rows: usize,
    initial_cwd: Option<String>,
    is_local: bool,
    settings: TerminalSettings,
}

impl super::TerminalView {
    /// Keep Crossh's product settings as the thin layer over Zed's terminal
    /// settings, so the core terminal and local renderer observe the same values.
    pub fn apply_zed_settings(settings: &TerminalSettings, cx: &mut App) {
        let mut zed_settings =
            zed_terminal::terminal_settings::TerminalSettings::get_global(cx).clone();
        zed_settings.font_size = Some(px(settings.font_size));
        zed_settings.max_scroll_history_lines = Some(settings.scrollback);
        zed_terminal::terminal_settings::TerminalSettings::override_global(zed_settings, cx);
    }

    /// Creates a local terminal using Zed's real PTY/event-loop implementation.
    pub fn from_local_zed(cwd: PathBuf, settings: TerminalSettings, cx: &mut App) -> Entity<Self> {
        Self::from_zed_shell(Some(cwd), None, Shell::System, false, settings, cx)
    }

    /// Creates a terminal whose process and PTY are owned by Zed's terminal
    /// infrastructure. Remote terminals use this with an interactive SSH
    /// command, so their rendering and input path is identical to local ones.
    ///
    /// The builder is asynchronous because PTY creation and shell setup run
    /// off the UI thread. The real terminal entity is attached lazily from
    /// `Render`, after the builder has finished creating the PTY.
    pub fn from_zed_shell(
        working_directory: Option<PathBuf>,
        initial_cwd: Option<String>,
        shell: Shell,
        is_remote_terminal: bool,
        settings: TerminalSettings,
        cx: &mut App,
    ) -> Entity<Self> {
        let (input_tx, input_rx) = async_channel::unbounded::<InputCmd>();
        let (_event_tx, event_rx) = async_channel::unbounded::<SessionEvent>();
        let entity = Self::from_bridge_with_cwd(
            TerminalBridge {
                input_tx,
                event_rx,
                cols: 100,
                rows: 30,
                initial_cwd,
                is_local: !is_remote_terminal,
                settings: settings.clone(),
            },
            cx,
        );

        let builder = zed_terminal::TerminalBuilder::new(
            working_directory,
            None,
            shell,
            HashMap::default(),
            zed_terminal::terminal_settings::CursorShape::Block,
            zed_terminal::terminal_settings::AlternateScroll::On,
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

        let relay_weak = entity.downgrade();
        let input_relay = cx.spawn(async move |cx| {
            while let Ok(command) = input_rx.recv().await {
                let applied = relay_weak.update(cx, |this, cx| {
                    match command {
                        InputCmd::Write(bytes) => {
                            if this.zed_terminal_ready {
                                this.zed_terminal
                                    .update(cx, |terminal, _| terminal.input(bytes));
                            } else {
                                this.pending_input.push_back(InputCmd::Write(bytes));
                            }
                        }
                        // `maybe_resize` applies the pixel-aware bounds directly
                        // to the Zed terminal. Keep this command for the legacy
                        // queue contract, but do not resize a second time here.
                        InputCmd::Resize { .. } => {}
                        InputCmd::Close => {
                            let should_emit = !matches!(this.state, ConnState::Closed);
                            this.state = ConnState::Closed;
                            this.command_running = false;
                            if should_emit {
                                cx.emit(TerminalEvent::Closed);
                            }
                        }
                    }
                    cx.notify();
                });
                if applied.is_err() {
                    break;
                }
            }
        });
        entity.update(cx, |this, _| this._input_relay = Some(input_relay));

        let weak = entity.downgrade();
        let load_task = cx.spawn(async move |cx| match builder.await {
            Ok(builder) => {
                let _ = weak.update(cx, |this, cx| {
                    this.pending_zed_builder = Some(builder);
                    cx.notify();
                });
            }
            Err(error) => {
                let message = error.to_string();
                let _ = weak.update(cx, |this, cx| {
                    this.zed_builder_error = Some(message.clone());
                    this.state = ConnState::Error(message);
                    cx.notify();
                });
            }
        });
        entity.update(cx, |this, _| this._zed_builder_task = Some(load_task));
        entity
    }

    fn from_bridge_with_cwd(bridge: TerminalBridge, cx: &mut App) -> Entity<Self> {
        let TerminalBridge {
            input_tx,
            event_rx,
            cols,
            rows,
            initial_cwd,
            is_local,
            settings,
        } = bridge;
        let settings = settings.normalized();
        let (window_size, protocol_responses) = terminal_queues_for_bridge(cols, rows);
        let font = super::zed_terminal_font(cx);

        let zed_builder = zed_terminal::TerminalBuilder::new_display_only_with_bounds(
            zed_terminal::terminal_settings::CursorShape::Block,
            zed_terminal::terminal_settings::AlternateScroll::On,
            Some(settings.scrollback),
            0,
            cx.background_executor(),
            util::paths::PathStyle::local(),
            terminal_bounds_for_grid(cols, rows, px(8.), px(settings.font_size * 1.3)),
        );

        let entity = cx.new(|cx| {
            let zed_terminal = cx.new(|terminal_cx| zed_builder.subscribe(terminal_cx));
            let terminal_content = zed_terminal.read(cx).last_content().clone();
            Self {
                zed_terminal,
                pending_zed_builder: None,
                zed_builder_error: None,
                _zed_builder_task: None,
                _input_relay: None,
                zed_terminal_ready: false,
                _zed_terminal_subscription: None,
                terminal_content,
                terminal_total_lines: rows,
                pending_terminal_output: Vec::new(),
                input_tx: input_tx.clone(),
                pending_input: VecDeque::new(),
                state: ConnState::Connecting,
                command_running: false,
                shell_activity_available: false,
                protocol_parser: TerminalProtocolParser::default(),
                cwd: initial_cwd,
                focus: cx.focus_handle(),
                cell_w: px(0.),
                line_h: px(settings.font_size * 1.3),
                cols,
                rows,
                content_origin: Point::new(px(0.), px(0.)),
                window_size,
                protocol_responses,
                is_local,
                font,
                font_size: settings.font_size,
                scrollback: settings.scrollback,
                show_timestamps: settings.show_timestamps,
                _drain: None,
                focused_once: false,
                _focus_in: None,
                _focus_out: None,
                sel_start: None,
                sel_end: None,
                selecting: false,
                ime_marked_text: String::new(),
                low_latency_shell_input: false,
                shell_input_buffer: ShellInputBuffer::default(),
                remote_mouse_button: None,
                scroll_acc: 0.,
                cursor_blink_on: true,
                urxvt_mouse: false,
                keyboard_protocol: KeyboardProtocolState::default(),
                focused: true,
                notifications_enabled: settings.notifications_enabled,
                progress: None,
                images: Vec::new(),
                kitty_image_data: HashMap::new(),
                kitty_image_numbers: HashMap::new(),
                next_kitty_image_id: 1,
                kitty_active_image_id: None,
                kitty_notifications: HashMap::new(),
                notification_states: HashMap::new(),
                notification_state_order: VecDeque::new(),
                kitty_notification_expiry: HashMap::new(),
                notification_serial: 0,
                core_selection_pending: false,
                clear_pending: false,
                _blink_task: None,
                detected_urls: Vec::new(),
                line_timestamps: TerminalTimestampState::default(),
                pending_timestamp: None,
                title: None,
                process_info: None,
                local_shell: is_local.then(default_local_shell_name),
                context_menu: None,
                anchor_bounds: Rc::new(StdCell::new(None)),
                last_progress: Instant::now(),
                events_processed: 0,
            }
        });

        // drain：在主线程上从 event_rx 取事件喂给 Term。
        let weak = entity.downgrade();
        let drain = cx.spawn(async move |cx| {
            while let Ok(first_event) = event_rx.recv().await {
                let mut events = Vec::with_capacity(OUTPUT_BATCH_LIMIT);
                events.push(first_event);
                while events.len() < OUTPUT_BATCH_LIMIT {
                    match event_rx.try_recv() {
                        Ok(event) => events.push(event),
                        Err(_) => break,
                    }
                }
                let applied = weak.update(cx, |this, cx| {
                    this.apply_session_event_batch(events, cx);
                    cx.notify();
                });
                if applied.is_err() {
                    break; // Entity 已销毁。
                }
            }
            log::info!("terminal: drain loop ended");
        });
        entity.update(cx, |this, _cx| this._drain = Some(drain));

        // 光标闪烁：每 530ms 切换一次状态。
        let weak2 = entity.downgrade();
        let blink: Task<()> = cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(530))
                    .await;
                if weak2
                    .update(cx, |this, cx| {
                        this.cursor_blink_on = !this.cursor_blink_on;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        entity.update(cx, |this, _cx| {
            this._blink_task = Some(blink);
        });

        entity
    }

    pub fn apply_settings(&mut self, settings: TerminalSettings, cx: &mut Context<Self>) {
        let settings = settings.normalized();
        let font_size = settings.font_size;
        if (self.font_size - font_size).abs() > f32::EPSILON {
            self.font_size = font_size;
            self.line_h = px(font_size * 1.3);
            self.cell_w = px(0.);
            if let Ok(mut size) = self.window_size.lock() {
                size.cell_height = self.line_h.as_f32().round().max(1.) as u16;
            }
        }

        if self.scrollback != settings.scrollback {
            self.scrollback = settings.scrollback;
            // Zed's display-only terminal fixes its scrollback capacity when
            // the emulator is built. The new value applies to newly opened
            // terminals; rebuilding here would discard the current screen.
        }

        self.show_timestamps = settings.show_timestamps;
        self.notifications_enabled = settings.notifications_enabled;
        cx.notify();
    }

    /// Ask the PTY/SSH channel to close cleanly before its entity is dropped.
    pub(crate) fn request_close(&mut self, _cx: &mut Context<Self>) {
        self.drain_protocol_responses();
        self.queue_input(InputCmd::Close);
        self.flush_pending_input();
    }

    /// 处理一批来自 drain 循环的 SessionEvent，并维护诊断心跳。
    pub(super) fn apply_session_event_batch(
        &mut self,
        events: Vec<SessionEvent>,
        cx: &mut Context<Self>,
    ) {
        let now = Instant::now();
        for event in events {
            self.apply_session_event(event, cx);
        }
        self.flush_pending_terminal_output(cx);
        self.drain_protocol_responses();
        self.flush_pending_input();
        self.last_progress = Instant::now();
        let elapsed = now.elapsed();
        if elapsed > Duration::from_millis(250) {
            log::warn!(
                "terminal: slow drain batch took {}ms ({} events)",
                elapsed.as_millis(),
                self.events_processed
            );
        }
    }

    pub(super) fn apply_session_event(&mut self, event: SessionEvent, cx: &mut Context<Self>) {
        self.events_processed += 1;
        match event {
            SessionEvent::Connected => {
                log::info!("terminal: connected");
                self.state = ConnState::Connected;
            }
            SessionEvent::Output(bytes) => {
                log::trace!("pty output ({}B): {}", bytes.len(), debug_bytes(&bytes));
                // alternate screen 是 Codex/vim/top 等 TUI 的绘制缓冲区；只保留
                // 普通 shell 网格的时间戳，避免全屏重绘产生大量错误行。
                let was_alt_screen =
                    alacritty_mode(&self.terminal_content).contains(TermMode::ALT_SCREEN);
                let timestamp = (!was_alt_screen).then(|| format_timestamp(Local::now()));
                self.zed_terminal.update(cx, |terminal, terminal_cx| {
                    terminal.write_output(&bytes, terminal_cx);
                });
                let protocol_events = self.protocol_parser.feed(&bytes);
                self.process_protocol_events(protocol_events, cx, 0);
                self.drain_protocol_responses();
                if let Some(timestamp) = timestamp {
                    self.pending_timestamp = Some(timestamp);
                }
            }
            SessionEvent::Cwd(cwd) => {
                if self.cwd.as_deref() != Some(cwd.as_str()) {
                    self.cwd = Some(cwd);
                    cx.emit(TerminalEvent::CwdChanged);
                }
            }
            SessionEvent::ProcessInfo(info) => {
                if self.process_info.as_ref() != Some(&info) {
                    self.process_info = Some(info);
                    cx.emit(TerminalEvent::TitleChanged);
                }
            }
            SessionEvent::Error(error) => {
                log::warn!("terminal: error {error}");
                self.state = ConnState::Error(error);
            }
            SessionEvent::Closed => {
                log::info!("terminal: closed");
                self.drain_protocol_responses();
                let was_connected = self.state == ConnState::Connected;
                self.state = ConnState::Closed;
                self.command_running = false;
                self.shell_input_buffer.clear();
                self.ime_marked_text.clear();
                self.reset_crossh_keyboard_state();
                if was_connected {
                    cx.emit(TerminalEvent::Closed);
                }
            }
        }
    }

    pub(super) fn process_protocol_events(
        &mut self,
        events: Vec<ProtocolEvent>,
        cx: &mut Context<Self>,
        passthrough_depth: usize,
    ) {
        for event in events {
            match event {
                ProtocolEvent::Title(title) => {
                    self.title = Some(title);
                    cx.emit(TerminalEvent::TitleChanged);
                }
                ProtocolEvent::Bell => {
                    self.notify_user(
                        self.title.clone().unwrap_or_default(),
                        "Terminal bell".to_string(),
                        false,
                        cx,
                    );
                }
                ProtocolEvent::ClipboardStore(text) => {
                    if osc52_text_within_limit(&text) {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                    } else {
                        log::warn!("ignoring oversized OSC 52 clipboard write");
                    }
                }
                ProtocolEvent::ClipboardQuery(selector) => {
                    if !osc52_load_allowed(self.is_local) {
                        log::debug!("ignoring OSC 52 clipboard read from remote terminal");
                        continue;
                    }
                    let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
                        continue;
                    };
                    if let Some(response) = format_osc52_response_for_selector(selector, &text) {
                        self.send_input(response);
                    } else {
                        log::warn!("ignoring oversized OSC 52 clipboard response");
                    }
                }
                ProtocolEvent::KeyboardModeSet { bits, behavior } => {
                    self.keyboard_protocol.kitty_set(bits, behavior)
                }
                ProtocolEvent::KeyboardModePush { bits } => {
                    self.keyboard_protocol.kitty_push(bits);
                }
                ProtocolEvent::KeyboardModePop(count) => {
                    self.keyboard_protocol.kitty_pop(count);
                }
                ProtocolEvent::KeyboardModeQuery => {
                    self.send_input(
                        format!("\x1b[?{}u", self.keyboard_protocol.kitty_flags()).into_bytes(),
                    );
                }
                ProtocolEvent::PrimaryDeviceAttributesQuery => {
                    self.send_input(b"\x1b[?6c".to_vec());
                }
                ProtocolEvent::SecondaryDeviceAttributesQuery => {
                    self.send_input(b"\x1b[>0;1;1c".to_vec());
                }
                ProtocolEvent::DeviceStatusQuery => {
                    self.send_input(b"\x1b[0n".to_vec());
                }
                ProtocolEvent::CursorPositionQuery => {
                    let row = self.terminal_content.cursor.point.line.max(0) as usize + 1;
                    let column = self.terminal_content.cursor.point.column + 1;
                    self.send_input(format_cursor_position_response(row, column, false));
                }
                ProtocolEvent::PrivateCursorPositionQuery => {
                    let row = self.terminal_content.cursor.point.line.max(0) as usize + 1;
                    let column = self.terminal_content.cursor.point.column + 1;
                    self.send_input(format_cursor_position_response(row, column, true));
                }
                ProtocolEvent::Cwd(cwd) => {
                    if self.cwd.as_deref() != Some(cwd.as_str()) {
                        self.cwd = Some(cwd);
                        cx.emit(TerminalEvent::CwdChanged);
                    }
                }
                ProtocolEvent::Command(command) => {
                    self.command_running = true;
                    cx.emit(TerminalEvent::CommandStarted {
                        command,
                        cwd: self.cwd.clone(),
                    });
                }
                ProtocolEvent::Shell(shell_event) => {
                    self.shell_activity_available = true;
                    match shell_event {
                        ShellEvent::PromptStart => {
                            self.command_running = false;
                            self.shell_input_buffer.clear();
                            self.ime_marked_text.clear();
                            cx.emit(TerminalEvent::PromptReached);
                        }
                        ShellEvent::PromptEnd => {}
                        ShellEvent::CommandStart => self.command_running = true,
                        ShellEvent::CommandFinished { .. } => self.command_running = false,
                    }
                }
                ProtocolEvent::Notification { title, body } => {
                    self.notify_user(title, body, false, cx);
                }
                ProtocolEvent::NotificationPart {
                    id,
                    title,
                    body,
                    complete,
                    occasion,
                    report_activation,
                    report_close,
                    focus_on_activation,
                    expiry_ms,
                    buttons,
                } => self.process_kitty_notification(
                    KittyNotificationUpdate {
                        id,
                        title,
                        body,
                        complete,
                        occasion,
                        report_activation,
                        report_close,
                        focus_on_activation,
                        expiry_ms,
                        buttons,
                    },
                    cx,
                ),
                ProtocolEvent::KittyNotificationQuery { id } => {
                    self.respond_kitty_notification_query(&id)
                }
                ProtocolEvent::KittyNotificationClose { id } => {
                    self.close_kitty_notification(&id, cx)
                }
                ProtocolEvent::KittyNotificationAliveQuery { id } => {
                    self.respond_kitty_notification_alive(&id)
                }
                ProtocolEvent::KittyNotificationAliveResponse { .. } => {}
                ProtocolEvent::Progress { state, progress } => {
                    self.progress = (state != 0).then_some(TerminalProgress { state, progress });
                }
                ProtocolEvent::Image(payload) => self.store_image(payload),
                ProtocolEvent::KittyGraphics(payload) => self.process_kitty_graphics(payload),
                ProtocolEvent::Sixel(data) => self.process_sixel(data),
                ProtocolEvent::Decrqss(query) => self.respond_decrqss(&query),
                ProtocolEvent::XtGetTcap(query) => self.respond_xtgettcap(&query),
                ProtocolEvent::UrxvtMouse(enabled) => self.urxvt_mouse = enabled,
                ProtocolEvent::ModifyOtherKeys(level) => {
                    self.keyboard_protocol.set_modify_other_keys(level)
                }
                ProtocolEvent::ModifyOtherKeysMask(mask) => {
                    self.keyboard_protocol.set_modify_other_keys_mask(mask)
                }
                ProtocolEvent::ModifyOtherKeysQuery => self.respond_modify_other_keys_query(),
                ProtocolEvent::WindowSizeQuery => self.respond_window_size_query(),
                ProtocolEvent::TextAreaSizeQuery => self.respond_text_area_size_query(),
                ProtocolEvent::CellSizeQuery => self.respond_cell_size_query(),
                ProtocolEvent::Passthrough(bytes) => {
                    if passthrough_depth >= 4 {
                        log::warn!("ignoring deeply nested terminal passthrough sequence");
                        continue;
                    }
                    let nested_events = self.protocol_parser.feed(&bytes);
                    self.process_protocol_events(nested_events, cx, passthrough_depth + 1);
                    self.zed_terminal.update(cx, |terminal, terminal_cx| {
                        terminal.write_output(&bytes, terminal_cx);
                    });
                }
                ProtocolEvent::Reset => {
                    self.images.clear();
                    self.kitty_image_data.clear();
                    self.kitty_image_numbers.clear();
                    self.kitty_active_image_id = None;
                    self.reset_crossh_keyboard_state();
                }
                ProtocolEvent::SoftReset => self.reset_crossh_keyboard_state(),
                ProtocolEvent::ClearImages => {
                    self.images.clear();
                    self.kitty_image_data.clear();
                    self.kitty_image_numbers.clear();
                    self.kitty_active_image_id = None;
                }
                ProtocolEvent::ScreenBufferSwitch(alternate) => {
                    self.images.clear();
                    self.kitty_image_data.clear();
                    self.kitty_image_numbers.clear();
                    self.kitty_active_image_id = None;
                    self.keyboard_protocol.switch_screen(alternate);
                }
            }
        }
    }

    fn reset_crossh_keyboard_state(&mut self) {
        self.keyboard_protocol.reset();
        self.urxvt_mouse = false;
        self.remote_mouse_button = None;
    }

    pub(super) fn process_kitty_notification(
        &mut self,
        update: KittyNotificationUpdate,
        cx: &mut Context<Self>,
    ) {
        let KittyNotificationUpdate {
            id,
            title,
            body,
            complete,
            occasion,
            report_activation,
            report_close,
            focus_on_activation,
            expiry_ms,
            buttons,
        } = update;
        if !self.kitty_notifications.contains_key(&id)
            && self.kitty_notifications.len() >= MAX_PENDING_KITTY_NOTIFICATIONS
            && let Some(oldest_id) = self.kitty_notifications.keys().next().cloned()
        {
            self.kitty_notifications.remove(&oldest_id);
        }
        let notification = self.kitty_notifications.entry(id.clone()).or_default();
        if let Some(title) = title {
            append_bounded_notification_text(&mut notification.title, &title);
        }
        if let Some(body) = body {
            append_bounded_notification_text(&mut notification.body, &body);
        }
        if let Some(occasion) = occasion {
            notification.occasion = occasion;
        }
        if let Some(report_activation) = report_activation {
            notification.report_activation = report_activation;
        }
        if let Some(report_close) = report_close {
            notification.report_close = report_close;
        }
        if let Some(focus_on_activation) = focus_on_activation {
            notification.focus_on_activation = focus_on_activation;
        }
        if let Some(expiry_ms) = expiry_ms {
            notification.expiry_ms = Some(expiry_ms);
        }
        if let Some(buttons) = buttons {
            notification.buttons.extend(buttons.into_iter().take(8));
            notification.buttons.truncate(8);
        }
        if !complete || (notification.title.is_empty() && notification.body.is_empty()) {
            return;
        }

        let notification = self
            .kitty_notifications
            .remove(&id)
            .expect("notification entry was just inserted");
        self.display_notification(
            DisplayNotification {
                kitty_id: (!id.is_empty())
                    .then(|| sanitize_kitty_notification_id(&id))
                    .flatten(),
                title: notification.title,
                body: notification.body,
                occasion: notification.occasion,
                report_activation: notification.report_activation,
                report_close: notification.report_close,
                focus_on_activation: notification.focus_on_activation,
                buttons: notification.buttons,
                expiry_ms: notification.expiry_ms,
            },
            cx,
        );
    }

    pub(super) fn notify_user(
        &mut self,
        title: String,
        body: String,
        notify_when_focused: bool,
        cx: &mut Context<Self>,
    ) {
        self.display_notification(
            DisplayNotification {
                kitty_id: None,
                title,
                body,
                occasion: if notify_when_focused {
                    NotificationOccasion::Always
                } else {
                    NotificationOccasion::Unfocused
                },
                report_activation: false,
                report_close: false,
                focus_on_activation: true,
                buttons: Vec::new(),
                expiry_ms: None,
            },
            cx,
        );
    }

    pub(super) fn display_notification(
        &mut self,
        notification: DisplayNotification,
        cx: &mut Context<Self>,
    ) {
        let DisplayNotification {
            kitty_id,
            title,
            body,
            occasion,
            report_activation,
            report_close,
            focus_on_activation,
            buttons,
            expiry_ms,
        } = notification;
        cx.emit(TerminalEvent::Notification);
        if !self.notifications_enabled
            || !self.should_show_notification(occasion)
            || (title.is_empty() && body.is_empty())
        {
            return;
        }
        let title = if title.is_empty() {
            self.title.as_deref().unwrap_or("Terminal")
        } else {
            title.as_str()
        };
        let (tag, notification_key) = if let Some(id) = kitty_id.as_deref() {
            (
                format!("crossh-terminal-{}-kitty-{id}", cx.entity_id()),
                id.to_string(),
            )
        } else {
            let serial = self.notification_serial;
            self.notification_serial = self.notification_serial.wrapping_add(1);
            (
                format!("crossh-terminal-{}-{serial}", cx.entity_id()),
                format!("system-{serial}"),
            )
        };
        let actions = buttons
            .iter()
            .enumerate()
            .map(|(index, label)| SystemNotificationAction {
                id: (index + 1).to_string().into(),
                label: label.clone().into(),
            })
            .collect();
        cx.show_system_notification(gpui::SystemNotification {
            tag: tag.clone().into(),
            title: title.into(),
            body: body.into(),
            actions,
        });

        self.insert_notification_state(
            notification_key.clone(),
            NotificationState {
                tag: tag.clone(),
                kitty_id: kitty_id.clone(),
                report_activation,
                report_close,
                focus_on_activation,
            },
        );

        let Some(id) = kitty_id else {
            return;
        };
        // A replacement resets the locally enforced expiry. An omitted `w`
        // therefore falls back to the platform policy for the new payload.
        self.kitty_notification_expiry.remove(&id);
        if let Some(expiry_ms) = expiry_ms.filter(|expiry| *expiry > 0) {
            let tag_for_task = tag.clone();
            let id_for_task = id.clone();
            let task = cx.spawn(async move |weak, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(expiry_ms as u64))
                    .await;
                let _ = weak.update(cx, |this, cx| {
                    let is_current = this
                        .notification_states
                        .get(&id_for_task)
                        .is_some_and(|state| state.tag == tag_for_task);
                    if !is_current {
                        return;
                    }
                    let report_close = this
                        .remove_notification_state(&id_for_task)
                        .is_some_and(|state| state.report_close);
                    this.kitty_notification_expiry.remove(&id_for_task);
                    cx.dismiss_system_notification(&tag_for_task);
                    if report_close {
                        this.send_kitty_notification_close(&id_for_task, false);
                    }
                });
            });
            self.kitty_notification_expiry.insert(id, task);
        }
    }

    pub(super) fn insert_notification_state(&mut self, key: String, state: NotificationState) {
        self.notification_states.insert(key.clone(), state);
        self.notification_state_order
            .retain(|existing| existing != &key);
        self.notification_state_order.push_back(key);
        while self.notification_state_order.len() > MAX_PENDING_KITTY_NOTIFICATIONS {
            let Some(oldest) = self.notification_state_order.pop_front() else {
                break;
            };
            self.notification_states.remove(&oldest);
        }
    }

    pub(super) fn remove_notification_state(&mut self, key: &str) -> Option<NotificationState> {
        let state = self.notification_states.remove(key);
        if state.is_some() {
            self.notification_state_order
                .retain(|existing| existing != key);
        }
        state
    }

    pub(super) fn should_show_notification(&self, occasion: NotificationOccasion) -> bool {
        match occasion {
            NotificationOccasion::Always => true,
            NotificationOccasion::Unfocused | NotificationOccasion::Invisible => !self.focused,
        }
    }

    pub(super) fn close_kitty_notification(&mut self, id: &str, cx: &mut Context<Self>) {
        self.kitty_notifications.remove(id);
        let Some(id) = sanitize_kitty_notification_id(id) else {
            return;
        };
        self.kitty_notification_expiry.remove(&id);
        let Some(key) = self.notification_states.iter().find_map(|(key, state)| {
            (state.kitty_id.as_deref() == Some(id.as_str())).then_some(key.clone())
        }) else {
            return;
        };
        let Some(state) = self.remove_notification_state(&key) else {
            return;
        };
        cx.dismiss_system_notification(&state.tag);
        if state.report_close {
            self.send_kitty_notification_close(&id, false);
        }
    }

    pub(super) fn respond_kitty_notification_query(&mut self, id: &str) {
        let id = sanitize_kitty_notification_id(id).unwrap_or_default();
        self.send_input(
            format!(
                "\x1b]99;i={id}:p=?;a=report,focus:o=always,unfocused:p=title,body,close,alive,buttons:w=1\x1b\\"
            )
            .into_bytes(),
        );
    }

    pub(super) fn respond_kitty_notification_alive(&mut self, id: &str) {
        let id = sanitize_kitty_notification_id(id).unwrap_or_default();
        let mut alive = self
            .notification_states
            .values()
            .filter_map(|state| state.kitty_id.clone())
            .collect::<Vec<_>>();
        alive.sort();
        self.send_input(format!("\x1b]99;i={id}:p=alive;{}\x1b\\", alive.join(",")).into_bytes());
    }

    pub(super) fn send_kitty_notification_close(&mut self, id: &str, untracked: bool) {
        let suffix = if untracked { "untracked" } else { "" };
        self.send_input(format!("\x1b]99;i={id}:p=close;{suffix}\x1b\\").into_bytes());
    }

    /// Handle an OS notification activation. The return value tells AppShell
    /// whether the originating terminal should become the active tab.
    pub(crate) fn handle_system_notification_response(
        &mut self,
        response: &SystemNotificationResponse,
        cx: &mut Context<Self>,
    ) -> Option<bool> {
        let (key, state) = notification_state_for_tag(&self.notification_states, &response.tag)?;
        self.remove_notification_state(&key);
        cx.dismiss_system_notification(&state.tag);
        if let Some(id) = state.kitty_id.as_deref() {
            self.kitty_notification_expiry.remove(id);
            if state.report_activation {
                let action = response
                    .action_id
                    .as_ref()
                    .filter(|action| action.chars().all(|character| character.is_ascii_digit()))
                    .map(|action| action.as_ref())
                    .unwrap_or("");
                self.send_input(format!("\x1b]99;i={id};{action}\x1b\\").into_bytes());
            }
            if state.report_close {
                self.send_kitty_notification_close(id, false);
            }
        }
        Some(state.focus_on_activation)
    }

    pub(super) fn store_image(&mut self, payload: ImagePayload) {
        let width = image_dimension_cells(payload.width);
        let height = image_dimension_cells(payload.height);
        let do_not_move_cursor = payload.do_not_move_cursor;
        if self.store_image_with_id(payload, None, None, KittyPlacement::default(), false) {
            self.advance_image_cursor(width, height, do_not_move_cursor);
        }
    }

    pub(super) fn store_image_with_id(
        &mut self,
        payload: ImagePayload,
        kitty_id: Option<u32>,
        placement_id: Option<u32>,
        placement: KittyPlacement,
        virtual_placement: bool,
    ) -> bool {
        let Some(format) = terminal_image_format(&payload.data) else {
            log::debug!("ignoring terminal image with an unsupported format");
            return false;
        };
        if !terminal_image_within_limits(&payload.data, format) {
            log::debug!("ignoring terminal image outside renderer limits");
            return false;
        }
        let origin_line = self.terminal_total_lines as i64
            - self.terminal_content.terminal_bounds.num_lines() as i64
            + self.terminal_content.cursor.point.line as i64;
        let image = TerminalImage {
            image: Arc::new(gpui::Image::from_bytes(format, payload.data)),
            kitty_id,
            placement_id,
            origin_line,
            origin_col: self.terminal_content.cursor.point.column,
            width: payload.width,
            height: payload.height,
            preserve_aspect_ratio: payload.preserve_aspect_ratio,
            offset_x: placement.offset_x,
            offset_y: placement.offset_y,
            z_index: if virtual_placement {
                0
            } else {
                placement.z_index
            },
            virtual_placement,
            relative_image_id: placement.relative_image_id,
            relative_placement_id: placement.relative_placement_id,
            relative_offset_x: placement.relative_offset_x,
            relative_offset_y: placement.relative_offset_y,
        };
        if virtual_placement {
            if let Some(kitty_id) = kitty_id {
                self.images
                    .retain(|image| !(image.virtual_placement && image.kitty_id == Some(kitty_id)));
            }
        } else if let (Some(kitty_id), Some(placement_id)) = (kitty_id, placement_id) {
            self.images.retain(|image| {
                !(image.kitty_id == Some(kitty_id) && image.placement_id == Some(placement_id))
            });
        }
        if self.images.len() >= MAX_TERMINAL_IMAGES {
            self.images.remove(0);
        }
        self.images.push(image);
        true
    }

    pub(super) fn advance_image_cursor(
        &mut self,
        width_cells: Option<usize>,
        height_cells: Option<usize>,
        do_not_move_cursor: bool,
    ) {
        if do_not_move_cursor {
            return;
        }
        let mut sequence = Vec::new();
        if let Some(height) = height_cells.filter(|height| *height > 0) {
            sequence.extend_from_slice(format!("\x1b[{height}B").as_bytes());
        }
        if let Some(width) = width_cells.filter(|width| *width > 0) {
            sequence.extend_from_slice(format!("\x1b[{width}C").as_bytes());
        }
        if !sequence.is_empty() {
            self.pending_terminal_output.extend_from_slice(&sequence);
        }
    }

    fn flush_pending_terminal_output(&mut self, cx: &mut Context<Self>) {
        if self.pending_terminal_output.is_empty() {
            return;
        }
        let output = std::mem::take(&mut self.pending_terminal_output);
        self.zed_terminal.update(cx, |terminal, terminal_cx| {
            terminal.write_output(&output, terminal_cx);
        });
    }

    pub(super) fn process_sixel(&mut self, data: Vec<u8>) {
        let Ok(image) = icy_sixel::SixelImage::decode(&data) else {
            log::debug!("ignoring invalid Sixel image");
            return;
        };
        let (width, height) = image.corrected_dimensions();
        let Some(encoded) = encode_rgba_png(&image.pixels, image.width, image.height) else {
            log::debug!("ignoring oversized Sixel image");
            return;
        };
        let (width, height) = if image.aspect_ratio.is_square() {
            (None, None)
        } else {
            (
                Some(ImageDimension::Pixels(width)),
                Some(ImageDimension::Pixels(height)),
            )
        };
        self.store_image(ImagePayload {
            data: encoded,
            width,
            height,
            preserve_aspect_ratio: false,
            do_not_move_cursor: false,
        });
    }

    pub(super) fn allocate_kitty_image_id(&mut self) -> u32 {
        for _ in 0..u32::MAX {
            let id = self.next_kitty_image_id.max(1);
            self.next_kitty_image_id = id.wrapping_add(1).max(1);
            let used = self.kitty_image_data.contains_key(&id)
                || self.images.iter().any(|image| image.kitty_id == Some(id));
            if !used {
                return id;
            }
        }
        0
    }

    pub(super) fn process_kitty_graphics(&mut self, payload: KittyGraphicsPayload) {
        let requested_action = kitty_parameter(&payload.control, "a");
        let action = requested_action.unwrap_or("t");
        let requested_image_id = kitty_parameter(&payload.control, "i")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or_default();
        let requested_image_number =
            kitty_parameter(&payload.control, "I").and_then(|value| value.parse::<u32>().ok());
        let more_chunks = kitty_parameter(&payload.control, "m") == Some("1");
        let virtual_requested = kitty_parameter(&payload.control, "U") == Some("1");
        let relative_image_id = kitty_parameter(&payload.control, "P")
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|value| *value > 0);
        let relative_placement_id = kitty_parameter(&payload.control, "Q")
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|value| *value > 0);
        let relative_offset_x = kitty_parameter(&payload.control, "H")
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or_default();
        let relative_offset_y = kitty_parameter(&payload.control, "V")
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or_default();

        if requested_image_id != 0 && requested_image_number.is_some() {
            self.respond_kitty_graphics(&payload.control, "EINVAL");
            return;
        }
        if virtual_requested && !matches!(action, "p" | "T") {
            self.respond_kitty_graphics(&payload.control, "EINVAL");
            return;
        }
        if relative_image_id.is_some() && (virtual_requested || !matches!(action, "p"))
            || relative_image_id.is_none() && relative_placement_id.is_some()
        {
            self.respond_kitty_graphics(&payload.control, "ENOTSUP");
            return;
        }
        if let Some(medium) = kitty_parameter(&payload.control, "t")
            && medium != "d"
        {
            // File and shared-memory transmission are deliberately rejected:
            // the path/name arrives from the remote PTY and must not become an
            // arbitrary local filesystem read primitive.
            self.respond_kitty_graphics(&payload.control, "ENOTSUP");
            return;
        }
        if matches!(action, "a" | "c" | "f") {
            self.respond_kitty_graphics(&payload.control, "ENOTSUP");
            return;
        }
        if !matches!(action, "d" | "p" | "q" | "t" | "T") {
            self.respond_kitty_graphics(&payload.control, "EINVAL");
            return;
        }

        let mut image_id = requested_image_id;
        if requested_action.is_none() && image_id == 0 && requested_image_number.is_none() {
            image_id = self.kitty_active_image_id.unwrap_or_default();
        }
        if let Some(image_number) = requested_image_number {
            if matches!(action, "t" | "T") {
                image_id = self.allocate_kitty_image_id();
                if image_id == 0 {
                    self.respond_kitty_graphics(&payload.control, "ENOSPC");
                    return;
                }
                self.kitty_image_numbers.insert(image_number, image_id);
            } else {
                image_id = self
                    .kitty_image_numbers
                    .get(&image_number)
                    .copied()
                    .unwrap_or_default();
                if image_id == 0 {
                    self.respond_kitty_graphics(&payload.control, "ENOENT");
                    return;
                }
            }
        }

        if let Some(parent_id) = relative_image_id {
            if image_id == 0 || image_id == parent_id {
                self.respond_kitty_graphics(&payload.control, "EINVAL");
                return;
            }
            let parent_exists = self.images.iter().any(|image| {
                image.kitty_id == Some(parent_id)
                    && relative_placement_id
                        .is_none_or(|placement| image.placement_id == Some(placement))
            });
            if !parent_exists {
                self.respond_kitty_graphics(&payload.control, "ENOPARENT");
                return;
            }
            let mut current_id = parent_id;
            let mut current_placement_id = relative_placement_id;
            for depth in 0..=8 {
                let Some(parent) = self.images.iter().find(|image| {
                    image.kitty_id == Some(current_id)
                        && current_placement_id
                            .is_none_or(|placement| image.placement_id == Some(placement))
                }) else {
                    break;
                };
                let Some(next_id) = parent.relative_image_id else {
                    break;
                };
                if next_id == image_id {
                    self.respond_kitty_graphics(&payload.control, "ECYCLE");
                    return;
                }
                if depth == 8 {
                    self.respond_kitty_graphics(&payload.control, "ETOODEEP");
                    return;
                }
                current_id = next_id;
                current_placement_id = parent.relative_placement_id;
            }
        }

        // Every explicit transmit action starts a new image. Continuation
        // chunks omit `a` and therefore retain the partial payload.
        if matches!(requested_action, Some("t" | "T" | "q")) {
            self.kitty_image_data.remove(&image_id);
            self.kitty_active_image_id = Some(image_id);
            if action == "T" {
                self.images.retain(|image| image.kitty_id != Some(image_id));
            }
            if let Some(image_number) = requested_image_number
                && matches!(action, "t" | "T")
            {
                self.kitty_image_data
                    .entry(image_id)
                    .or_default()
                    .image_number = Some(image_number);
            }
        }

        if action == "d" {
            let active_image_id = self.kitty_active_image_id.take();
            if let Some(active_image_id) = active_image_id {
                self.kitty_image_data.remove(&active_image_id);
            }
            let placement_id =
                kitty_parameter(&payload.control, "p").and_then(|value| value.parse::<u32>().ok());
            let deletion = kitty_parameter(&payload.control, "d").unwrap_or("a");
            let history_size = self
                .terminal_total_lines
                .saturating_sub(self.terminal_content.terminal_bounds.num_lines())
                as i64;
            let cursor_line = history_size + self.terminal_content.cursor.point.line as i64;
            let cursor_column = self.terminal_content.cursor.point.column;
            let contains_cell = |image: &TerminalImage, line: i64, column: usize| {
                let width = image_dimension_cells(image.width).unwrap_or(1);
                let height = image_dimension_cells(image.height).unwrap_or(1);
                line >= image.origin_line
                    && line < image.origin_line.saturating_add(height as i64)
                    && column >= image.origin_col
                    && column < image.origin_col.saturating_add(width)
            };
            let delete_image = |image: &TerminalImage, image_id: u32, placement_id: Option<u32>| {
                image.kitty_id == Some(image_id)
                    && placement_id.is_none_or(|placement| image.placement_id == Some(placement))
            };
            let mut status = "OK";
            match deletion {
                // Visible deletion never touches virtual prototypes; their
                // real placements are represented by ordinary grid cells.
                "a" | "A" => self
                    .images
                    .retain(|image| image.virtual_placement || image.kitty_id.is_none()),
                "i" | "I" | "n" | "N" => {
                    if image_id == 0 {
                        status = "ENOENT";
                    } else {
                        self.images
                            .retain(|image| !delete_image(image, image_id, placement_id));
                        if matches!(deletion, "I" | "N") {
                            self.kitty_image_data.remove(&image_id);
                            self.kitty_image_numbers
                                .retain(|_, mapped_id| *mapped_id != image_id);
                        }
                    }
                }
                "c" | "C" => self.images.retain(|image| {
                    image.virtual_placement || !contains_cell(image, cursor_line, cursor_column)
                }),
                "p" | "P" | "q" | "Q" => {
                    let x = kitty_parameter(&payload.control, "x")
                        .and_then(|value| value.parse::<usize>().ok());
                    let y = kitty_parameter(&payload.control, "y")
                        .and_then(|value| value.parse::<usize>().ok());
                    let target = x.zip(y).and_then(|(x, y)| {
                        (x > 0 && y > 0).then_some((history_size + y as i64 - 1, x - 1))
                    });
                    let z = kitty_parameter(&payload.control, "z")
                        .and_then(|value| value.parse::<i32>().ok());
                    if target.is_none() || (matches!(deletion, "q" | "Q") && z.is_none()) {
                        status = "EINVAL";
                    } else if let Some((line, column)) = target {
                        self.images.retain(|image| {
                            image.virtual_placement
                                || (matches!(deletion, "q" | "Q")
                                    && image.z_index != z.unwrap_or_default())
                                || !contains_cell(image, line, column)
                        });
                    }
                }
                "r" | "R" => {
                    let lower = kitty_parameter(&payload.control, "x")
                        .and_then(|value| value.parse::<u32>().ok());
                    let upper = kitty_parameter(&payload.control, "y")
                        .and_then(|value| value.parse::<u32>().ok());
                    let Some((lower, upper)) = lower.zip(upper) else {
                        status = "EINVAL";
                        self.respond_kitty_graphics_for_image(
                            &payload.control,
                            image_id,
                            requested_image_number,
                            status,
                        );
                        return;
                    };
                    self.images.retain(|image| {
                        !image.kitty_id.is_some_and(|id| id >= lower && id <= upper)
                    });
                    if deletion == "R" {
                        self.kitty_image_data
                            .retain(|id, _| !(*id >= lower && *id <= upper));
                        self.kitty_image_numbers
                            .retain(|_, id| !(*id >= lower && *id <= upper));
                    }
                }
                "f" | "F" => status = "ENOTSUP",
                "x" | "X" => {
                    let Some(column) = kitty_parameter(&payload.control, "x")
                        .and_then(|value| value.parse::<usize>().ok())
                        .filter(|column| *column > 0)
                    else {
                        status = "EINVAL";
                        self.respond_kitty_graphics_for_image(
                            &payload.control,
                            image_id,
                            requested_image_number,
                            status,
                        );
                        return;
                    };
                    let column = column - 1;
                    self.images.retain(|image| {
                        image.virtual_placement
                            || column < image.origin_col
                            || column
                                >= image
                                    .origin_col
                                    .saturating_add(image_dimension_cells(image.width).unwrap_or(1))
                    });
                }
                "y" | "Y" => {
                    let Some(row) = kitty_parameter(&payload.control, "y")
                        .and_then(|value| value.parse::<usize>().ok())
                        .filter(|row| *row > 0)
                    else {
                        status = "EINVAL";
                        self.respond_kitty_graphics_for_image(
                            &payload.control,
                            image_id,
                            requested_image_number,
                            status,
                        );
                        return;
                    };
                    let line = history_size + row as i64 - 1;
                    self.images.retain(|image| {
                        image.virtual_placement
                            || line < image.origin_line
                            || line
                                >= image.origin_line.saturating_add(
                                    image_dimension_cells(image.height).unwrap_or(1) as i64,
                                )
                    });
                }
                "z" | "Z" => {
                    let Some(z_index) = kitty_parameter(&payload.control, "z")
                        .and_then(|value| value.parse::<i32>().ok())
                    else {
                        status = "EINVAL";
                        self.respond_kitty_graphics_for_image(
                            &payload.control,
                            image_id,
                            requested_image_number,
                            status,
                        );
                        return;
                    };
                    self.images
                        .retain(|image| image.virtual_placement || image.z_index != z_index);
                    if deletion == "Z" {
                        self.kitty_image_data
                            .retain(|_, data| data.z_index != z_index);
                    }
                }
                _ => status = "EINVAL",
            }
            self.respond_kitty_graphics_for_image(
                &payload.control,
                image_id,
                requested_image_number,
                status,
            );
            return;
        }

        if !payload.data.is_empty() {
            let image_data = self.kitty_image_data.entry(image_id).or_default();
            if image_data.data.len().saturating_add(payload.data.len()) > MAX_KITTY_IMAGE_BYTES {
                self.kitty_image_data.remove(&image_id);
                log::warn!("ignoring oversized Kitty graphics image");
                return;
            }
            image_data.data.extend(payload.data);
        }
        if let Some(action @ ("t" | "T")) = requested_action {
            self.kitty_image_data.entry(image_id).or_default().action = Some(action.to_string());
        }
        if virtual_requested {
            self.kitty_image_data
                .entry(image_id)
                .or_default()
                .virtual_placement = true;
        }
        if kitty_parameter(&payload.control, "o") == Some("z") {
            self.kitty_image_data
                .entry(image_id)
                .or_default()
                .compressed = true;
        }
        if let Some(placement_id) = kitty_parameter(&payload.control, "p")
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|placement| *placement > 0)
        {
            self.kitty_image_data
                .entry(image_id)
                .or_default()
                .placement_id = Some(placement_id);
        }
        if kitty_parameter(&payload.control, "C") == Some("1") {
            self.kitty_image_data
                .entry(image_id)
                .or_default()
                .do_not_move_cursor = true;
        }
        if let Some(format) =
            kitty_parameter(&payload.control, "f").and_then(|value| value.parse::<u32>().ok())
        {
            self.kitty_image_data.entry(image_id).or_default().format = Some(format);
        }
        if let Some(width) = kitty_parameter(&payload.control, "s")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| (1..=MAX_IMAGE_DIMENSION).contains(value))
        {
            self.kitty_image_data.entry(image_id).or_default().width = Some(width);
        }
        if let Some(height) = kitty_parameter(&payload.control, "v")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| (1..=MAX_IMAGE_DIMENSION).contains(value))
        {
            self.kitty_image_data.entry(image_id).or_default().height = Some(height);
        }
        if let Some(columns) = kitty_parameter(&payload.control, "c")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| (1..=MAX_IMAGE_DIMENSION).contains(value))
        {
            self.kitty_image_data.entry(image_id).or_default().columns = Some(columns);
        }
        if let Some(rows) = kitty_parameter(&payload.control, "r")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| (1..=MAX_IMAGE_DIMENSION).contains(value))
        {
            self.kitty_image_data.entry(image_id).or_default().rows = Some(rows);
        }
        if let Some(source_x) =
            kitty_parameter(&payload.control, "x").and_then(|value| value.parse::<usize>().ok())
        {
            self.kitty_image_data.entry(image_id).or_default().source_x = Some(source_x);
        }
        if let Some(source_y) =
            kitty_parameter(&payload.control, "y").and_then(|value| value.parse::<usize>().ok())
        {
            self.kitty_image_data.entry(image_id).or_default().source_y = Some(source_y);
        }
        if let Some(source_width) = kitty_parameter(&payload.control, "w")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| (1..=MAX_IMAGE_DIMENSION).contains(value))
        {
            self.kitty_image_data
                .entry(image_id)
                .or_default()
                .source_width = Some(source_width);
        }
        if let Some(source_height) = kitty_parameter(&payload.control, "h")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| (1..=MAX_IMAGE_DIMENSION).contains(value))
        {
            self.kitty_image_data
                .entry(image_id)
                .or_default()
                .source_height = Some(source_height);
        }
        if let Some(offset_x) =
            kitty_parameter(&payload.control, "X").and_then(|value| value.parse::<usize>().ok())
        {
            self.kitty_image_data.entry(image_id).or_default().offset_x = offset_x;
        }
        if let Some(offset_y) =
            kitty_parameter(&payload.control, "Y").and_then(|value| value.parse::<usize>().ok())
        {
            self.kitty_image_data.entry(image_id).or_default().offset_y = offset_y;
        }
        if let Some(z_index) =
            kitty_parameter(&payload.control, "z").and_then(|value| value.parse::<i32>().ok())
        {
            self.kitty_image_data.entry(image_id).or_default().z_index = z_index;
        }
        if more_chunks || !matches!(action, "p" | "T" | "t" | "q") {
            return;
        }
        self.kitty_active_image_id = None;

        let Some(image_data) = self.kitty_image_data.get(&image_id) else {
            self.respond_kitty_graphics(&payload.control, "ENOENT");
            return;
        };
        let stored_data = image_data.data.clone();
        let stored_compressed = image_data.compressed;
        let stored_action = image_data.action.clone();
        let stored_format = image_data.format;
        let stored_width = image_data.width;
        let stored_height = image_data.height;
        let stored_columns = image_data.columns;
        let stored_rows = image_data.rows;
        let stored_placement_id = image_data.placement_id;
        let stored_do_not_move_cursor = image_data.do_not_move_cursor;
        let stored_source_x = image_data.source_x;
        let stored_source_y = image_data.source_y;
        let stored_source_width = image_data.source_width;
        let stored_source_height = image_data.source_height;
        let stored_offset_x = image_data.offset_x;
        let stored_offset_y = image_data.offset_y;
        let stored_z_index = image_data.z_index;
        let stored_image_number = image_data.image_number;
        let stored_virtual_placement = image_data.virtual_placement;
        let data = if stored_compressed {
            let Some(data) = kitty_zlib_decode(&stored_data) else {
                self.respond_kitty_graphics(&payload.control, "EINVAL");
                return;
            };
            data
        } else {
            stored_data
        };
        let action = kitty_image_action(&payload.control, stored_action.as_deref());
        let format = kitty_parameter(&payload.control, "f")
            .and_then(|value| value.parse::<u32>().ok())
            .or(stored_format)
            .unwrap_or(32);
        let pixel_width = kitty_parameter(&payload.control, "s")
            .and_then(|value| value.parse::<usize>().ok())
            .or(stored_width);
        let pixel_height = kitty_parameter(&payload.control, "v")
            .and_then(|value| value.parse::<usize>().ok())
            .or(stored_height);
        let encoded_data = match format {
            24 | 32 => {
                let (Some(width), Some(height)) = (pixel_width, pixel_height) else {
                    log::debug!("ignoring Kitty raw image without pixel dimensions");
                    self.respond_kitty_graphics(&payload.control, "EINVAL");
                    return;
                };
                let channels = (format / 8) as usize;
                let Some(encoded) = kitty_raw_to_png(&data, width, height, channels) else {
                    log::debug!("ignoring invalid Kitty raw image");
                    self.respond_kitty_graphics(&payload.control, "EINVAL");
                    return;
                };
                encoded
            }
            100 if terminal_image_format(&data) == Some(gpui::ImageFormat::Png) => data,
            _ => {
                log::debug!("ignoring unsupported Kitty graphics format {format}");
                self.respond_kitty_graphics(&payload.control, "EINVAL");
                return;
            }
        };
        let source_x = kitty_parameter(&payload.control, "x")
            .and_then(|value| value.parse::<usize>().ok())
            .or(stored_source_x);
        let source_y = kitty_parameter(&payload.control, "y")
            .and_then(|value| value.parse::<usize>().ok())
            .or(stored_source_y);
        let source_width = kitty_parameter(&payload.control, "w")
            .and_then(|value| value.parse::<usize>().ok())
            .or(stored_source_width);
        let source_height = kitty_parameter(&payload.control, "h")
            .and_then(|value| value.parse::<usize>().ok())
            .or(stored_source_height);
        let placement = KittyPlacement {
            source_x,
            source_y,
            source_width,
            source_height,
            offset_x: kitty_parameter(&payload.control, "X")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(stored_offset_x),
            offset_y: kitty_parameter(&payload.control, "Y")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(stored_offset_y),
            z_index: kitty_parameter(&payload.control, "z")
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(stored_z_index),
            relative_image_id,
            relative_placement_id,
            relative_offset_x,
            relative_offset_y,
        };
        let Some(encoded_data) = crop_kitty_image(&encoded_data, placement) else {
            self.respond_kitty_graphics(&payload.control, "EINVAL");
            return;
        };
        if action == "q" {
            self.kitty_image_data.remove(&image_id);
            self.respond_kitty_graphics_for_image(
                &payload.control,
                image_id,
                requested_image_number.or(stored_image_number),
                "OK",
            );
            return;
        }
        if action == "t" {
            self.respond_kitty_graphics_for_image(
                &payload.control,
                image_id,
                requested_image_number.or(stored_image_number),
                "OK",
            );
            return;
        }
        if action == "T" {
            self.images.retain(|image| image.kitty_id != Some(image_id));
        }
        let width = kitty_parameter(&payload.control, "c")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| (1..=MAX_IMAGE_DIMENSION).contains(value))
            .or(stored_columns)
            .map(ImageDimension::Cells);
        let height = kitty_parameter(&payload.control, "r")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| (1..=MAX_IMAGE_DIMENSION).contains(value))
            .or(stored_rows)
            .map(ImageDimension::Cells);
        let placement_id = kitty_parameter(&payload.control, "p")
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|placement| *placement > 0)
            .or(stored_placement_id);
        let virtual_placement = virtual_requested || stored_virtual_placement;
        if virtual_placement && (width.is_none() || height.is_none()) {
            self.respond_kitty_graphics(&payload.control, "EINVAL");
            return;
        }
        let do_not_move_cursor =
            kitty_parameter(&payload.control, "C") == Some("1") || stored_do_not_move_cursor;
        let stored = self.store_image_with_id(
            ImagePayload {
                data: encoded_data,
                width,
                height,
                preserve_aspect_ratio: true,
                do_not_move_cursor,
            },
            Some(image_id),
            placement_id,
            placement,
            virtual_placement,
        );
        if stored && !virtual_placement {
            self.advance_image_cursor(
                image_dimension_cells(width),
                image_dimension_cells(height),
                do_not_move_cursor || relative_image_id.is_some(),
            );
        }
        self.respond_kitty_graphics_for_image(
            &payload.control,
            image_id,
            requested_image_number.or(stored_image_number),
            "OK",
        );
    }

    pub(super) fn respond_kitty_graphics(&mut self, control: &str, message: &str) {
        let image_id = kitty_parameter(control, "i")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        let image_number =
            kitty_parameter(control, "I").and_then(|value| value.parse::<u32>().ok());
        self.respond_kitty_graphics_for_image(control, image_id, image_number, message);
    }

    pub(super) fn respond_kitty_graphics_for_image(
        &mut self,
        control: &str,
        image_id: u32,
        image_number: Option<u32>,
        message: &str,
    ) {
        let quiet = kitty_parameter(control, "q");
        if (message == "OK" && quiet == Some("1")) || (message != "OK" && quiet == Some("2")) {
            return;
        }
        let image_number = image_number
            .map(|number| format!(",I={number}"))
            .unwrap_or_default();
        let placement = kitty_parameter(control, "p")
            .and_then(|value| value.parse::<u32>().ok())
            .map(|placement| format!(",p={placement}"))
            .unwrap_or_default();
        self.send_input(
            format!("\x1b_Gi={image_id}{image_number}{placement};{message}\x1b\\").into_bytes(),
        );
    }

    pub(super) fn respond_decrqss(&mut self, query: &[u8]) {
        let value = match query {
            b"m" => Some("0m".to_string()),
            b" q" => {
                let style_id = match self.terminal_content.cursor.shape {
                    zed_terminal::CursorShape::Block => 1,
                    zed_terminal::CursorShape::Underline => 3,
                    zed_terminal::CursorShape::Bar => 5,
                    zed_terminal::CursorShape::HollowBlock | zed_terminal::CursorShape::Hidden => 2,
                };
                Some(format!("{style_id} q"))
            }
            b"r" => Some(format!(
                "1;{}r",
                self.terminal_content.terminal_bounds.num_lines()
            )),
            b"\"p" => Some("64;1\"p".to_string()),
            _ => None,
        };
        let response = if let Some(value) = value {
            format!("\x1bP1$r{value}\x1b\\")
        } else {
            format!("\x1bP0$r{}\x1b\\", String::from_utf8_lossy(query))
        };
        self.send_input(response.into_bytes());
    }

    pub(super) fn respond_xtgettcap(&mut self, query: &[u8]) {
        let Some(capability) = decode_hex_bytes(query) else {
            self.send_input(
                format!("\x1bP0+r{}\x1b\\", String::from_utf8_lossy(query)).into_bytes(),
            );
            return;
        };
        let value = match capability.as_slice() {
            b"TN" => Some(b"xterm-256color".as_slice()),
            b"Co" => Some(b"256".as_slice()),
            b"RGB" => Some(b"8".as_slice()),
            b"Tc" => Some(b"truecolor".as_slice()),
            _ => None,
        };
        let response = if let Some(value) = value {
            format!(
                "\x1bP1+r{}={}\x1b\\",
                String::from_utf8_lossy(query),
                encode_hex_bytes(value)
            )
        } else {
            format!("\x1bP0+r{}\x1b\\", String::from_utf8_lossy(query))
        };
        self.send_input(response.into_bytes());
    }

    pub(super) fn respond_modify_other_keys_query(&mut self) {
        self.send_input(format_modify_other_keys_response(
            self.keyboard_protocol.modify_other_keys(),
        ));
    }

    pub(super) fn respond_cell_size_query(&mut self) {
        let size = self
            .window_size
            .lock()
            .map(|size| *size)
            .unwrap_or(WindowSize {
                num_lines: 30,
                num_cols: 100,
                cell_width: 8,
                cell_height: (FONT_SIZE * 1.3) as u16,
            });
        // xterm/Kitty encode this report as CSI 6 ; height ; width t.
        self.send_input(format!("\x1b[6;{};{}t", size.cell_height, size.cell_width).into_bytes());
    }

    pub(super) fn respond_window_size_query(&mut self) {
        let size = self
            .window_size
            .lock()
            .map(|size| *size)
            .unwrap_or(WindowSize {
                num_lines: 30,
                num_cols: 100,
                cell_width: 8,
                cell_height: (FONT_SIZE * 1.3) as u16,
            });
        let width = u32::from(size.num_cols).saturating_mul(u32::from(size.cell_width));
        let height = u32::from(size.num_lines).saturating_mul(u32::from(size.cell_height));
        // xterm/Kitty encode this report as CSI 4 ; height ; width t.
        self.send_input(format!("\x1b[4;{height};{width}t").into_bytes());
    }

    pub(super) fn respond_text_area_size_query(&mut self) {
        let size = self
            .window_size
            .lock()
            .map(|size| *size)
            .unwrap_or(WindowSize {
                num_lines: 30,
                num_cols: 100,
                cell_width: 8,
                cell_height: (FONT_SIZE * 1.3) as u16,
            });
        // CSI 8 reports rows and columns of the text area. CSI 19 has the
        // same dimensions for Crossh because the terminal has no separate
        // physical screen larger than its window.
        self.send_input(format!("\x1b[8;{};{}t", size.num_lines, size.num_cols).into_bytes());
    }

    /// Move parser-generated protocol replies into the same ordered queue used
    /// for keyboard input. Locking is bounded to a short drain operation and
    /// never awaits the channel consumer.
    pub(super) fn drain_protocol_responses(&mut self) {
        let responses = take_protocol_responses(&self.protocol_responses);
        for response in responses {
            self.queue_input(InputCmd::Write(response));
        }
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Return the compact tab title while keeping the raw OSC title available
    /// through title() for callers that need the unmodified value.
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
                self.process_info.as_ref(),
                self.local_shell.as_deref(),
            );
            return if title != "Terminal" {
                title
            } else {
                fallback.to_owned()
            };
        }

        remote_terminal_title(None)
    }

    pub fn is_local(&self) -> bool {
        self.is_local
    }

    pub(crate) fn low_latency_shell_input_enabled(&self) -> bool {
        self.low_latency_shell_input
    }

    /// The menu stays available once the remote shell has advertised shell
    /// integration; the input buffer itself only becomes active at a prompt.
    pub(crate) fn low_latency_shell_input_available(&self) -> bool {
        !self.is_local && self.state == ConnState::Connected && self.shell_activity_available
    }

    pub(crate) fn toggle_low_latency_shell_input(&mut self) {
        if self.low_latency_shell_input {
            self.flush_shell_input_buffer();
            self.ime_marked_text.clear();
        }
        self.low_latency_shell_input = !self.low_latency_shell_input;
    }

    pub(crate) fn is_command_running(&self) -> bool {
        self.state == ConnState::Connected
            && (self.command_running
                || alacritty_mode(&self.terminal_content).contains(TermMode::ALT_SCREEN))
    }
}

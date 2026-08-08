//! Terminal input handling, selection, menus, and IME bridge.

use super::*;

impl super::TerminalView {
    pub(super) fn send_input(&mut self, bytes: Vec<u8>) {
        log::trace!("pty write: {}", debug_bytes(&bytes));
        self.queue_input(InputCmd::Write(bytes));
    }

    pub(super) fn shell_input_active(&self) -> bool {
        self.low_latency_shell_input
            && !self.is_local
            && self.state == ConnState::Connected
            && self.shell_activity_available
            && !self.command_running
            && !self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    pub(super) fn flush_shell_input_buffer(&mut self) {
        let text = self.shell_input_buffer.take();
        if !text.is_empty() {
            self.send_input(text.into_bytes());
        }
    }

    pub(super) fn submit_shell_input(&mut self, enter: Vec<u8>) {
        let text = self.shell_input_buffer.take();
        let mut bytes = text.into_bytes();
        bytes.extend(enter);
        self.ime_marked_text.clear();
        if !bytes.is_empty() {
            self.send_input(bytes);
        }
    }

    /// Send the pending local text together with a key that needs the remote
    /// shell's own line editor, then return to transparent terminal input.
    pub(super) fn bypass_shell_input(&mut self, bytes: Vec<u8>) {
        let text = self.shell_input_buffer.take();
        let mut combined = text.into_bytes();
        combined.extend(bytes);
        self.ime_marked_text.clear();
        self.low_latency_shell_input = false;
        if !combined.is_empty() {
            self.send_input(combined);
        }
    }

    pub(super) fn insert_shell_input_text(&mut self, text: &str) {
        if text.is_empty() || text.chars().any(char::is_control) {
            return;
        }
        self.ime_marked_text.clear();
        self.shell_input_buffer.insert(text);
    }

    pub(super) fn handle_shell_input_key(&mut self, ks: &gpui::Keystroke, event_type: u8) -> bool {
        if !self.shell_input_active() {
            return false;
        }

        let key = ks.key.as_str();
        let modifiers = &ks.modifiers;
        let text_modifiers = !modifiers.alt && !modifiers.control && !modifiers.platform;
        let editing_modifiers = text_modifiers && !modifiers.shift;

        if is_low_latency_shell_passthrough_key(ks)
            && let Some(bytes) = encode_keystroke_with_options(
                ks,
                *self.term.mode(),
                event_type,
                self.modify_other_keys,
            )
        {
            self.ime_marked_text.clear();
            self.send_input(bytes);
            return true;
        }

        if matches!(key, "enter" | "return")
            && editing_modifiers
            && let Some(bytes) = encode_keystroke_with_options(
                ks,
                *self.term.mode(),
                event_type,
                self.modify_other_keys,
            )
        {
            self.submit_shell_input(bytes);
            self.command_running = true;
            return true;
        }

        if editing_modifiers {
            match key {
                "back" | "backspace" => {
                    self.shell_input_buffer.backspace();
                    self.ime_marked_text.clear();
                    return true;
                }
                "delete" => {
                    self.shell_input_buffer.delete();
                    self.ime_marked_text.clear();
                    return true;
                }
                "left" => {
                    self.shell_input_buffer.move_left();
                    self.ime_marked_text.clear();
                    return true;
                }
                "right" => {
                    self.shell_input_buffer.move_right();
                    self.ime_marked_text.clear();
                    return true;
                }
                "home" => {
                    self.shell_input_buffer.move_home();
                    self.ime_marked_text.clear();
                    return true;
                }
                "end" => {
                    self.shell_input_buffer.move_end();
                    self.ime_marked_text.clear();
                    return true;
                }
                "space" => {
                    self.insert_shell_input_text(" ");
                    return true;
                }
                _ => {}
            }
        }

        if text_modifiers
            && let Some(text) = ks.key_char.as_deref()
            && !text.is_empty()
            && text.chars().all(|ch| !ch.is_control())
        {
            self.insert_shell_input_text(text);
            return true;
        }

        if let Some(bytes) =
            encode_keystroke_with_options(ks, *self.term.mode(), event_type, self.modify_other_keys)
        {
            self.bypass_shell_input(bytes);
            return true;
        }

        false
    }

    /// The standalone UI drives `vte::Processor` directly instead of using
    /// Alacritty's PTY event loop, so it must enforce the synchronized-update
    /// deadline itself. Otherwise an interrupted `?2026h` sequence can leave
    /// the grid buffered indefinitely.
    pub(super) fn finish_expired_sync_update(&mut self) {
        let expired = self
            .parser
            .sync_timeout()
            .sync_timeout()
            .is_some_and(|deadline| deadline <= Instant::now());
        if expired {
            self.parser.stop_sync(&mut self.term);
            self._sync_timeout_task = None;
            self.drain_protocol_responses();
            self.line_timestamps.sync_to_term(&self.term);
        }
    }

    pub(super) fn schedule_sync_timeout(&mut self, cx: &mut Context<Self>) {
        let Some(deadline) = self.parser.sync_timeout().sync_timeout() else {
            self._sync_timeout_task = None;
            return;
        };
        let delay = deadline.saturating_duration_since(Instant::now());
        let task = cx.spawn(async move |weak, cx| {
            cx.background_executor().timer(delay).await;
            let _ = weak.update(cx, |this, cx| {
                this.finish_expired_sync_update();
                cx.notify();
            });
        });
        self._sync_timeout_task = Some(task);
    }

    /// 非阻塞地把输入命令送入 SSH relay；暂时满载时保留顺序，避免丢键。
    pub(super) fn queue_input(&mut self, command: InputCmd) {
        queue_input_nonblocking(&self.input_tx, &mut self.pending_input, command);
    }

    pub(super) fn flush_pending_input(&mut self) {
        flush_pending_commands(&self.input_tx, &mut self.pending_input);
    }

    /// 请求在下次 render 时自动聚焦终端（用于打开/切回 tab）。
    pub fn request_focus(&mut self) {
        self.focused_once = false;
    }

    /// Execute a command in this terminal's current interactive shell.
    pub fn run_command(&mut self, command: &str) {
        let command = command.trim();
        if command.is_empty() {
            return;
        }
        self.shell_input_buffer.clear();
        self.ime_marked_text.clear();
        if self.shell_activity_available {
            self.command_running = true;
        }
        self.send_input(format!("{command}\r").into_bytes());
        self.request_focus();
    }

    /// 根据当前尺寸调整远端 PTY 窗口。
    pub(super) fn maybe_resize(&mut self, bounds: Size) {
        // cell_w 由 render 阶段测量；若尚未测量则跳过（不应发生）。
        if self.cell_w.as_f32() <= 0.0 {
            return;
        }
        let new_cols = ((bounds.w / self.cell_w.as_f32()).floor() as usize).max(1);
        let new_rows = ((bounds.h / self.line_h.as_f32()).floor() as usize).max(1);
        if new_cols != self.cols || new_rows != self.rows {
            log::debug!(
                "maybe_resize: PTY {}x{} -> {}x{} (bounds={}x{}, cell_w={})",
                self.cols,
                self.rows,
                new_cols,
                new_rows,
                bounds.w as u32,
                bounds.h as u32,
                self.cell_w.as_f32()
            );
            self.cols = new_cols;
            self.rows = new_rows;
            self.term.resize(TermSize {
                cols: new_cols,
                rows: new_rows,
            });
            if !self.term.mode().contains(TermMode::ALT_SCREEN) {
                self.line_timestamps.sync_to_term(&self.term);
            }
            if let Ok(mut size) = self.window_size.lock() {
                size.num_cols = new_cols as u16;
                size.num_lines = new_rows as u16;
                size.cell_width = self.cell_w.as_f32().round().max(1.) as u16;
                size.cell_height = self.line_h.as_f32().round().max(1.) as u16;
            }
            self.queue_input(InputCmd::Resize {
                cols: new_cols as u16,
                rows: new_rows as u16,
            });
        }
    }

    pub(super) fn handle_key_down(
        &mut self,
        ev: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &ev.keystroke;
        // 右键菜单打开时拦截所有按键（Escape 关闭菜单），避免键位漏到远端。
        if self.context_menu.is_some() {
            if ks.key == "escape" {
                self.close_context_menu(cx);
            }
            cx.stop_propagation();
            return;
        }
        // 这些组合由 AppShell 处理；不要在 macOS 的 Ctrl+Tab 或其它平台的
        // Ctrl+W/Ctrl+T 情况下误发给远端 shell。
        if is_shell_shortcut(ks) {
            return;
        }
        // Cmd+C / Cmd+V / Cmd+A
        if ks.modifiers.platform && !ks.modifiers.alt && !ks.modifiers.control {
            match ks.key.as_str() {
                "c" => {
                    self.copy_selection(cx);
                    cx.stop_propagation();
                    return;
                }
                "v" => {
                    self.paste_clipboard(cx);
                    cx.stop_propagation();
                    return;
                }
                "a" => {
                    self.select_all();
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                _ => {}
            }
        }
        let event_type = if ev.is_held { 2 } else { 1 };
        if self.handle_shell_input_key(ks, event_type) {
            cx.notify();
            // Keep the existing Escape propagation behavior so AppShell can
            // dismiss any surrounding menu after the remote byte is sent.
            if ks.key != "escape" {
                cx.stop_propagation();
            }
            return;
        }
        let mode = *self.term.mode();
        match encode_keystroke_with_options(ks, mode, event_type, self.modify_other_keys) {
            Some(bytes) => {
                if self.shell_activity_available && matches!(ks.key.as_str(), "enter" | "return") {
                    self.command_running = true;
                }
                self.send_input(bytes);
                // macOS 会在 key callback 未消费时继续把事件交给
                // NSTextInputContext；终端已将它编码写入 PTY，必须阻止第二次文本提交。
                // 例外：无修饰的 Escape 需要继续冒泡给 AppShell（关闭右键菜单），
                // 且它不产生文本，不会触发重复提交。
                if ks.key != "escape" {
                    cx.stop_propagation();
                }
            }
            None => log::debug!(
                "unhandled keystroke: key={} key_char={:?}",
                ks.key,
                ks.key_char
            ),
        }
    }

    pub(super) fn handle_key_up(
        &mut self,
        ev: &KeyUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &ev.keystroke;
        if is_shell_shortcut(ks)
            || (ks.modifiers.platform
                && !ks.modifiers.alt
                && !ks.modifiers.control
                && matches!(ks.key.as_str(), "c" | "v"))
        {
            return;
        }
        // Printable keys handled by the local shell editor must not produce
        // a second byte when a remote application has enabled key-release
        // reporting.
        if self.shell_input_active() && !is_low_latency_shell_passthrough_key(ks) {
            return;
        }
        let mode = *self.term.mode();
        if !mode.contains(TermMode::REPORT_EVENT_TYPES) {
            return;
        }
        // REPORT_EVENT_TYPES only adds release events to keys represented by
        // kitty CSI-u sequences. Legacy text bytes ("a", Backspace, Enter,
        // etc.) must not be resent on key-up or TUIs such as Codex see them twice.
        if let Some(bytes) = encode_kitty_keystroke(ks, mode, 3) {
            self.send_input(bytes);
            cx.stop_propagation();
        }
    }

    pub(super) fn send_focus_event(&mut self, focused: bool) {
        if self.state == ConnState::Connected && self.term.mode().contains(TermMode::FOCUS_IN_OUT) {
            self.send_input(if focused {
                b"\x1b[I".to_vec()
            } else {
                b"\x1b[O".to_vec()
            });
        }
    }

    pub(super) fn handle_mouse_down(
        &mut self,
        ev: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Menu clicks bubble through terminal-root. Keep them from starting a
        // new one-cell terminal selection before the menu action runs.
        if self.context_menu.is_some() {
            return;
        }
        if self.state != ConnState::Connected {
            return;
        }
        let Some((col, row)) = self.pos_to_grid(ev.position) else {
            return;
        };

        // Cmd+Click → 检查 URL 跳转
        if ev.modifiers.platform
            && ev.button == MouseButton::Left
            && let Some(url) = self.url_at(col, row)
        {
            log::info!("opening URL: {url}");
            cx.open_url(&url);
            return;
        }

        let mode = *self.term.mode();

        // 右键：鼠模式开启时转发给远端应用，否则打开本地上下文菜单。
        if ev.button == MouseButton::Right {
            if mode.intersects(TermMode::MOUSE_MODE) && !ev.modifiers.shift {
                if let Some(button) = mouse_button_code(ev.button)
                    && let Some(bytes) = encode_mouse_report(
                        button,
                        col,
                        row,
                        true,
                        &ev.modifiers,
                        mode,
                        self.urxvt_mouse,
                    )
                {
                    self.send_input(bytes);
                    self.remote_mouse_button = Some(button);
                }
            } else {
                let url = self.url_at(col, row);
                self.open_terminal_context_menu(ev.position, url, cx);
            }
            return;
        }

        // Shift 保留本地选择，即使远端应用开启了鼠标模式。
        if mode.intersects(TermMode::MOUSE_MODE) && !ev.modifiers.shift {
            if let Some(button) = mouse_button_code(ev.button)
                && let Some(bytes) = encode_mouse_report(
                    button,
                    col,
                    row,
                    true,
                    &ev.modifiers,
                    mode,
                    self.urxvt_mouse,
                )
            {
                self.send_input(bytes);
                self.remote_mouse_button = Some(button);
            }
            return;
        }

        if ev.button == MouseButton::Left {
            self.selecting = true;
            self.sel_start = Some((col, row));
            self.sel_end = Some((col, row));
            cx.notify();
        }
    }

    pub(super) fn handle_mouse_up(
        &mut self,
        ev: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.context_menu.is_some() {
            return;
        }
        if self.state != ConnState::Connected {
            return;
        }
        if self.selecting && ev.button == MouseButton::Left {
            self.selecting = false;
            // 单击只改变焦点，不应留下一个单字符的“选区阴影”；只有
            // 实际拖拽出范围时才保留选区，供 Cmd+C 复制。
            if self.sel_start == self.sel_end {
                self.sel_start = None;
                self.sel_end = None;
            }
            cx.notify();
            return;
        }
        let mode = *self.term.mode();
        if mode.intersects(TermMode::MOUSE_MODE)
            && let Some((col, row)) = self.pos_to_grid(ev.position)
            && let Some(button) = mouse_button_code(ev.button)
        {
            let tracked_release = self.remote_mouse_button == Some(button);
            if !ev.modifiers.shift || tracked_release {
                if let Some(bytes) = encode_mouse_report(
                    button,
                    col,
                    row,
                    false,
                    &ev.modifiers,
                    mode,
                    self.urxvt_mouse,
                ) {
                    self.send_input(bytes);
                }
                if tracked_release {
                    self.remote_mouse_button = None;
                }
            }
        }
    }

    pub(super) fn handle_mouse_move(
        &mut self,
        ev: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.context_menu.is_some() {
            return;
        }
        if self.state != ConnState::Connected {
            return;
        }
        let mode = *self.term.mode();
        if self.selecting && ev.pressed_button == Some(MouseButton::Left) {
            if let Some((col, row)) = self.pos_to_grid(ev.position)
                && self.sel_end != Some((col, row))
            {
                self.sel_end = Some((col, row));
                cx.notify();
            }
            return;
        }

        if mode.intersects(TermMode::MOUSE_MODE) && !ev.modifiers.shift {
            let reports_motion = mode.contains(TermMode::MOUSE_MOTION)
                || (mode.contains(TermMode::MOUSE_DRAG) && ev.pressed_button.is_some());
            if let Some((col, row)) = self.pos_to_grid(ev.position)
                && reports_motion
            {
                let button = ev
                    .pressed_button
                    .and_then(mouse_button_code)
                    .map(|button| 32 + button)
                    .unwrap_or(35);
                if let Some(bytes) = encode_mouse_report(
                    button,
                    col,
                    row,
                    true,
                    &ev.modifiers,
                    mode,
                    self.urxvt_mouse,
                ) {
                    self.send_input(bytes);
                }
            }
        }
    }

    pub(super) fn handle_scroll_wheel(
        &mut self,
        ev: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.context_menu.is_some() {
            return;
        }
        let mode = *self.term.mode();
        let Some(delta) = wheel_lines_for_phase(
            ev.touch_phase,
            ev.delta,
            &mut self.scroll_acc,
            self.line_h.as_f32(),
        ) else {
            return;
        };
        let steps = delta.unsigned_abs().clamp(1, 8) as usize;

        cx.stop_propagation();
        match wheel_route(mode, ev.modifiers.shift) {
            WheelRoute::MouseReport => {
                let dir = if delta > 0 { 64 } else { 65 };
                let point = self.pos_to_grid(ev.position).unwrap_or((
                    self.cols.saturating_sub(1) / 2,
                    self.rows.saturating_sub(1) / 2,
                ));
                for _ in 0..steps {
                    if let Some(bytes) = encode_mouse_report(
                        dir,
                        point.0,
                        point.1,
                        true,
                        &ev.modifiers,
                        mode,
                        self.urxvt_mouse,
                    ) {
                        self.send_input(bytes);
                    }
                }
            }
            WheelRoute::AlternateScroll => {
                let key = if delta > 0 { b'A' } else { b'B' };
                let sequence = alternate_scroll_sequence(mode, key);
                for _ in 0..steps {
                    self.send_input(sequence.to_vec());
                }
            }
            WheelRoute::LocalScrollback => {
                let n = steps as i32 * delta.signum();
                self.term.scroll_display(Scroll::Delta(n));
                cx.notify();
            }
        }
    }

    /// 将 GPUI 的窗口坐标转换为终端 canvas 内的 grid 坐标。
    pub(super) fn pos_to_grid(&self, pos: Point<Pixels>) -> Option<(usize, usize)> {
        if self.cell_w.as_f32() <= 0. || self.line_h.as_f32() <= 0. {
            return None;
        }
        let local = pos - self.content_origin;
        // gutter 属于 UI，不是终端的第 0 列；点击 gutter 时不应触发
        // 选择或远端鼠标协议。
        let x = local.x.as_f32();
        let y = local.y.as_f32();
        if x < 0. || y < 0. {
            return None;
        }
        let col = ((x / self.cell_w.as_f32()) as usize).min(self.cols.saturating_sub(1));
        let row = ((y / self.line_h.as_f32()) as usize).min(self.rows.saturating_sub(1));
        (self.cols > 0 && self.rows > 0).then_some((col, row))
    }

    /// Cmd+C：将选中的文本复制到剪贴板。
    pub(super) fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let Some((sx, sy)) = self.sel_start else {
            return;
        };
        let Some((ex, ey)) = self.sel_end else { return };
        if sx == ex && sy == ey {
            return;
        }

        let text = self.extract_selection_text(sx, sy, ex, ey);
        self.sel_start = None;
        self.sel_end = None;
        cx.notify();

        if text.is_empty() {
            return;
        }
        let item = gpui::ClipboardItem::new_string(text);
        cx.write_to_clipboard(item);
    }

    /// Cmd+V：从剪贴板读取文本并发送到 PTY。
    pub(super) fn paste_clipboard(&mut self, _cx: &mut Context<Self>) {
        if self.state != ConnState::Connected {
            return;
        }
        let item = _cx.read_from_clipboard();
        if let Some(text) = item.and_then(|it| {
            it.entries.into_iter().find_map(|e| {
                if let gpui::ClipboardEntry::String(s) = e {
                    Some(s.text)
                } else {
                    None
                }
            })
        }) {
            if self.shell_input_active() {
                if !text.chars().any(|ch| ch == '\n' || ch == '\r') {
                    self.insert_shell_input_text(&text);
                    return;
                }
                // Multi-line paste has shell-specific semantics. Let the
                // remote line editor handle it instead of guessing locally.
                self.flush_shell_input_buffer();
                self.ime_marked_text.clear();
                self.low_latency_shell_input = false;
            }
            let bytes = if self.term.mode().contains(TermMode::BRACKETED_PASTE) {
                let mut bytes = b"\x1b[200~".to_vec();
                bytes.extend_from_slice(text.as_bytes());
                bytes.extend_from_slice(b"\x1b[201~");
                bytes
            } else {
                text.into_bytes()
            };
            self.send_input(bytes);
        }
    }

    /// 全选当前视口（配合 Cmd+A / 右键菜单）。
    pub(super) fn select_all(&mut self) {
        if self.cols == 0 || self.rows == 0 {
            return;
        }
        self.sel_start = Some((0, 0));
        self.sel_end = Some((self.cols.saturating_sub(1), self.rows.saturating_sub(1)));
    }

    /// 右键打开上下文菜单；外部点击监听在 canvas 的 paint 阶段注册。
    pub(super) fn open_terminal_context_menu(
        &mut self,
        position: Point<Pixels>,
        url: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let has_selection = self.sel_start.zip(self.sel_end).is_some();
        let connected = self.state == ConnState::Connected;
        let mut entries = vec![
            MenuEntry::Item(MenuItem {
                id: "copy".into(),
                label: i18n::text("context_menu.copy"),
                shortcut_hint: Some("⌘C".into()),
                disabled: !has_selection,
                danger: false,
                action: TerminalMenuAction::Copy,
            }),
            MenuEntry::Item(MenuItem {
                id: "paste".into(),
                label: i18n::text("context_menu.paste"),
                shortcut_hint: Some("⌘V".into()),
                disabled: !connected,
                danger: false,
                action: TerminalMenuAction::Paste,
            }),
            MenuEntry::Item(MenuItem {
                id: "select-all".into(),
                label: i18n::text("context_menu.select_all"),
                shortcut_hint: Some("⌘A".into()),
                disabled: false,
                danger: false,
                action: TerminalMenuAction::SelectAll,
            }),
        ];
        if let Some(url) = url {
            entries.push(MenuEntry::Separator);
            entries.push(MenuEntry::Item(MenuItem {
                id: "open-url".into(),
                label: i18n::text("context_menu.open_link"),
                shortcut_hint: None,
                disabled: false,
                danger: false,
                action: TerminalMenuAction::OpenUrl(url),
            }));
        }
        self.context_menu = Some(ContextMenuState { position, entries });
        cx.notify();
    }

    pub(super) fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn dispatch_menu_action(
        &mut self,
        action: TerminalMenuAction,
        cx: &mut Context<Self>,
    ) {
        match action {
            TerminalMenuAction::Copy => self.copy_selection(cx),
            TerminalMenuAction::Paste => self.paste_clipboard(cx),
            TerminalMenuAction::SelectAll => self.select_all(),
            TerminalMenuAction::OpenUrl(url) => {
                log::info!("opening URL: {url}");
                cx.open_url(&url);
            }
        }
        self.close_context_menu(cx);
    }

    /// 检查 (col, row) 是否在某个检测到的 URL 上。
    pub(super) fn url_at(&self, col: usize, row: usize) -> Option<String> {
        for &(r, cs, ce, ref url) in &self.detected_urls {
            if r == row && col >= cs && col < ce {
                return Some(url.clone());
            }
        }
        None
    }

    /// 从 grid 中提取选择区域内的文本。
    pub(super) fn extract_selection_text(
        &self,
        sx: usize,
        sy: usize,
        ex: usize,
        ey: usize,
    ) -> String {
        let grid = self.term.grid();
        let display_offset = grid.display_offset();
        let top_line = -(display_offset as i32);
        let _rows = self.term.screen_lines();

        let (y0, y1) = if sy <= ey { (sy, ey) } else { (ey, sy) };
        let (x0, x1) = selection_column_bounds(sx, sy, ex, ey);

        let mut out = String::new();
        for vy in y0..=y1 {
            let line_idx = top_line + vy as i32;
            let line = &grid[Line(line_idx)];
            let start_col = if vy == y0 { x0 } else { 0 };
            let end_col = if vy == y1 {
                x1.min(self.cols.saturating_sub(1))
            } else {
                self.cols - 1
            };
            for c in start_col..=end_col {
                let ch = line[Column(c)].c;
                if ch != '\0' {
                    out.push(ch);
                }
            }
            if vy < y1 {
                out.push('\n');
            }
        }
        out
    }

    /// 当前终端光标在可视 viewport 中的位置。
    pub(super) fn ime_cursor_bounds(
        &self,
        _element_bounds: Bounds<Pixels>,
        window: &Window,
    ) -> Option<Bounds<Pixels>> {
        if self.cell_w.as_f32() <= 0.0 || self.line_h.as_f32() <= 0.0 {
            return None;
        }

        let grid = self.term.grid();
        let (col, row) = cursor_viewport_position(
            grid.cursor.point.line.0,
            grid.cursor.point.column.0,
            grid.display_offset(),
            self.term.screen_lines(),
            self.term.columns(),
        )?;

        let cursor_column = grid.cursor.point.column.0;
        let line = &grid[grid.cursor.point.line];
        let cursor_cell = &line[Column(cursor_column)];
        let (visual_col, width_cells) = if cursor_cell.flags.contains(CellFlags::WIDE_CHAR) {
            (col, 2)
        } else if cursor_cell
            .flags
            .intersects(CellFlags::WIDE_CHAR_SPACER | CellFlags::LEADING_WIDE_CHAR_SPACER)
            && cursor_column > 0
            && line[Column(cursor_column - 1)]
                .flags
                .contains(CellFlags::WIDE_CHAR)
        {
            (col.saturating_sub(1), 2)
        } else if cursor_cell
            .flags
            .intersects(CellFlags::WIDE_CHAR_SPACER | CellFlags::LEADING_WIDE_CHAR_SPACER)
            && cursor_column + 1 < self.term.columns()
            && line[Column(cursor_column + 1)]
                .flags
                .contains(CellFlags::WIDE_CHAR)
        {
            (col, 2)
        } else {
            (col, 1)
        };

        let mut origin_x = self.content_origin.x + px(visual_col as f32 * self.cell_w.as_f32());
        if self.shell_input_active() {
            let prefix = &self.shell_input_buffer.text()[..self.shell_input_buffer.cursor()];
            if !prefix.is_empty() {
                let text_run = TextRun {
                    len: prefix.len(),
                    font: self.font.clone(),
                    color: fg_of(&self.term),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let shaped = window.text_system().shape_line(
                    SharedString::from(prefix.to_owned()),
                    px(self.font_size),
                    &[text_run],
                    None,
                );
                origin_x += shaped.width();
            }
        }

        Some(Bounds {
            origin: Point::new(
                origin_x,
                self.content_origin.y + px(row as f32 * self.line_h.as_f32()),
            ),
            size: gpui::size(px(width_cells as f32 * self.cell_w.as_f32()), self.line_h),
        })
    }
}

pub(super) fn selection_column_bounds(
    sx: usize,
    sy: usize,
    ex: usize,
    ey: usize,
) -> (usize, usize) {
    if sy == ey {
        (sx.min(ex), sx.max(ex))
    } else if sy < ey {
        (sx, ex)
    } else {
        (ex, sx)
    }
}

/// Bridges the terminal's virtual IME buffer to GPUI without presenting the
/// terminal grid as an editable document.
///
/// `ElementInputHandler` derives `prefers_ime_for_printable_keys` from
/// `accepts_text_input`. Crossh needs committed text and IME composition to go
/// through macOS first; keys the input context does not consume are routed
/// back to the terminal by GPUI.
pub(super) struct TerminalInputHandler {
    pub(super) terminal: Entity<TerminalView>,
    pub(super) element_bounds: Bounds<Pixels>,
}

impl InputHandler for TerminalInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(&mut self, window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.terminal
            .update(cx, |view, view_cx| view.marked_text_range(window, view_cx))
    }

    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<String> {
        // A terminal is not a random-access text document. The marked text is
        // only a visual composition buffer and is never queried as document
        // text by the platform.
        None
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.terminal.update(cx, |view, view_cx| {
            view.replace_text_in_range(range, text, window, view_cx);
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.terminal.update(cx, |view, view_cx| {
            view.replace_and_mark_text_in_range(range, new_text, selected_range, window, view_cx);
        });
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut App) {
        self.terminal.update(cx, |view, view_cx| {
            view.unmark_text(window, view_cx);
        });
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.terminal.update(cx, |view, view_cx| {
            view.bounds_for_range(range, self.element_bounds, window, view_cx)
        })
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<usize> {
        self.terminal.update(cx, |view, view_cx| {
            view.character_index_for_point(point, window, view_cx)
        })
    }

    fn accepts_text_input(&mut self, window: &mut Window, cx: &mut App) -> bool {
        self.terminal
            .update(cx, |view, view_cx| view.accepts_text_input(window, view_cx))
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }

    fn prefers_ime_for_printable_keys(&mut self, _window: &mut Window, _cx: &mut App) -> bool {
        // Let macOS compose Chinese/Japanese/Korean and dictation text first.
        // If the input context does not consume a key, GPUI routes it back to
        // the terminal key handler through doCommandBySelector.
        true
    }
}

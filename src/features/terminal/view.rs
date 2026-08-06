//! 终端视图：gpui Entity，持有 alacritty_terminal::Term，自研 Canvas 渲染器。
//!
//! 数据流：
//!  - 远端 → russh 读循环 → SessionEvent::Output → 本 Entity 的 drain 任务
//!    → `parser.advance(&mut term, bytes)` → cx.notify() 触发重绘。
//!  - 键盘 → on_key_down → 编码为字节 → input_tx → russh 写循环。
//!
//! Term 只在 gpui 主线程被触碰（drain 与 paint 都在主线程）。

use std::cell::Cell as StdCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::{Cursor, Read};
use std::ops::Range;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::ClipboardType;
use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::{Config, Osc52, Term, TermMode};
use async_channel::{Receiver, Sender};
use chrono::Local;
use flate2::read::ZlibDecoder;
use gpui::{
    App, AppContext, Bounds, Context, Corners, Edges, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Font, FontWeight, Hsla, InputHandler, InteractiveElement, IntoElement,
    KeyDownEvent, KeyUpEvent, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, SharedString,
    StatefulInteractiveElement, StrikethroughStyle, Styled, Subscription, SystemNotificationAction,
    SystemNotificationResponse, Task, TextAlign, TextRun, TouchPhase, UTF16Selection,
    UnderlineStyle, Window, canvas, div, hsla, px, quad,
};
use vte::ansi::{Color, CursorShape, NamedColor, Processor, Rgb};

use crate::shared::i18n::{self, AppSettings};
use crate::shared::terminal::{
    ImageDimension, ImagePayload, InputCmd, KittyGraphicsPayload, NotificationOccasion,
    ProtocolEvent, SessionEvent, ShellEvent, TerminalProcessInfo, TerminalProtocolParser,
    local_terminal_tab_title, local_terminal_title, remote_terminal_title, strip_shell_host_prefix,
    truncate_path_title,
};
use crate::shared::ui::context_menu::{
    CONTEXT_MENU_WIDTH, ContextMenuState, MenuEntry, MenuItem, TerminalMenuAction,
    clamp_menu_position, estimate_menu_height, render_context_menu,
};
use crate::shared::ui::theme;

use super::events::{ConnState, TerminalEvent};
use super::input::{flush_pending_commands, queue_input_nonblocking};

/// 终端字体大小（像素）。
const FONT_SIZE: f32 = 14.0;
/// 终端根 div 的内边距（渲染时 `px_3` / `py_2`，用于把窗口坐标换算成根内坐标）。
const TERMINAL_PADDING_X: f32 = 12.0;
const TERMINAL_PADDING_Y: f32 = 8.0;
/// 滚动历史行数上限（控内存）。
#[cfg(test)]
const SCROLLBACK: usize = 1000;
/// 一次 drain 最多合并的 PTY 输出事件，避免高频 TUI 输出触发大量小重绘。
const OUTPUT_BATCH_LIMIT: usize = 128;
/// 左侧时间戳 gutter 的固定宽度，保证终端列数不会随文字内容抖动。
const TIMESTAMP_GUTTER_WIDTH: f32 = 104.0;
/// 时间戳文本与 gutter 两侧的间距。
const TIMESTAMP_GUTTER_PADDING: f32 = 8.0;
/// gutter 分隔线与终端第 0 列之间的视觉留白。
const TIMESTAMP_GUTTER_GAP: f32 = 8.0;
/// OSC 52 单次剪贴板文本上限。
const MAX_OSC52_CLIPBOARD_BYTES: usize = 1024 * 1024;
const MAX_OSC52_RESPONSE_BYTES: usize = MAX_OSC52_CLIPBOARD_BYTES * 2;
const MAX_TERMINAL_IMAGES: usize = 64;
const MAX_PENDING_KITTY_NOTIFICATIONS: usize = 128;
const MAX_KITTY_IMAGE_BYTES: usize = 6 * 1024 * 1024;
const MAX_DECODED_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: usize = 16 * 1024;
const KITTY_BACKGROUND_Z_INDEX: i32 = -1_073_741_824;
const KITTY_PLACEHOLDER_CHAR: char = '\u{10eeee}';

fn default_local_shell_name() -> String {
    #[cfg(windows)]
    {
        std::env::var_os("ComSpec")
            .or_else(|| std::env::var_os("SHELL"))
            .map(|shell| shell.to_string_lossy().into_owned())
            .unwrap_or_else(|| "powershell.exe".to_owned())
    }

    #[cfg(not(windows))]
    {
        std::env::var_os("SHELL")
            .map(|shell| shell.to_string_lossy().into_owned())
            .unwrap_or_else(|| "sh".to_owned())
    }
}

impl EventEmitter<TerminalEvent> for TerminalView {}

/// Events emitted by the terminal parser that require a UI/platform action.
/// They are buffered because `EventListener::send_event` has no GPUI context.
enum TerminalSideEffect {
    Bell,
    Title(String),
    ResetTitle,
    ClipboardStore(ClipboardType, String),
    ClipboardLoad(
        ClipboardType,
        Arc<dyn Fn(&str) -> String + Sync + Send + 'static>,
    ),
    ColorRequest(usize, Arc<dyn Fn(Rgb) -> String + Sync + Send + 'static>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WheelRoute {
    LocalScrollback,
    MouseReport,
    AlternateScroll,
}

fn wheel_route(mode: TermMode, shift: bool) -> WheelRoute {
    if mode.intersects(TermMode::MOUSE_MODE) && !shift {
        WheelRoute::MouseReport
    } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) && !shift {
        WheelRoute::AlternateScroll
    } else {
        WheelRoute::LocalScrollback
    }
}

fn wheel_lines_for_phase(
    phase: TouchPhase,
    delta: ScrollDelta,
    scroll_acc: &mut f32,
    line_height: f32,
) -> Option<i32> {
    match phase {
        TouchPhase::Started => {
            *scroll_acc = 0.;
            None
        }
        TouchPhase::Moved => {
            let line_height = line_height.max(1.);
            let delta_pixels = match delta {
                ScrollDelta::Lines(delta) => delta.y * line_height,
                ScrollDelta::Pixels(delta) => delta.y.as_f32(),
            };
            *scroll_acc += delta_pixels;
            let lines = (*scroll_acc / line_height) as i32;
            *scroll_acc -= lines as f32 * line_height;
            (lines != 0).then_some(lines)
        }
        TouchPhase::Ended | TouchPhase::Cancelled => None,
    }
}

fn alternate_scroll_sequence(mode: TermMode, key: u8) -> [u8; 3] {
    if mode.contains(TermMode::APP_CURSOR) {
        [0x1b, b'O', key]
    } else {
        [0x1b, b'[', key]
    }
}

type WindowSizeHandle = Arc<Mutex<WindowSize>>;
type SideEffectQueue = Arc<Mutex<VecDeque<TerminalSideEffect>>>;
/// 解析器同步路径使用的非阻塞回复暂存区。
type ProtocolResponseQueue = Arc<Mutex<VecDeque<Vec<u8>>>>;

/// alacritty EventListener：把终端模拟器需要回写的响应送回远端 PTY。
#[derive(Clone)]
struct NoopListener {
    window_size: WindowSizeHandle,
    side_effects: SideEffectQueue,
    protocol_responses: ProtocolResponseQueue,
}

impl Default for NoopListener {
    fn default() -> Self {
        Self {
            window_size: Arc::new(Mutex::new(WindowSize {
                num_lines: 30,
                num_cols: 100,
                cell_width: 8,
                cell_height: (FONT_SIZE * 1.3) as u16,
            })),
            side_effects: Arc::new(Mutex::new(VecDeque::new())),
            protocol_responses: Arc::new(Mutex::new(VecDeque::new())),
        }
    }
}

impl NoopListener {
    fn for_bridge(
        cols: usize,
        rows: usize,
    ) -> (
        Self,
        WindowSizeHandle,
        SideEffectQueue,
        ProtocolResponseQueue,
    ) {
        let window_size = Arc::new(Mutex::new(WindowSize {
            num_lines: rows as u16,
            num_cols: cols as u16,
            cell_width: 8,
            cell_height: (FONT_SIZE * 1.3) as u16,
        }));
        let side_effects = Arc::new(Mutex::new(VecDeque::new()));
        let protocol_responses = Arc::new(Mutex::new(VecDeque::new()));
        (
            Self {
                window_size: window_size.clone(),
                side_effects: side_effects.clone(),
                protocol_responses: protocol_responses.clone(),
            },
            window_size,
            side_effects,
            protocol_responses,
        )
    }

    fn queue_side_effect(&self, effect: TerminalSideEffect) {
        if let Ok(mut effects) = self.side_effects.lock() {
            effects.push_back(effect);
        }
    }

    fn write_to_pty(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        // EventListener::send_event is synchronous. Never wait for the relay
        // here: the Entity drains this queue into its ordered input backlog.
        match self.protocol_responses.lock() {
            Ok(mut responses) => responses.push_back(bytes),
            Err(poisoned) => poisoned.into_inner().push_back(bytes),
        }
    }
}

impl EventListener for NoopListener {
    fn send_event(&self, event: Event) {
        match event {
            // DSR/DA/mode reports are generated by alacritty as PtyWrite events.
            // Dropping them makes applications that probe the terminal state hang
            // or fall back to a much less capable input mode.
            Event::PtyWrite(text) => self.write_to_pty(text.into_bytes()),
            Event::TextAreaSizeRequest(format) => {
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
                self.write_to_pty(format(size).into_bytes());
            }
            Event::Title(title) => self.queue_side_effect(TerminalSideEffect::Title(title)),
            Event::ResetTitle => self.queue_side_effect(TerminalSideEffect::ResetTitle),
            Event::ClipboardStore(ty, text) => {
                self.queue_side_effect(TerminalSideEffect::ClipboardStore(ty, text));
            }
            Event::ClipboardLoad(ty, formatter) => {
                self.queue_side_effect(TerminalSideEffect::ClipboardLoad(ty, formatter));
            }
            Event::ColorRequest(index, formatter) => {
                self.queue_side_effect(TerminalSideEffect::ColorRequest(index, formatter));
            }
            Event::Bell => self.queue_side_effect(TerminalSideEffect::Bell),
            _ => {}
        }
    }
}

/// 终端尺寸（用于构造 Term）。
struct TermSize {
    cols: usize,
    rows: usize,
}
impl alacritty_terminal::grid::Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

#[derive(Clone)]
struct TerminalImage {
    image: Arc<gpui::Image>,
    kitty_id: Option<u32>,
    placement_id: Option<u32>,
    /// Absolute line in the terminal grid: history size + screen-relative line.
    origin_line: i64,
    origin_col: usize,
    width: Option<ImageDimension>,
    height: Option<ImageDimension>,
    preserve_aspect_ratio: bool,
    offset_x: usize,
    offset_y: usize,
    z_index: i32,
    virtual_placement: bool,
    relative_image_id: Option<u32>,
    relative_placement_id: Option<u32>,
    relative_offset_x: i32,
    relative_offset_y: i32,
}

#[derive(Clone, Copy, Debug, Default)]
struct KittyPlacement {
    source_x: Option<usize>,
    source_y: Option<usize>,
    source_width: Option<usize>,
    source_height: Option<usize>,
    offset_x: usize,
    offset_y: usize,
    z_index: i32,
    relative_image_id: Option<u32>,
    relative_placement_id: Option<u32>,
    relative_offset_x: i32,
    relative_offset_y: i32,
}

#[derive(Default)]
struct KittyImageData {
    data: Vec<u8>,
    action: Option<String>,
    format: Option<u32>,
    width: Option<usize>,
    height: Option<usize>,
    columns: Option<usize>,
    rows: Option<usize>,
    placement_id: Option<u32>,
    do_not_move_cursor: bool,
    compressed: bool,
    source_x: Option<usize>,
    source_y: Option<usize>,
    source_width: Option<usize>,
    source_height: Option<usize>,
    offset_x: usize,
    offset_y: usize,
    z_index: i32,
    image_number: Option<u32>,
    virtual_placement: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct KittyPlaceholder {
    image_id: u32,
    placement_id: Option<u32>,
    row: usize,
    column: usize,
    viewport_row: usize,
    viewport_column: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct KittyPlaceholderState {
    foreground: u32,
    underline: Option<u32>,
    row: usize,
    column: usize,
    image_id_high: u8,
}

#[derive(Clone, Copy)]
struct TerminalProgress {
    state: u8,
    progress: Option<u8>,
}

struct PendingKittyNotification {
    title: String,
    body: String,
    occasion: NotificationOccasion,
    report_activation: bool,
    report_close: bool,
    focus_on_activation: bool,
    expiry_ms: Option<i64>,
    buttons: Vec<String>,
}

struct KittyNotificationUpdate {
    id: String,
    title: Option<String>,
    body: Option<String>,
    complete: bool,
    occasion: Option<NotificationOccasion>,
    report_activation: Option<bool>,
    report_close: Option<bool>,
    focus_on_activation: Option<bool>,
    expiry_ms: Option<i64>,
    buttons: Option<Vec<String>>,
}

struct DisplayNotification {
    kitty_id: Option<String>,
    title: String,
    body: String,
    occasion: NotificationOccasion,
    report_activation: bool,
    report_close: bool,
    focus_on_activation: bool,
    buttons: Vec<String>,
    expiry_ms: Option<i64>,
}

impl Default for PendingKittyNotification {
    fn default() -> Self {
        Self {
            title: String::new(),
            body: String::new(),
            occasion: NotificationOccasion::Always,
            report_activation: false,
            report_close: false,
            focus_on_activation: true,
            expiry_ms: None,
            buttons: Vec::new(),
        }
    }
}

#[derive(Clone)]
struct NotificationState {
    tag: String,
    kitty_id: Option<String>,
    report_activation: bool,
    report_close: bool,
    focus_on_activation: bool,
}

fn notification_state_for_tag(
    states: &HashMap<String, NotificationState>,
    tag: &str,
) -> Option<(String, NotificationState)> {
    states
        .iter()
        .find_map(|(key, state)| (state.tag == tag).then_some((key.clone(), state.clone())))
}

pub struct TerminalView {
    term: Term<NoopListener>,
    parser: Processor,
    input_tx: Sender<InputCmd>,
    pending_input: VecDeque<InputCmd>,
    pub state: ConnState,
    command_running: bool,
    shell_activity_available: bool,
    protocol_parser: TerminalProtocolParser,
    /// 当前 shell 报告的工作目录；用于本地终端状态栏和 Git 状态。
    pub cwd: Option<String>,
    focus: FocusHandle,
    cell_w: Pixels,
    line_h: Pixels,
    cols: usize,
    rows: usize,
    /// canvas 在窗口坐标中的原点，用于把 GPUI 鼠标位置转换到终端网格。
    content_origin: Point<Pixels>,
    window_size: Arc<Mutex<WindowSize>>,
    side_effects: Arc<Mutex<VecDeque<TerminalSideEffect>>>,
    protocol_responses: ProtocolResponseQueue,
    /// Local PTYs retain OSC 52 paste; remote sessions deny clipboard reads.
    is_local: bool,
    font: Font,
    font_size: f32,
    scrollback: usize,
    show_timestamps: bool,
    _drain: Option<Task<()>>,
    focused_once: bool,
    _focus_in: Option<Subscription>,
    _focus_out: Option<Subscription>,
    /// 文本选择：viewport 内 (col, row) 起点/终点。
    sel_start: Option<(usize, usize)>,
    sel_end: Option<(usize, usize)>,
    selecting: bool,
    /// macOS/GPUI 输入法的临时合成文本。它不能直接写入 PTY，只有提交后的文本才发送。
    ime_marked_text: String,
    remote_mouse_button: Option<u8>,
    /// 累积滚动偏移（trackpad 累加用）。
    scroll_acc: f32,
    /// 光标闪烁状态（true = 显示，false = 隐藏）。
    cursor_blink_on: bool,
    /// xterm's legacy CSI 1015 mouse mode is not modeled by alacritty yet.
    urxvt_mouse: bool,
    /// xterm modifyOtherKeys level (0 means the legacy encoding is active).
    modify_other_keys: u8,
    /// GPUI focus state, used to avoid raising a notification for the active terminal.
    focused: bool,
    notifications_enabled: bool,
    progress: Option<TerminalProgress>,
    images: Vec<TerminalImage>,
    kitty_image_data: HashMap<u32, KittyImageData>,
    kitty_image_numbers: HashMap<u32, u32>,
    next_kitty_image_id: u32,
    kitty_active_image_id: Option<u32>,
    kitty_notifications: HashMap<String, PendingKittyNotification>,
    notification_states: HashMap<String, NotificationState>,
    notification_state_order: VecDeque<String>,
    kitty_notification_expiry: HashMap<String, Task<()>>,
    notification_serial: u64,
    _sync_timeout_task: Option<Task<()>>,
    _blink_task: Option<Task<()>>,
    /// 最近一帧检测到的 URL（用于点击跳转）。
    detected_urls: Vec<(usize, usize, usize, String)>,
    /// 终端行的旁路时间戳；绝不写入 PTY 或 alacritty 的字符网格。
    line_timestamps: TerminalTimestampState,
    /// 由 OSC 标题序列设置的窗口标题。
    title: Option<String>,
    /// 本地 PTY 前台进程快照，用于生成动态 tab 标题。
    process_info: Option<TerminalProcessInfo>,
    /// 本地 PTY 的 shell 路径，进程快照暂不可用时作为 fallback。
    local_shell: Option<String>,
    /// 当前打开的右键上下文菜单。
    context_menu: Option<ContextMenuState<TerminalMenuAction>>,
    /// canvas 在窗口坐标中的 bounds（右键菜单定位/外点关闭用）。
    anchor_bounds: Rc<StdCell<Option<Bounds<Pixels>>>>,
    /// 诊断：最近一次成功执行到 render/drain 的时间（用于检测 UI 卡死）。
    last_progress: Instant,
    /// 诊断：累计处理的 SessionEvent 数。
    events_processed: u64,
}

impl TerminalView {
    /// 用一个已有的终端桥接（来自 `Connection::open_terminal`）创建视图。
    ///
    /// 连接本身由调用方（AppShell）经 `Connection` 管理；本视图只负责：
    ///  - 主线程 drain `event_rx` 喂 alacritty `Term`；
    ///  - 键盘/resize 经 `input_tx` 送回。
    pub fn from_bridge(
        input_tx: Sender<InputCmd>,
        event_rx: Receiver<SessionEvent>,
        cols: usize,
        rows: usize,
        cx: &mut App,
    ) -> Entity<Self> {
        Self::from_bridge_with_cwd(input_tx, event_rx, cols, rows, None, false, cx)
    }

    /// 创建一个本地 PTY 终端。除工作目录事件外，其渲染和交互路径与 SSH 终端一致。
    pub fn from_local_bridge(
        input_tx: Sender<InputCmd>,
        event_rx: Receiver<SessionEvent>,
        cols: usize,
        rows: usize,
        cwd: String,
        cx: &mut App,
    ) -> Entity<Self> {
        Self::from_bridge_with_cwd(input_tx, event_rx, cols, rows, Some(cwd), true, cx)
    }

    fn from_bridge_with_cwd(
        input_tx: Sender<InputCmd>,
        event_rx: Receiver<SessionEvent>,
        cols: usize,
        rows: usize,
        initial_cwd: Option<String>,
        is_local: bool,
        cx: &mut App,
    ) -> Entity<Self> {
        let settings = i18n::settings(cx);
        let config = Config {
            scrolling_history: settings.terminal_scrollback,
            kitty_keyboard: true,
            osc52: osc52_mode(is_local),
            ..Default::default()
        };
        let size = TermSize { cols, rows };
        let (listener, window_size, side_effects, protocol_responses) =
            NoopListener::for_bridge(cols, rows);
        let font = Font {
            family: "Menlo".into(),
            weight: FontWeight::NORMAL,
            style: gpui::FontStyle::Normal,
            features: Default::default(),
            fallbacks: None,
        };

        let entity = cx.new(|cx| Self {
            term: Term::new(config, &size, listener),
            parser: Processor::new(),
            input_tx: input_tx.clone(),
            pending_input: VecDeque::new(),
            state: ConnState::Connecting,
            command_running: false,
            shell_activity_available: false,
            protocol_parser: TerminalProtocolParser::default(),
            cwd: initial_cwd,
            focus: cx.focus_handle(),
            cell_w: px(0.),
            line_h: px(settings.terminal_font_size * 1.3),
            cols,
            rows,
            content_origin: Point::new(px(0.), px(0.)),
            window_size,
            side_effects,
            protocol_responses,
            is_local,
            font,
            font_size: settings.terminal_font_size,
            scrollback: settings.terminal_scrollback,
            show_timestamps: settings.show_timestamps,
            _drain: None,
            focused_once: false,
            _focus_in: None,
            _focus_out: None,
            sel_start: None,
            sel_end: None,
            selecting: false,
            ime_marked_text: String::new(),
            remote_mouse_button: None,
            scroll_acc: 0.,
            cursor_blink_on: true,
            urxvt_mouse: false,
            modify_other_keys: 0,
            focused: true,
            notifications_enabled: settings.terminal_notifications,
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
            _sync_timeout_task: None,
            _blink_task: None,
            detected_urls: Vec::new(),
            line_timestamps: TerminalTimestampState::default(),
            title: None,
            process_info: None,
            local_shell: is_local.then(default_local_shell_name),
            context_menu: None,
            anchor_bounds: Rc::new(StdCell::new(None)),
            last_progress: Instant::now(),
            events_processed: 0,
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

    pub fn apply_settings(&mut self, settings: AppSettings, cx: &mut Context<Self>) {
        let font_size = settings
            .terminal_font_size
            .clamp(i18n::MIN_TERMINAL_FONT_SIZE, i18n::MAX_TERMINAL_FONT_SIZE);
        if (self.font_size - font_size).abs() > f32::EPSILON {
            self.font_size = font_size;
            self.line_h = px(font_size * 1.3);
            self.cell_w = px(0.);
            if let Ok(mut size) = self.window_size.lock() {
                size.cell_height = self.line_h.as_f32().round().max(1.) as u16;
            }
        }

        if self.scrollback != settings.terminal_scrollback {
            self.scrollback = settings.terminal_scrollback;
            self.term.set_options(Config {
                scrolling_history: self.scrollback,
                kitty_keyboard: true,
                osc52: osc52_mode(self.is_local),
                ..Default::default()
            });
        }

        self.show_timestamps = settings.show_timestamps;
        self.notifications_enabled = settings.terminal_notifications;
        cx.notify();
    }

    /// Ask the PTY/SSH channel to close cleanly before its entity is dropped.
    pub(crate) fn request_close(&mut self) {
        self.drain_protocol_responses();
        self.queue_input(InputCmd::Close);
        self.flush_pending_input();
    }

    /// 处理一批来自 drain 循环的 SessionEvent，并维护诊断心跳。
    fn apply_session_event_batch(&mut self, events: Vec<SessionEvent>, cx: &mut Context<Self>) {
        let now = Instant::now();
        for event in events {
            self.apply_session_event(event, cx);
        }
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

    fn apply_session_event(&mut self, event: SessionEvent, cx: &mut Context<Self>) {
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
                let was_alt_screen = self.term.mode().contains(TermMode::ALT_SCREEN);
                let timestamp = (!was_alt_screen).then(|| format_timestamp(Local::now()));
                self.parser.advance(&mut self.term, &bytes);
                let protocol_events = self.protocol_parser.feed(&bytes);
                self.process_protocol_events(protocol_events, cx, 0);
                self.schedule_sync_timeout(cx);
                self.drain_terminal_side_effects(cx);
                self.drain_protocol_responses();
                if !self.term.mode().contains(TermMode::ALT_SCREEN) {
                    self.line_timestamps.observe(
                        &self.term,
                        timestamp.unwrap_or_else(|| format_timestamp(Local::now())),
                    );
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
                if was_connected {
                    cx.emit(TerminalEvent::Closed);
                }
            }
        }
    }

    fn process_protocol_events(
        &mut self,
        events: Vec<ProtocolEvent>,
        cx: &mut Context<Self>,
        passthrough_depth: usize,
    ) {
        for event in events {
            match event {
                ProtocolEvent::Cwd(cwd) => {
                    if self.cwd.as_deref() != Some(cwd.as_str()) {
                        self.cwd = Some(cwd);
                        cx.emit(TerminalEvent::CwdChanged);
                    }
                }
                ProtocolEvent::Shell(shell_event) => {
                    self.shell_activity_available = true;
                    match shell_event {
                        ShellEvent::PromptStart => {
                            self.command_running = false;
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
                ProtocolEvent::ModifyOtherKeys(level) => self.modify_other_keys = level,
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
                    self.parser.advance(&mut self.term, &bytes);
                }
                ProtocolEvent::Reset
                | ProtocolEvent::ClearImages
                | ProtocolEvent::ScreenBufferSwitch(_) => {
                    self.images.clear();
                    self.kitty_image_data.clear();
                    self.kitty_image_numbers.clear();
                    self.kitty_active_image_id = None;
                }
            }
        }
    }

    fn process_kitty_notification(
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

    fn notify_user(
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

    fn display_notification(&mut self, notification: DisplayNotification, cx: &mut Context<Self>) {
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

    fn insert_notification_state(&mut self, key: String, state: NotificationState) {
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

    fn remove_notification_state(&mut self, key: &str) -> Option<NotificationState> {
        let state = self.notification_states.remove(key);
        if state.is_some() {
            self.notification_state_order
                .retain(|existing| existing != key);
        }
        state
    }

    fn should_show_notification(&self, occasion: NotificationOccasion) -> bool {
        match occasion {
            NotificationOccasion::Always => true,
            NotificationOccasion::Unfocused | NotificationOccasion::Invisible => !self.focused,
        }
    }

    fn close_kitty_notification(&mut self, id: &str, cx: &mut Context<Self>) {
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

    fn respond_kitty_notification_query(&mut self, id: &str) {
        let id = sanitize_kitty_notification_id(id).unwrap_or_default();
        self.send_input(
            format!(
                "\x1b]99;i={id}:p=?;a=report,focus:o=always,unfocused:p=title,body,close,alive,buttons:w=1\x1b\\"
            )
            .into_bytes(),
        );
    }

    fn respond_kitty_notification_alive(&mut self, id: &str) {
        let id = sanitize_kitty_notification_id(id).unwrap_or_default();
        let mut alive = self
            .notification_states
            .values()
            .filter_map(|state| state.kitty_id.clone())
            .collect::<Vec<_>>();
        alive.sort();
        self.send_input(format!("\x1b]99;i={id}:p=alive;{}\x1b\\", alive.join(",")).into_bytes());
    }

    fn send_kitty_notification_close(&mut self, id: &str, untracked: bool) {
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

    fn store_image(&mut self, payload: ImagePayload) {
        let width = image_dimension_cells(payload.width);
        let height = image_dimension_cells(payload.height);
        let do_not_move_cursor = payload.do_not_move_cursor;
        if self.store_image_with_id(payload, None, None, KittyPlacement::default(), false) {
            self.advance_image_cursor(width, height, do_not_move_cursor);
        }
    }

    fn store_image_with_id(
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
        let grid = self.term.grid();
        let origin_line = grid.history_size() as i64 + grid.cursor.point.line.0 as i64;
        let image = TerminalImage {
            image: Arc::new(gpui::Image::from_bytes(format, payload.data)),
            kitty_id,
            placement_id,
            origin_line,
            origin_col: grid.cursor.point.column.0,
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

    fn advance_image_cursor(
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
            self.parser.advance(&mut self.term, &sequence);
        }
    }

    fn process_sixel(&mut self, data: Vec<u8>) {
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

    fn allocate_kitty_image_id(&mut self) -> u32 {
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

    fn process_kitty_graphics(&mut self, payload: KittyGraphicsPayload) {
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
            let history_size = self.term.grid().history_size() as i64;
            let cursor_line = history_size + self.term.grid().cursor.point.line.0 as i64;
            let cursor_column = self.term.grid().cursor.point.column.0;
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

    fn respond_kitty_graphics(&mut self, control: &str, message: &str) {
        let image_id = kitty_parameter(control, "i")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        let image_number =
            kitty_parameter(control, "I").and_then(|value| value.parse::<u32>().ok());
        self.respond_kitty_graphics_for_image(control, image_id, image_number, message);
    }

    fn respond_kitty_graphics_for_image(
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

    fn respond_decrqss(&mut self, query: &[u8]) {
        let value = match query {
            b"m" => Some("0m".to_string()),
            b" q" => {
                let style = self.term.cursor_style();
                let style_id = match (style.shape, style.blinking) {
                    (CursorShape::Block, true) => 1,
                    (CursorShape::Block, false) => 2,
                    (CursorShape::Underline, true) => 3,
                    (CursorShape::Underline, false) => 4,
                    (CursorShape::Beam, true) => 5,
                    (CursorShape::Beam, false) => 6,
                    (CursorShape::HollowBlock, _) | (CursorShape::Hidden, _) => 2,
                };
                Some(format!("{style_id} q"))
            }
            b"r" => Some(format!("1;{}r", self.term.screen_lines())),
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

    fn respond_xtgettcap(&mut self, query: &[u8]) {
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

    fn respond_modify_other_keys_query(&mut self) {
        self.send_input(format!("\x1b[>4;{}m", self.modify_other_keys).into_bytes());
    }

    fn respond_cell_size_query(&mut self) {
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

    fn respond_window_size_query(&mut self) {
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

    fn respond_text_area_size_query(&mut self) {
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

    fn drain_terminal_side_effects(&mut self, cx: &mut Context<Self>) {
        let effects = self
            .side_effects
            .lock()
            .map(|mut queue| queue.drain(..).collect::<Vec<_>>())
            .unwrap_or_default();

        for effect in effects {
            match effect {
                TerminalSideEffect::Bell => {
                    self.notify_user(
                        self.title.clone().unwrap_or_default(),
                        "Terminal bell".to_string(),
                        false,
                        cx,
                    );
                }
                TerminalSideEffect::Title(title) => {
                    self.title = Some(title);
                    cx.emit(TerminalEvent::TitleChanged);
                }
                TerminalSideEffect::ResetTitle => {
                    self.title = None;
                    cx.emit(TerminalEvent::TitleChanged);
                }
                TerminalSideEffect::ClipboardStore(clipboard, text) => {
                    if !osc52_text_within_limit(&text) {
                        log::warn!(
                            "ignoring oversized OSC 52 clipboard write ({} bytes)",
                            text.len()
                        );
                        continue;
                    }
                    let item = gpui::ClipboardItem::new_string(text);
                    // GPUI exposes the platform clipboard consistently across
                    // desktop targets; Selection is treated as the same store
                    // when a primary-selection API is unavailable.
                    let _ = clipboard;
                    cx.write_to_clipboard(item);
                }
                TerminalSideEffect::ClipboardLoad(_clipboard, formatter) => {
                    if !osc52_load_allowed(self.is_local) {
                        log::debug!("ignoring OSC 52 clipboard read from remote terminal");
                        continue;
                    }
                    let item = cx.read_from_clipboard();
                    if let Some(text) = item.and_then(|item| item.text()) {
                        if let Some(response) = format_osc52_response(&formatter, &text) {
                            self.send_input(response);
                        } else {
                            log::warn!(
                                "ignoring oversized OSC 52 clipboard response ({} bytes)",
                                text.len()
                            );
                        }
                    }
                }
                TerminalSideEffect::ColorRequest(index, formatter) => {
                    if index < alacritty_terminal::term::color::COUNT
                        && let Some(color) =
                            self.term.colors()[index].or_else(|| default_palette_rgb_index(index))
                    {
                        self.send_input(formatter(color).into_bytes());
                    }
                }
            }
        }
    }

    /// Move parser-generated protocol replies into the same ordered queue used
    /// for keyboard input. Locking is bounded to a short drain operation and
    /// never awaits the channel consumer.
    fn drain_protocol_responses(&mut self) {
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

    pub(crate) fn is_command_running(&self) -> bool {
        self.state == ConnState::Connected
            && (self.command_running || self.term.mode().contains(TermMode::ALT_SCREEN))
    }

    /// 发送输入字节到远端。
    fn send_input(&mut self, bytes: Vec<u8>) {
        log::trace!("pty write: {}", debug_bytes(&bytes));
        self.queue_input(InputCmd::Write(bytes));
    }

    /// The standalone UI drives `vte::Processor` directly instead of using
    /// Alacritty's PTY event loop, so it must enforce the synchronized-update
    /// deadline itself. Otherwise an interrupted `?2026h` sequence can leave
    /// the grid buffered indefinitely.
    fn finish_expired_sync_update(&mut self) {
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

    fn schedule_sync_timeout(&mut self, cx: &mut Context<Self>) {
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
    fn queue_input(&mut self, command: InputCmd) {
        queue_input_nonblocking(&self.input_tx, &mut self.pending_input, command);
    }

    fn flush_pending_input(&mut self) {
        flush_pending_commands(&self.input_tx, &mut self.pending_input);
    }

    /// 请求在下次 render 时自动聚焦终端（用于打开/切回 tab）。
    pub fn request_focus(&mut self) {
        self.focused_once = false;
    }

    /// 根据当前尺寸调整远端 PTY 窗口。
    fn maybe_resize(&mut self, bounds: Size) {
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

    fn handle_key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
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
        let mode = *self.term.mode();
        let event_type = if ev.is_held { 2 } else { 1 };
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

    fn handle_key_up(&mut self, ev: &KeyUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        if is_shell_shortcut(ks)
            || (ks.modifiers.platform
                && !ks.modifiers.alt
                && !ks.modifiers.control
                && matches!(ks.key.as_str(), "c" | "v"))
        {
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

    fn send_focus_event(&mut self, focused: bool) {
        if self.state == ConnState::Connected && self.term.mode().contains(TermMode::FOCUS_IN_OUT) {
            self.send_input(if focused {
                b"\x1b[I".to_vec()
            } else {
                b"\x1b[O".to_vec()
            });
        }
    }

    fn handle_mouse_down(
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

    fn handle_mouse_up(&mut self, ev: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
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

    fn handle_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
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

    fn handle_scroll_wheel(
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
    fn pos_to_grid(&self, pos: Point<Pixels>) -> Option<(usize, usize)> {
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
    fn copy_selection(&mut self, cx: &mut Context<Self>) {
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
    fn paste_clipboard(&mut self, _cx: &mut Context<Self>) {
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
    fn select_all(&mut self) {
        if self.cols == 0 || self.rows == 0 {
            return;
        }
        self.sel_start = Some((0, 0));
        self.sel_end = Some((self.cols.saturating_sub(1), self.rows.saturating_sub(1)));
    }

    /// 右键打开上下文菜单；外部点击监听在 canvas 的 paint 阶段注册。
    fn open_terminal_context_menu(
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

    fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu.take().is_some() {
            cx.notify();
        }
    }

    fn dispatch_menu_action(&mut self, action: TerminalMenuAction, cx: &mut Context<Self>) {
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
    fn url_at(&self, col: usize, row: usize) -> Option<String> {
        for &(r, cs, ce, ref url) in &self.detected_urls {
            if r == row && col >= cs && col < ce {
                return Some(url.clone());
            }
        }
        None
    }

    /// 从 grid 中提取选择区域内的文本。
    fn extract_selection_text(&self, sx: usize, sy: usize, ex: usize, ey: usize) -> String {
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
    fn ime_cursor_bounds(&self, _element_bounds: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
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

        Some(Bounds {
            origin: Point::new(
                self.content_origin.x + px(visual_col as f32 * self.cell_w.as_f32()),
                self.content_origin.y + px(row as f32 * self.line_h.as_f32()),
            ),
            size: gpui::size(px(width_cells as f32 * self.cell_w.as_f32()), self.line_h),
        })
    }
}

fn selection_column_bounds(sx: usize, sy: usize, ex: usize, ey: usize) -> (usize, usize) {
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
struct TerminalInputHandler {
    terminal: Entity<TerminalView>,
    element_bounds: Bounds<Pixels>,
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

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        // A terminal is not a random-access text document. Returning the
        // marked text here makes AppKit treat the terminal grid as if it were
        // an editable document and can cause dictation/IME commits to be
        // applied twice. The marked text is only a visual composition buffer.
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        // The terminal has no document selection for the IME to replace. Keep
        // the insertion point at the terminal cursor, matching Zed's
        // terminal input contract.
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        (!self.ime_marked_text.is_empty()).then(|| 0..utf16_len(&self.ime_marked_text))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ime_marked_text.clear();
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ime_marked_text.clear();
        if !text.is_empty() {
            self.send_input(text.as_bytes().to_vec());
        }
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ime_marked_text.clear();
        self.ime_marked_text.push_str(new_text);
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.ime_cursor_bounds(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        // A terminal is not a random-access text document. The IME only needs
        // the virtual marked text and the cursor bounds supplied above.
        None
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        !matches!(self.state, ConnState::Error(_) | ConnState::Closed)
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.finish_expired_sync_update();
        self.flush_pending_input();

        // 诊断：如果 render 帧间隔异常大，说明主线程之前卡住了很久。
        let now = Instant::now();
        let stalled = now.saturating_duration_since(self.last_progress);
        self.last_progress = now;
        if stalled > Duration::from_secs(5) {
            log::warn!(
                "terminal: UI stalled {}s before this frame (events_processed={})",
                stalled.as_secs(),
                self.events_processed
            );
        }

        if self._focus_in.is_none() {
            let focus = self.focus.clone();
            self._focus_in = Some(cx.on_focus_in(&focus, window, |this, _window, cx| {
                this.focused = true;
                this.send_focus_event(true);
                cx.notify();
            }));
            let focus = self.focus.clone();
            self._focus_out = Some(
                cx.on_focus_out(&focus, window, |this, _event, _window, cx| {
                    this.focused = false;
                    this.send_focus_event(false);
                    cx.notify();
                }),
            );
        }

        // 打开/切回 tab 时自动聚焦终端，确保键盘输入直达 PTY（否则 on_key_down 不会触发）。
        if !self.focused_once && !matches!(self.state, ConnState::Error(_)) {
            self.focus.focus(window, cx);
            self.focused_once = true;
        }
        let focus = self.focus.clone();
        let entity = cx.entity();

        // 状态分支：未连接时显示提示。
        let status_overlay = match &self.state {
            ConnState::Connecting => Some(i18n::text("terminal.connecting")),
            ConnState::Error(e) => {
                return connecting_or_error_view(e, &focus).into_any_element();
            }
            ConnState::Closed => Some(i18n::text("terminal.closed")),
            ConnState::Connected => None,
        };

        // 确保字体度量已测量：paint 闭包在 render 时就捕获 cell_w，
        // 必须此时拿到的不是 0，否则首帧光标/布局会错位（视觉上像「刷新」）。
        if self.cell_w.as_f32() <= 0.0 {
            let run = TextRun {
                len: 1,
                font: self.font.clone(),
                color: hsla(0., 0., 1., 1.),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped =
                window
                    .text_system()
                    .shape_line("M".into(), px(self.font_size), &[run], None);
            self.cell_w = shaped.width().max(px(1.0));
        }

        // 捕获绘制所需的克隆。
        let bg = bg_of(&self.term);
        let font = self.font.clone();
        let font_size = self.font_size;
        let show_timestamps = self.show_timestamps;
        let cell_w = self.cell_w;
        let line_h = self.line_h;
        let weak = entity.downgrade();
        let weak2 = weak.clone();
        let context_menu_state = self.context_menu.clone();
        let context_menu_weak = entity.downgrade();
        let context_menu_anchor = self.anchor_bounds.clone();
        let input_entity = entity.clone();
        let input_focus = focus.clone();
        let default_fg = fg_of(&self.term);

        let canvas_el = canvas(
            move |bounds, _window, cx| {
                // prepaint：可能 resize。
                if let Some(t) = weak.upgrade() {
                    t.update(cx, |this, _cx| {
                        this.anchor_bounds.set(Some(bounds));
                        let terminal_bounds = terminal_bounds_for(bounds, show_timestamps);
                        this.content_origin = terminal_bounds.origin;
                        this.maybe_resize(Size {
                            w: terminal_bounds.size.width.as_f32(),
                            h: terminal_bounds.size.height.as_f32(),
                        });
                    });
                }
                bounds
            },
            move |bounds, _pre, window, cx| {
                if let Some(menu) = context_menu_state.as_ref() {
                    let menu_position = clamp_menu_position(menu.position, window, &menu.entries);
                    // Include padding, border, and the small gap between rows.
                    // This is intentionally conservative so the outside-click
                    // listener cannot dismiss a menu item near an edge.
                    let menu_bounds = Bounds {
                        origin: menu_position,
                        size: gpui::size(
                            px(CONTEXT_MENU_WIDTH + 32.0),
                            px(estimate_menu_height(&menu.entries) + 32.0),
                        ),
                    };
                    let weak = context_menu_weak.clone();
                    let anchor = context_menu_anchor.clone();
                    window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
                        if !matches!(phase, gpui::DispatchPhase::Capture) {
                            return;
                        }
                        let closed = weak
                            .update(cx, |this, _| {
                                let inside_menu = menu_bounds.contains(&ev.position);
                                let outside = anchor
                                    .get()
                                    .is_some_and(|bounds| !bounds.contains(&ev.position));
                                if this.context_menu.is_some() && outside && !inside_menu {
                                    this.context_menu = None;
                                    true
                                } else {
                                    false
                                }
                            })
                            .unwrap_or(false);
                        if closed {
                            // 外部点击只关菜单，不再触发下方控件（如侧栏主机）。
                            cx.stop_propagation();
                            window.refresh();
                        }
                    });
                }

                // GPUI 只有在 paint 阶段注册了 InputHandler，macOS 才会把
                // 中文/日文等合成输入交给当前终端，并能查询候选框坐标。
                window.handle_input(
                    &input_focus,
                    TerminalInputHandler {
                        terminal: input_entity.clone(),
                        element_bounds: bounds,
                    },
                    cx,
                );

                // paint：先快照可见单元格（避免持有对 cx 的不可变借用），
                // 再用 &mut App 绘制。
                if let Some(t) = weak2.upgrade() {
                    let (snapshot, ime_marked_text, images, progress) = {
                        let this = t.read(cx);
                        let sel = this.sel_start.zip(this.sel_end);
                        let cursor_style = this.term.cursor_style();
                        let show_cur = this.term.mode().contains(TermMode::SHOW_CURSOR)
                            && cursor_style.shape != CursorShape::Hidden
                            && (!cursor_style.blinking || this.cursor_blink_on);
                        let timestamps = if this.show_timestamps {
                            this.line_timestamps.visible(&this.term)
                        } else {
                            vec![None; this.term.screen_lines()]
                        };
                        (
                            snapshot_visible(&this.term, sel, this.cols, show_cur, &timestamps),
                            this.ime_marked_text.clone(),
                            this.images.clone(),
                            this.progress,
                        )
                    };
                    // 保存 URL 供点击跳转。
                    let urls = snapshot.urls.clone();
                    let _ = weak2.update(cx, |this, _| {
                        this.detected_urls = urls;
                    });
                    paint_snapshot(
                        &PaintContext {
                            snapshot: &snapshot,
                            ime_marked_text: &ime_marked_text,
                            canvas_bounds: bounds,
                            bounds: terminal_bounds_for(bounds, show_timestamps),
                            cell_w,
                            line_h,
                            font_size,
                            show_timestamps,
                            font: &font,
                            default_fg,
                            default_bg: bg,
                            images: &images,
                            progress,
                        },
                        window,
                        cx,
                    );
                }
            },
        )
        // 关键：canvas 必须显式占满父容器，否则默认 Style 的尺寸为 auto，
        // 在 flex 父容器里会塌缩成 0，导致 maybe_resize 算出 rows=1，
        // 多行输出全部滚出视口（只看得见最后一行的提示符）。
        .size_full();

        let mut root = div()
            .id("terminal-root")
            .size_full()
            .px_3()
            .py_2()
            .bg(bg)
            .track_focus(&focus)
            .on_key_down(cx.listener(TerminalView::handle_key_down))
            .on_key_up(cx.listener(TerminalView::handle_key_up))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::handle_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::handle_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::handle_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::handle_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::handle_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::handle_mouse_up))
            .on_mouse_move(cx.listener(Self::handle_mouse_move))
            .on_scroll_wheel(cx.listener(Self::handle_scroll_wheel))
            .on_click({
                let focus = focus.clone();
                move |_ev, window, cx| window.focus(&focus, cx)
            })
            .child(canvas_el);

        if let Some(msg) = status_overlay {
            root = root.child(
                div()
                    .absolute()
                    .top_2()
                    .left_2()
                    .px_2()
                    .py_1()
                    .rounded(px(theme::RADIUS_SM))
                    .bg(theme::raised())
                    .border_1()
                    .border_color(theme::border_strong())
                    .text_xs()
                    .text_color(theme::warning())
                    .child(SharedString::from(msg)),
            );
        }
        // 右键菜单（需在 status overlay 之后以保证 z 序最高）。
        if let Some(menu) = self.context_menu.clone() {
            let canvas_origin = self
                .anchor_bounds
                .get()
                .map(|bounds| bounds.origin)
                .unwrap_or_else(|| Point::new(px(0.), px(0.)));
            let anchor = Point::new(
                canvas_origin.x - px(TERMINAL_PADDING_X),
                canvas_origin.y - px(TERMINAL_PADDING_Y),
            );
            root = root.child(render_context_menu(
                &menu,
                anchor,
                window,
                cx,
                |this, action, _window, cx| this.dispatch_menu_action(action, cx),
                |this, cx| this.close_context_menu(cx),
            ));
        }
        root.into_any_element()
    }
}

fn connecting_or_error_view(msg: &str, focus: &FocusHandle) -> impl IntoElement {
    div()
        .id("terminal-error")
        .size_full()
        .bg(theme::canvas())
        .track_focus(focus)
        .flex()
        .items_center()
        .justify_center()
        .text_sm()
        .text_color(theme::warning())
        .child(SharedString::from(msg.to_string()))
}

/// 一个单元格的渲染快照（owned，避免绘制时持有对 term 的借用）。
#[derive(Clone)]
struct RenderCell {
    ch: char,
    fg: Hsla,
    bg: Hsla,
    bold: bool,
    italic: bool,
    underline: UnderlineKind,
    underline_color: Hsla,
    strikeout: bool,
    spacer: bool,
    wide: bool,
    zero_width: String,
    kitty_placeholder: bool,
    is_url: bool,
    hyperlink: Option<String>,
}

#[derive(Clone, Debug)]
struct RenderTextRun {
    start_col: usize,
    cell_count: usize,
    force_width_cells: usize,
    text: String,
    fg: Hsla,
    bold: bool,
    italic: bool,
    underline: UnderlineKind,
    underline_color: Hsla,
    strikeout: bool,
    is_url: bool,
}

/// GPUI exposes solid and wavy underlines. The other terminal underline modes
/// still retain their semantic presence and use the closest available paint style.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum UnderlineKind {
    #[default]
    None,
    Solid,
    Wavy,
}

#[derive(Clone, Copy)]
struct EffectiveCellStyle {
    fg: Hsla,
    bg: Hsla,
    bold: bool,
    italic: bool,
    underline: UnderlineKind,
    underline_color: Hsla,
    strikeout: bool,
}

/// Resolve the terminal cell's rendition before it reaches GPUI.
///
/// ANSI inverse is a cell attribute, not a color. It must therefore be
/// applied after both colors have been looked up in the current palette. This
/// also gives hidden text, dim text, and underline colors one consistent
/// place to resolve their interactions.
fn effective_cell_style(
    cell: &Cell,
    colors: &alacritty_terminal::term::color::Colors,
    default_fg: Hsla,
    default_bg: Hsla,
) -> EffectiveCellStyle {
    let mut fg_color = cell.fg;
    if cell.flags.contains(CellFlags::BOLD) && !cell.flags.contains(CellFlags::DIM) {
        fg_color = brighten_color(fg_color);
    }

    let mut fg = color_to_hsla(&fg_color, colors).unwrap_or(default_fg);
    let mut bg = color_to_hsla(&cell.bg, colors).unwrap_or(default_bg);
    if cell.flags.contains(CellFlags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }

    if cell.flags.contains(CellFlags::DIM) {
        fg = dimen(fg);
    }
    if cell.flags.contains(CellFlags::HIDDEN) {
        fg = bg;
    }

    let underline = if cell.flags.contains(CellFlags::UNDERCURL) {
        UnderlineKind::Wavy
    } else if cell.flags.intersects(CellFlags::ALL_UNDERLINES) {
        // GPUI currently has no dotted/dashed/double underline primitive. A
        // solid line is preferable to silently dropping the terminal style.
        UnderlineKind::Solid
    } else {
        UnderlineKind::None
    };
    let underline_color = cell
        .underline_color()
        .and_then(|color| color_to_hsla(&color, colors))
        .unwrap_or(fg);

    EffectiveCellStyle {
        fg,
        bg,
        bold: cell.flags.contains(CellFlags::BOLD),
        italic: cell.flags.contains(CellFlags::ITALIC),
        underline,
        underline_color,
        strikeout: cell.flags.contains(CellFlags::STRIKEOUT),
    }
}

fn brighten_color(color: Color) -> Color {
    match color {
        Color::Named(name) => Color::Named(name.to_bright()),
        other => other,
    }
}

impl RenderTextRun {
    fn from_cell(col: usize, cell: &RenderCell) -> Self {
        let mut text = String::with_capacity(cell.ch.len_utf8() + cell.zero_width.len());
        text.push(cell.ch);
        text.push_str(&cell.zero_width);

        let cell_width = if cell.wide { 2 } else { 1 };
        let is_url = cell.is_url || cell.hyperlink.is_some();
        Self {
            start_col: col,
            cell_count: cell_width,
            force_width_cells: cell_width,
            text,
            fg: if is_url {
                rgb_to_hsla(Rgb {
                    r: 0x4f,
                    g: 0xaf,
                    b: 0xff,
                })
            } else {
                cell.fg
            },
            bold: cell.bold,
            italic: cell.italic,
            underline: if is_url && cell.underline == UnderlineKind::None {
                UnderlineKind::Solid
            } else {
                cell.underline
            },
            underline_color: if is_url {
                rgb_to_hsla(Rgb {
                    r: 0x4f,
                    g: 0xaf,
                    b: 0xff,
                })
            } else {
                cell.underline_color
            },
            strikeout: cell.strikeout,
            is_url,
        }
    }

    fn has_same_style(&self, other: &Self) -> bool {
        self.force_width_cells == 1
            && other.force_width_cells == 1
            && self.fg == other.fg
            && self.bold == other.bold
            && self.italic == other.italic
            && self.underline == other.underline
            && self.underline_color == other.underline_color
            && self.strikeout == other.strikeout
            && self.is_url == other.is_url
    }
}

/// 将终端网格转换为带有明确列起点的文本 run。
///
/// 普通字符可以按样式合并，但宽字符必须单独成 run：GPUI 的固定宽度排版
/// 按 glyph 推进，而终端网格按 cell 推进。把两者混在同一个 run 中会让中文
/// 只占一列，随后字符就会覆盖它。
fn terminal_text_runs(row: &[RenderCell]) -> Vec<RenderTextRun> {
    let mut runs = Vec::with_capacity(row.len() / 8 + 1);
    let mut current: Option<RenderTextRun> = None;

    for (col, cell) in row.iter().enumerate() {
        if cell.spacer || cell.kitty_placeholder {
            continue;
        }

        let cell_run = RenderTextRun::from_cell(col, cell);
        if cell.wide {
            if let Some(run) = current.take() {
                runs.push(run);
            }
            runs.push(cell_run);
            continue;
        }

        if let Some(run) = current.as_mut()
            && run.start_col + run.cell_count == col
            && run.has_same_style(&cell_run)
        {
            run.text.push_str(&cell_run.text);
            run.cell_count += 1;
        } else {
            if let Some(run) = current.take() {
                runs.push(run);
            }
            current = Some(cell_run);
        }
    }

    if let Some(run) = current {
        runs.push(run);
    }

    runs
}

/// Returns the visual cell span occupied by the terminal cursor.
///
/// Alacritty stores a wide character in a leading cell followed by a spacer,
/// but the cursor can temporarily point at either half while an application is
/// editing the line. Keep the cursor rectangle and the glyph it repaints on
/// the same two-cell span in both cases.
fn cursor_visual_span(row: &[RenderCell], cursor_col: usize) -> (usize, usize, usize) {
    let Some(cell) = row.get(cursor_col) else {
        return (cursor_col, 1, cursor_col);
    };

    if cell.wide {
        return (cursor_col, 2, cursor_col);
    }

    if cell.spacer {
        if cursor_col > 0 && row[cursor_col - 1].wide {
            return (cursor_col - 1, 2, cursor_col - 1);
        }
        if row.get(cursor_col + 1).is_some_and(|next| next.wide) {
            return (cursor_col, 2, cursor_col + 1);
        }
    }

    (cursor_col, 1, cursor_col)
}

/// 可见视口快照 + 光标位置 + 选择区域。
struct Snapshot {
    rows: Vec<Vec<RenderCell>>,
    cursor: Option<(usize, usize)>, // (col, row_within_viewport)
    /// viewport 内 ((col,row), (col,row)) 选择起止。
    selection: Option<((usize, usize), (usize, usize))>,
    cols: usize,
    /// 当前显示偏移（滚动条用）。
    display_offset: usize,
    /// 历史总行数（滚动条用）。
    history_len: usize,
    /// 光标是否可见（闪烁控制 + DECTCEM）。
    cursor_visible: bool,
    /// DECSCUSR/OSC 50 设置的光标形状。
    cursor_shape: CursorShape,
    /// 可见区内的 URL：(row_in_viewport, col_start, col_end, url_string)。
    urls: Vec<(usize, usize, usize, String)>,
    /// 可见区每一行的时间戳；换行续行和 alternate screen 为 None。
    timestamps: Vec<Option<String>>,
    /// Kitty Unicode placeholders decoded from the visible terminal grid.
    kitty_placeholders: Vec<KittyPlaceholder>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RowSignature {
    hash: u64,
    has_content: bool,
    text: String,
    wraps_to_next: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct LogicalTimestampLine {
    text: String,
    timestamp: Option<String>,
}

/// 保存终端主屏幕的行时间戳。它和 alacritty 网格分开，避免任何 UI 元数据
/// 进入 PTY，也避免 ANSI 控制序列被误显示成终端内容。
#[derive(Default)]
struct TerminalTimestampState {
    lines: Vec<Option<String>>,
    signatures: Vec<RowSignature>,
    columns: usize,
    screen_lines: usize,
}

impl TerminalTimestampState {
    fn observe(&mut self, term: &Term<NoopListener>, timestamp: String) {
        let grid = term.grid();
        let signatures = terminal_row_signatures(term);
        let columns = grid.columns();
        let screen_lines = grid.screen_lines();
        let shape_changed =
            self.columns != 0 && (self.columns != columns || self.screen_lines != screen_lines);

        let old_signatures = std::mem::take(&mut self.signatures);
        let old_lines = std::mem::take(&mut self.lines);
        let mut next_lines = vec![None; signatures.len()];
        let mut mapping = vec![None; signatures.len()];

        if !shape_changed && !old_signatures.is_empty() {
            if signatures.len() > old_signatures.len() {
                for (new_index, old_index) in
                    mapping.iter_mut().enumerate().take(old_signatures.len())
                {
                    *old_index = Some(new_index);
                }
            } else if signatures.len() == old_signatures.len() {
                if let Some(shift) = detect_scroll_shift(&old_signatures, &signatures) {
                    for (new_index, mapped_old_index) in mapping.iter_mut().enumerate() {
                        let old_index = new_index + shift;
                        if old_index < old_signatures.len() {
                            *mapped_old_index = Some(old_index);
                        }
                    }
                } else {
                    for (new_index, old_index) in mapping.iter_mut().enumerate() {
                        *old_index = Some(new_index);
                    }
                }
            }
        }

        for (new_index, signature) in signatures.iter().enumerate() {
            let Some(old_index) = mapping[new_index] else {
                if signature.has_content {
                    next_lines[new_index] = Some(timestamp.clone());
                }
                continue;
            };

            if old_signatures.get(old_index) == Some(signature) {
                next_lines[new_index] = old_lines.get(old_index).cloned().flatten();
            } else if signature.has_content {
                next_lines[new_index] = Some(timestamp.clone());
            }
        }

        // 即使输出只有 ANSI 控制序列，当前编辑行也代表一次新的终端活动。
        // 这能让空提示符行在 gutter 中有时间，而不会给所有空白行加时间。
        let cursor_index = grid.history_size() as i32 + grid.cursor.point.line.0;
        if let Ok(cursor_index) = usize::try_from(cursor_index)
            && cursor_index < next_lines.len()
        {
            next_lines[cursor_index] = Some(timestamp);
        }

        self.lines = next_lines;
        self.signatures = signatures;
        self.columns = columns;
        self.screen_lines = screen_lines;
    }

    fn sync_to_term(&mut self, term: &Term<NoopListener>) {
        let signatures = terminal_row_signatures(term);
        let old_signatures = std::mem::take(&mut self.signatures);
        let old_lines = std::mem::take(&mut self.lines);
        let next_lines = remap_timestamps_after_resize(&old_signatures, &old_lines, &signatures);

        self.lines = next_lines;
        self.signatures = signatures;
        self.columns = term.grid().columns();
        self.screen_lines = term.grid().screen_lines();
    }

    fn visible(&self, term: &Term<NoopListener>) -> Vec<Option<String>> {
        let grid = term.grid();
        let rows = grid.screen_lines();
        if term.mode().contains(TermMode::ALT_SCREEN) {
            return vec![None; rows];
        }

        let history = grid.history_size();
        let display_offset = grid.display_offset();
        let start = history.saturating_sub(display_offset);
        (0..rows)
            .map(|row| {
                let line = Line(-(display_offset as i32) + row as i32);
                let index = start + row;
                let continuation = row > 0
                    && grid[Line(line.0 - 1)][Column(grid.columns() - 1)]
                        .flags
                        .contains(CellFlags::WRAPLINE);
                if continuation {
                    None
                } else {
                    self.lines.get(index).cloned().flatten()
                }
            })
            .collect()
    }
}

fn terminal_row_signatures(term: &Term<NoopListener>) -> Vec<RowSignature> {
    let grid = term.grid();
    let history = grid.history_size();
    let mut signatures = Vec::with_capacity(grid.total_lines());

    for line in -(history as i32)..grid.screen_lines() as i32 {
        let row = &grid[Line(line)];
        let mut hasher = DefaultHasher::new();
        let wraps_to_next = row
            .last()
            .is_some_and(|cell| cell.flags.contains(CellFlags::WRAPLINE));
        let mut text = String::new();
        for cell in row {
            cell.c.hash(&mut hasher);
            cell.flags.hash(&mut hasher);
            if let Some(zerowidth) = cell.zerowidth() {
                zerowidth.hash(&mut hasher);
            }

            if !cell
                .flags
                .intersects(CellFlags::WIDE_CHAR_SPACER | CellFlags::LEADING_WIDE_CHAR_SPACER)
            {
                text.push(if cell.c == '\0' { ' ' } else { cell.c });
                if let Some(zerowidth) = cell.zerowidth() {
                    for &character in zerowidth {
                        text.push(character);
                    }
                }
            }
        }
        if !wraps_to_next {
            while text.ends_with(' ') {
                text.pop();
            }
        }
        let has_content = text.chars().any(|character| character != ' ');
        signatures.push(RowSignature {
            hash: hasher.finish(),
            has_content,
            text,
            wraps_to_next,
        });
    }

    signatures
}

fn logical_timestamp_lines(
    signatures: &[RowSignature],
    timestamps: &[Option<String>],
) -> Vec<LogicalTimestampLine> {
    let mut logical_lines = Vec::new();
    let mut text = String::new();
    let mut timestamp = None;

    for (index, signature) in signatures.iter().enumerate() {
        if timestamp.is_none() {
            timestamp = timestamps.get(index).cloned().flatten();
        }
        text.push_str(&signature.text);

        if !signature.wraps_to_next {
            logical_lines.push(LogicalTimestampLine {
                text: std::mem::take(&mut text),
                timestamp: timestamp.take(),
            });
        }
    }

    if !text.is_empty()
        || timestamp.is_some()
        || signatures
            .last()
            .is_some_and(|signature| signature.wraps_to_next)
    {
        logical_lines.push(LogicalTimestampLine { text, timestamp });
    }

    logical_lines
}

fn remap_timestamps_after_resize(
    old_signatures: &[RowSignature],
    old_timestamps: &[Option<String>],
    new_signatures: &[RowSignature],
) -> Vec<Option<String>> {
    let old_logical_lines = logical_timestamp_lines(old_signatures, old_timestamps);
    let new_logical_lines = logical_timestamp_lines(new_signatures, &[]);
    let mut logical_timestamps = vec![None; new_logical_lines.len()];
    let mut old_start = 0;

    for (new_index, new_line) in new_logical_lines.iter().enumerate() {
        let Some(relative_old_index) = old_logical_lines[old_start..]
            .iter()
            .position(|old_line| old_line.text == new_line.text)
        else {
            continue;
        };
        let old_index = old_start + relative_old_index;
        logical_timestamps[new_index] = old_logical_lines[old_index].timestamp.clone();
        old_start = old_index + 1;
    }

    let mut timestamps = vec![None; new_signatures.len()];
    let mut logical_index = 0;
    for (row_index, signature) in new_signatures.iter().enumerate() {
        if let Some(timestamp) = logical_timestamps.get(logical_index) {
            timestamps[row_index] = timestamp.clone();
        }
        if !signature.wraps_to_next {
            logical_index += 1;
        }
    }

    timestamps
}

fn detect_scroll_shift(old: &[RowSignature], new: &[RowSignature]) -> Option<usize> {
    if old.len() != new.len() || old.len() < 4 {
        return None;
    }

    for shift in 1..=old.len().saturating_sub(1).min(8) {
        let overlap = old.len() - shift;
        let matches = old[shift..]
            .iter()
            .zip(&new[..overlap])
            .filter(|(old, new)| old == new)
            .count();
        let informative_matches = old[shift..]
            .iter()
            .zip(&new[..overlap])
            .filter(|(old, new)| old == new && old.has_content)
            .count();
        if matches >= overlap.saturating_sub(1) && informative_matches > 0 {
            return Some(shift);
        }
    }

    None
}

fn format_timestamp(timestamp: chrono::DateTime<Local>) -> String {
    timestamp.format("%H:%M:%S%.3f").to_string()
}

fn terminal_bounds_for(canvas_bounds: Bounds<Pixels>, show_timestamps: bool) -> Bounds<Pixels> {
    let gutter_width = if show_timestamps {
        TIMESTAMP_GUTTER_WIDTH
            .min((canvas_bounds.size.width.as_f32() - TIMESTAMP_GUTTER_GAP - 1.0).max(0.0))
    } else {
        0.0
    };
    let gap = if show_timestamps {
        TIMESTAMP_GUTTER_GAP
    } else {
        0.0
    };
    Bounds {
        origin: Point::new(
            canvas_bounds.origin.x + px(gutter_width + gap),
            canvas_bounds.origin.y,
        ),
        size: gpui::size(
            px((canvas_bounds.size.width.as_f32() - gutter_width - gap).max(1.0)),
            canvas_bounds.size.height,
        ),
    }
}

/// 把 alacritty 的绝对行号转换成当前 viewport 内的行列。
///
/// 终端滚动时光标仍然保留在 grid 的绝对位置，候选框必须使用同一套
/// display_offset 换算，否则会落在错误位置，或在不可见时错误地回退到左下角。
fn cursor_viewport_position(
    cursor_line: i32,
    cursor_column: usize,
    display_offset: usize,
    rows: usize,
    cols: usize,
) -> Option<(usize, usize)> {
    if rows == 0 || cols == 0 {
        return None;
    }

    let display_offset = i32::try_from(display_offset).unwrap_or(i32::MAX);
    let viewport_row = cursor_line.saturating_add(display_offset);
    if viewport_row < 0 || viewport_row >= rows as i32 {
        return None;
    }

    Some((cursor_column.min(cols - 1), viewport_row as usize))
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

const URL_PREFIXES: [&str; 3] = ["https://", "http://", "www."];

fn next_url_start(chars: &[char], from: usize) -> Option<(usize, usize)> {
    for index in from..chars.len() {
        for prefix in URL_PREFIXES {
            let prefix_len = prefix.len();
            if index + prefix_len <= chars.len()
                && chars[index..]
                    .iter()
                    .take(prefix_len)
                    .copied()
                    .eq(prefix.chars())
            {
                return Some((index, prefix_len));
            }
        }
    }
    None
}

fn is_url_delimiter(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '"' | '\'' | '>' | '<' | ')' | ']')
}

fn is_trailing_url_punctuation(ch: char) -> bool {
    matches!(ch, '.' | ',' | ';' | ':' | '!' | '?')
}

/// Find plain-text URLs using logical characters, then translate their ranges
/// back to terminal columns. A terminal cell is not a UTF-8 byte: wide cells
/// and non-ASCII text make those coordinate systems diverge.
fn detect_plain_urls(
    row: &mut [RenderCell],
    row_index: usize,
) -> Vec<(usize, usize, usize, String)> {
    let display_chars: Vec<(usize, char)> = row
        .iter()
        .enumerate()
        .filter(|(_, cell)| !cell.spacer && !cell.kitty_placeholder)
        .map(|(col, cell)| (col, cell.ch))
        .collect();
    let chars: Vec<char> = display_chars.iter().map(|(_, ch)| *ch).collect();
    let mut urls = Vec::new();
    let mut position = 0;

    while let Some((url_start, prefix_len)) = next_url_start(&chars, position) {
        let mut url_end = url_start + prefix_len;
        while url_end < chars.len() && !is_url_delimiter(chars[url_end]) {
            url_end += 1;
        }
        while url_end > url_start + prefix_len && is_trailing_url_punctuation(chars[url_end - 1]) {
            url_end -= 1;
        }

        if url_end > url_start + prefix_len {
            let start_col = display_chars[url_start].0;
            let end_cell_col = display_chars[url_end - 1].0;
            let end_col = end_cell_col + if row[end_cell_col].wide { 2 } else { 1 };
            let url = chars[url_start..url_end].iter().collect();

            for cell in row.iter_mut().take(end_col).skip(start_col) {
                cell.is_url = true;
            }
            urls.push((row_index, start_col, end_col, url));
        }

        position = url_end.max(url_start + prefix_len);
    }

    urls
}

/// Kitty reserves this stable list of combining marks for placeholder row and
/// column numbers. The first entries cover the common compact grid sizes;
/// keeping the mapping explicit avoids treating an unrelated combining mark
/// as image metadata.
const KITTY_PLACEHOLDER_DIACRITICS: &[char] = &[
    '\u{0305}',
    '\u{030d}',
    '\u{030e}',
    '\u{0310}',
    '\u{0312}',
    '\u{033d}',
    '\u{033e}',
    '\u{033f}',
    '\u{0346}',
    '\u{034a}',
    '\u{034b}',
    '\u{034c}',
    '\u{0350}',
    '\u{0351}',
    '\u{0352}',
    '\u{0357}',
    '\u{035b}',
    '\u{0363}',
    '\u{0364}',
    '\u{0365}',
    '\u{0366}',
    '\u{0367}',
    '\u{0368}',
    '\u{0369}',
    '\u{036a}',
    '\u{036b}',
    '\u{036c}',
    '\u{036d}',
    '\u{036e}',
    '\u{036f}',
    '\u{0483}',
    '\u{0484}',
    '\u{0485}',
    '\u{0486}',
    '\u{0487}',
    '\u{0592}',
    '\u{0593}',
    '\u{0594}',
    '\u{0595}',
    '\u{0597}',
    '\u{0598}',
    '\u{0599}',
    '\u{059c}',
    '\u{059d}',
    '\u{059e}',
    '\u{059f}',
    '\u{05a0}',
    '\u{05a1}',
    '\u{05a8}',
    '\u{05a9}',
    '\u{05ab}',
    '\u{05ac}',
    '\u{05af}',
    '\u{05c4}',
    '\u{0610}',
    '\u{0611}',
    '\u{0612}',
    '\u{0613}',
    '\u{0614}',
    '\u{0615}',
    '\u{0616}',
    '\u{0617}',
    '\u{0657}',
    '\u{0658}',
    '\u{0659}',
    '\u{065a}',
    '\u{065b}',
    '\u{065d}',
    '\u{065e}',
    '\u{06d6}',
    '\u{06d7}',
    '\u{06d8}',
    '\u{06d9}',
    '\u{06da}',
    '\u{06db}',
    '\u{06dc}',
    '\u{06df}',
    '\u{06e0}',
    '\u{06e1}',
    '\u{06e2}',
    '\u{06e4}',
    '\u{06e7}',
    '\u{06e8}',
    '\u{06eb}',
    '\u{06ec}',
    '\u{0730}',
    '\u{0732}',
    '\u{0733}',
    '\u{0735}',
    '\u{0736}',
    '\u{073a}',
    '\u{073d}',
    '\u{073f}',
    '\u{0740}',
    '\u{0741}',
    '\u{0743}',
    '\u{0745}',
    '\u{0747}',
    '\u{0749}',
    '\u{074a}',
    '\u{07eb}',
    '\u{07ec}',
    '\u{07ed}',
    '\u{07ee}',
    '\u{07ef}',
    '\u{07f0}',
    '\u{07f1}',
    '\u{07f3}',
    '\u{0816}',
    '\u{0817}',
    '\u{0818}',
    '\u{0819}',
    '\u{081b}',
    '\u{081c}',
    '\u{081d}',
    '\u{081e}',
    '\u{081f}',
    '\u{0820}',
    '\u{0821}',
    '\u{0822}',
    '\u{0823}',
    '\u{0825}',
    '\u{0826}',
    '\u{0827}',
    '\u{0829}',
    '\u{082a}',
    '\u{082b}',
    '\u{082c}',
    '\u{082d}',
    '\u{0951}',
    '\u{0953}',
    '\u{0954}',
    '\u{0f82}',
    '\u{0f83}',
    '\u{0f86}',
    '\u{0f87}',
    '\u{135d}',
    '\u{135e}',
    '\u{135f}',
    '\u{17dd}',
    '\u{193a}',
    '\u{1a17}',
    '\u{1a75}',
    '\u{1a76}',
    '\u{1a77}',
    '\u{1a78}',
    '\u{1a79}',
    '\u{1a7a}',
    '\u{1a7b}',
    '\u{1a7c}',
    '\u{1b6b}',
    '\u{1b6d}',
    '\u{1b6e}',
    '\u{1b6f}',
    '\u{1b70}',
    '\u{1b71}',
    '\u{1b72}',
    '\u{1b73}',
    '\u{1cd0}',
    '\u{1cd1}',
    '\u{1cd2}',
    '\u{1cda}',
    '\u{1cdb}',
    '\u{1ce0}',
    '\u{1dc0}',
    '\u{1dc1}',
    '\u{1dc3}',
    '\u{1dc4}',
    '\u{1dc5}',
    '\u{1dc6}',
    '\u{1dc7}',
    '\u{1dc8}',
    '\u{1dc9}',
    '\u{1dcb}',
    '\u{1dcc}',
    '\u{1dd1}',
    '\u{1dd2}',
    '\u{1dd3}',
    '\u{1dd4}',
    '\u{1dd5}',
    '\u{1dd6}',
    '\u{1dd7}',
    '\u{1dd8}',
    '\u{1dd9}',
    '\u{1dda}',
    '\u{1ddb}',
    '\u{1ddc}',
    '\u{1ddd}',
    '\u{1dde}',
    '\u{1ddf}',
    '\u{1de0}',
    '\u{1de1}',
    '\u{1de2}',
    '\u{1de3}',
    '\u{1de4}',
    '\u{1de5}',
    '\u{1de6}',
    '\u{1dfe}',
    '\u{20d0}',
    '\u{20d1}',
    '\u{20d4}',
    '\u{20d5}',
    '\u{20d6}',
    '\u{20d7}',
    '\u{20db}',
    '\u{20dc}',
    '\u{20e1}',
    '\u{20e7}',
    '\u{20e9}',
    '\u{20f0}',
    '\u{2cef}',
    '\u{2cf0}',
    '\u{2cf1}',
    '\u{2de0}',
    '\u{2de1}',
    '\u{2de2}',
    '\u{2de3}',
    '\u{2de4}',
    '\u{2de5}',
    '\u{2de6}',
    '\u{2de7}',
    '\u{2de8}',
    '\u{2de9}',
    '\u{2dea}',
    '\u{2deb}',
    '\u{2dec}',
    '\u{2ded}',
    '\u{2dee}',
    '\u{2def}',
    '\u{2df0}',
    '\u{2df1}',
    '\u{2df2}',
    '\u{2df3}',
    '\u{2df4}',
    '\u{2df5}',
    '\u{2df6}',
    '\u{2df7}',
    '\u{2df8}',
    '\u{2df9}',
    '\u{2dfa}',
    '\u{2dfb}',
    '\u{2dfc}',
    '\u{2dfd}',
    '\u{2dfe}',
    '\u{2dff}',
    '\u{a66f}',
    '\u{a67c}',
    '\u{a67d}',
    '\u{a6f0}',
    '\u{a6f1}',
    '\u{a8e0}',
    '\u{a8e1}',
    '\u{a8e2}',
    '\u{a8e3}',
    '\u{a8e4}',
    '\u{a8e5}',
    '\u{a8e6}',
    '\u{a8e7}',
    '\u{a8e8}',
    '\u{a8e9}',
    '\u{a8ea}',
    '\u{a8eb}',
    '\u{a8ec}',
    '\u{a8ed}',
    '\u{a8ee}',
    '\u{a8ef}',
    '\u{a8f0}',
    '\u{a8f1}',
    '\u{aab0}',
    '\u{aab2}',
    '\u{aab3}',
    '\u{aab7}',
    '\u{aab8}',
    '\u{aabe}',
    '\u{aabf}',
    '\u{aac1}',
    '\u{fe20}',
    '\u{fe21}',
    '\u{fe22}',
    '\u{fe23}',
    '\u{fe24}',
    '\u{fe25}',
    '\u{fe26}',
    '\u{10a0f}',
    '\u{10a38}',
    '\u{1d185}',
    '\u{1d186}',
    '\u{1d187}',
    '\u{1d188}',
    '\u{1d189}',
    '\u{1d1aa}',
    '\u{1d1ab}',
    '\u{1d1ac}',
    '\u{1d1ad}',
    '\u{1d242}',
    '\u{1d243}',
    '\u{1d244}',
];

fn kitty_placeholder_diacritic_value(character: char) -> Option<usize> {
    KITTY_PLACEHOLDER_DIACRITICS
        .iter()
        .position(|candidate| *candidate == character)
}

fn kitty_placeholder_color_value(color: &Color) -> Option<u32> {
    match color {
        Color::Indexed(value) => Some(*value as u32),
        Color::Spec(Rgb { r, g, b }) => Some((*r as u32) << 16 | (*g as u32) << 8 | *b as u32),
        Color::Named(_) => None,
    }
}

fn decode_kitty_placeholder(
    cell: &Cell,
    zero_width: &str,
    viewport_row: usize,
    viewport_column: usize,
    previous: Option<KittyPlaceholderState>,
) -> Option<(KittyPlaceholder, KittyPlaceholderState)> {
    if cell.c != KITTY_PLACEHOLDER_CHAR {
        return None;
    }
    let foreground = kitty_placeholder_color_value(&cell.fg)?;
    let underline = cell
        .underline_color()
        .and_then(|color| kitty_placeholder_color_value(&color));
    let marks = zero_width
        .chars()
        .filter_map(kitty_placeholder_diacritic_value)
        .take(3)
        .collect::<Vec<_>>();
    let same_colors = previous.is_some_and(|previous| {
        previous.foreground == foreground && previous.underline == underline
    });
    let previous = previous
        .filter(|previous| previous.foreground == foreground && previous.underline == underline);

    let (row, column, image_id_high) = match marks.as_slice() {
        [] => {
            let previous = previous?;
            (
                previous.row,
                previous.column.checked_add(1)?,
                previous.image_id_high,
            )
        }
        [row] => {
            let column = if same_colors && previous.is_some_and(|previous| previous.row == *row) {
                previous?.column.checked_add(1)?
            } else {
                0
            };
            let image_id_high = previous
                .filter(|previous| previous.row == *row)
                .map(|previous| previous.image_id_high)
                .unwrap_or(0);
            (*row, column, image_id_high)
        }
        [row, column] => {
            let image_id_high = previous
                .filter(|previous| {
                    previous.row == *row && previous.column.checked_add(1) == Some(*column)
                })
                .map(|previous| previous.image_id_high)
                .unwrap_or(0);
            (*row, *column, image_id_high)
        }
        [row, column, image_id_high] => (*row, *column, u8::try_from(*image_id_high).ok()?),
        _ => unreachable!(),
    };
    let image_id = (foreground & 0x00ff_ffff) | (u32::from(image_id_high) << 24);
    let state = KittyPlaceholderState {
        foreground,
        underline,
        row,
        column,
        image_id_high,
    };
    Some((
        KittyPlaceholder {
            image_id,
            placement_id: underline.filter(|value| *value != 0),
            row,
            column,
            viewport_row,
            viewport_column,
        },
        state,
    ))
}

/// 把 Term 可见区快照成 owned 数据。
fn snapshot_visible(
    term: &Term<NoopListener>,
    selection: Option<((usize, usize), (usize, usize))>,
    _cols: usize,
    cursor_visible: bool,
    timestamps: &[Option<String>],
) -> Snapshot {
    let grid = term.grid();
    let display_offset = grid.display_offset();
    let cols = term.columns();
    let rows = term.screen_lines();
    let top_visible = Line(-(display_offset as i32));
    let colors = term.colors();
    let default_fg = fg_of(term);
    let default_bg = bg_of(term);
    let cursor_shape = term.cursor_style().shape;

    log::trace!(
        "snapshot_visible: display_offset={} top_visible={} cols={} rows={} total_lines={}",
        display_offset,
        top_visible.0,
        cols,
        rows,
        grid.total_lines()
    );

    let mut out_rows: Vec<Vec<RenderCell>> = Vec::with_capacity(rows);
    let mut kitty_placeholders = Vec::new();
    for r in 0..rows {
        let line = Line(top_visible.0 + r as i32);
        let row = &grid[line];
        let mut out: Vec<RenderCell> = Vec::with_capacity(cols);
        let mut previous_placeholder = None;
        for c in 0..cols {
            let cell: &Cell = &row[Column(c)];
            let style = effective_cell_style(cell, colors, default_fg, default_bg);
            let mut zero_width = String::new();
            if let Some(chars) = cell.zerowidth() {
                zero_width.extend(chars.iter().copied());
            }
            let kitty_placeholder =
                decode_kitty_placeholder(cell, &zero_width, r, c, previous_placeholder)
                    .map(|(placeholder, state)| {
                        kitty_placeholders.push(placeholder);
                        previous_placeholder = Some(state);
                        true
                    })
                    .unwrap_or_else(|| {
                        previous_placeholder = None;
                        false
                    });
            out.push(RenderCell {
                ch: if cell.c == '\0' { ' ' } else { cell.c },
                fg: style.fg,
                bg: style.bg,
                bold: style.bold,
                italic: style.italic,
                underline: style.underline,
                underline_color: style.underline_color,
                strikeout: style.strikeout,
                spacer: cell
                    .flags
                    .intersects(CellFlags::WIDE_CHAR_SPACER | CellFlags::LEADING_WIDE_CHAR_SPACER),
                wide: cell.flags.contains(CellFlags::WIDE_CHAR),
                zero_width,
                kitty_placeholder,
                is_url: false,
                hyperlink: cell.hyperlink().map(|link| link.uri().to_string()),
            });
        }
        out_rows.push(out);
    }

    // 光标位置（视口内）。
    let cursor = cursor_viewport_position(
        grid.cursor.point.line.0,
        grid.cursor.point.column.0,
        display_offset,
        rows,
        cols,
    );

    let display_offset = grid.display_offset();
    let history_len = grid.history_size();
    // URL 检测与标记。
    let mut urls: Vec<(usize, usize, usize, String)> = Vec::new();
    for vy in 0..rows.min(out_rows.len()) {
        // OSC 8 hyperlinks are authoritative: preserve their target even
        // when the visible label does not contain a URL.
        let mut col = 0;
        while col < out_rows[vy].len() {
            let Some(url) = out_rows[vy][col].hyperlink.clone() else {
                col += 1;
                continue;
            };
            let start = col;
            while col < out_rows[vy].len()
                && out_rows[vy][col].hyperlink.as_deref() == Some(url.as_str())
            {
                out_rows[vy][col].is_url = true;
                col += 1;
            }
            urls.push((vy, start, col, url));
        }

        urls.extend(detect_plain_urls(&mut out_rows[vy], vy));
    }

    Snapshot {
        rows: out_rows,
        cursor,
        selection,
        cols,
        display_offset,
        history_len,
        cursor_visible,
        cursor_shape,
        urls,
        timestamps: timestamps.to_vec(),
        kitty_placeholders,
    }
}

/// paint 阶段共享的绘制参数（收敛 paint_* 长参数列表）。
struct PaintContext<'a> {
    snapshot: &'a Snapshot,
    ime_marked_text: &'a str,
    canvas_bounds: Bounds<Pixels>,
    bounds: Bounds<Pixels>,
    cell_w: Pixels,
    line_h: Pixels,
    font_size: f32,
    show_timestamps: bool,
    font: &'a Font,
    default_fg: Hsla,
    default_bg: Hsla,
    images: &'a [TerminalImage],
    progress: Option<TerminalProgress>,
}

/// 根据快照绘制。
fn paint_timestamp_gutter(ctx: &PaintContext, window: &mut Window, cx: &mut App) {
    let snapshot = ctx.snapshot;
    let canvas_bounds = ctx.canvas_bounds;
    let terminal_bounds = ctx.bounds;
    let line_h = ctx.line_h;
    let font_size = ctx.font_size;
    let font = ctx.font;
    let default_fg = ctx.default_fg;
    let default_bg = ctx.default_bg;
    window.paint_quad(quad(
        canvas_bounds,
        Corners::default(),
        default_bg,
        Edges::default(),
        hsla(0., 0., 0., 0.),
        gpui::BorderStyle::default(),
    ));

    let reserved_width = terminal_bounds.origin.x - canvas_bounds.origin.x;
    let gutter_width = reserved_width - px(TIMESTAMP_GUTTER_GAP);
    if gutter_width.as_f32() <= 1.0 {
        return;
    }

    let divider_color = Hsla::from(theme::border());
    window.paint_quad(quad(
        Bounds {
            origin: Point::new(
                canvas_bounds.origin.x + gutter_width - px(1.),
                canvas_bounds.origin.y,
            ),
            size: gpui::size(px(1.), canvas_bounds.size.height),
        },
        Corners::default(),
        divider_color,
        Edges::default(),
        hsla(0., 0., 0., 0.),
        gpui::BorderStyle::default(),
    ));

    let text_width = gutter_width.as_f32() - TIMESTAMP_GUTTER_PADDING;
    if text_width <= 0.0 {
        return;
    }
    let timestamp_color = Hsla {
        a: default_fg.a * 0.48,
        ..default_fg
    };

    for (row, timestamp) in snapshot.timestamps.iter().enumerate() {
        let Some(timestamp) = timestamp else {
            continue;
        };
        let text_run = TextRun {
            len: timestamp.len(),
            font: font.clone(),
            color: timestamp_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = window.text_system().shape_line(
            SharedString::from(timestamp.clone()),
            px((font_size - 2.0).max(8.0)),
            &[text_run],
            None,
        );
        let origin = Point::new(
            canvas_bounds.origin.x + px(TIMESTAMP_GUTTER_PADDING / 2.0),
            canvas_bounds.origin.y + px(row as f32 * line_h.as_f32()),
        );
        if let Err(error) = shaped.paint(
            origin,
            line_h,
            TextAlign::Right,
            Some(px(text_width)),
            window,
            cx,
        ) {
            log::warn!("paint timestamp row {row} failed: {error}");
        }
    }
}

fn paint_cell_backgrounds(
    snapshot: &Snapshot,
    bounds: Bounds<Pixels>,
    cell_w: f32,
    line_h: Pixels,
    default_bg: Hsla,
    window: &mut Window,
) {
    for (row_index, row) in snapshot.rows.iter().enumerate() {
        let mut start = 0usize;
        while start < row.len() {
            let color = row[start].bg;
            let mut end = start + 1;
            while end < row.len() && row[end].bg == color {
                end += 1;
            }

            if color != default_bg {
                window.paint_quad(quad(
                    Bounds {
                        origin: Point::new(
                            bounds.origin.x + px(start as f32 * cell_w),
                            bounds.origin.y + px(row_index as f32 * line_h.as_f32()),
                        ),
                        size: gpui::size(px((end - start) as f32 * cell_w), line_h),
                    },
                    Corners::default(),
                    color,
                    Edges::default(),
                    hsla(0., 0., 0., 0.),
                    gpui::BorderStyle::default(),
                ));
            }
            start = end;
        }
    }
}

fn paint_terminal_images(
    ctx: &PaintContext,
    window: &mut Window,
    cx: &mut App,
    under_text: bool,
    under_cell_background: bool,
) {
    if ctx.images.is_empty() {
        return;
    }
    let snapshot = ctx.snapshot;
    let top_line = snapshot.history_len.saturating_sub(snapshot.display_offset) as i64;
    let cell_w = ctx.cell_w.as_f32();
    let line_h = ctx.line_h.as_f32();
    let mut images = ctx
        .images
        .iter()
        .filter(|image| (image.z_index < 0) == under_text)
        .filter(|image| (image.z_index < KITTY_BACKGROUND_Z_INDEX) == under_cell_background)
        .collect::<Vec<_>>();
    images.sort_by_key(|image| {
        (
            image.z_index,
            image.kitty_id.unwrap_or(u32::MAX),
            image.placement_id.unwrap_or(u32::MAX),
        )
    });

    for image in images {
        let Some(render_image) = image.image.clone().get_render_image(window, cx) else {
            continue;
        };
        let natural = render_image.size(0);
        let natural_width = natural.width.0.max(1) as f32;
        let natural_height = natural.height.0.max(1) as f32;
        let mut width = image
            .width
            .map(|dimension| image_dimension_pixels(dimension, cell_w))
            .unwrap_or(natural_width);
        let mut height = image
            .height
            .map(|dimension| image_dimension_pixels(dimension, line_h))
            .unwrap_or(natural_height);

        if image.preserve_aspect_ratio {
            match (image.width, image.height) {
                (Some(_), None) => height = natural_height * width / natural_width,
                (None, Some(_)) => width = natural_width * height / natural_height,
                _ => {}
            }
        }
        if width <= 0.0 || height <= 0.0 {
            continue;
        }

        let origins = terminal_image_origins(image, ctx.images, snapshot, top_line, 0);
        for (row, column) in origins {
            let offset_x = if image.virtual_placement {
                0.
            } else {
                image.offset_x.min(cell_w.max(0.) as usize) as f32
            };
            let offset_y = if image.virtual_placement {
                0.
            } else {
                image.offset_y.min(line_h.max(0.) as usize) as f32
            };
            let image_bounds = Bounds {
                origin: Point::new(
                    ctx.bounds.origin.x + px(column as f32 * cell_w) + px(offset_x),
                    ctx.bounds.origin.y + px(row as f32 * line_h) + px(offset_y),
                ),
                size: gpui::size(px(width), px(height)),
            };
            if let Err(error) = window.paint_image(
                ctx.bounds,
                image_bounds,
                Corners::default(),
                render_image.clone(),
                0,
                false,
            ) {
                log::debug!("paint terminal image failed: {error}");
            }
        }
    }
}

fn paint_terminal_progress(ctx: &PaintContext, window: &mut Window) {
    let Some(progress) = ctx.progress else {
        return;
    };
    let height = px(2.0);
    let y = ctx.bounds.bottom() - height;
    let track = hsla(0.0, 0.0, 0.0, 0.32);
    let fill = match progress.state {
        2 => Hsla::from(theme::danger()),
        4 => Hsla::from(theme::warning()),
        _ => Hsla::from(theme::accent()),
    };
    let fraction = match progress.state {
        3 => 0.35,
        _ => progress.progress.unwrap_or(0) as f32 / 100.0,
    };
    window.paint_quad(quad(
        Bounds {
            origin: Point::new(ctx.bounds.origin.x, y),
            size: gpui::size(ctx.bounds.size.width, height),
        },
        Corners::default(),
        track,
        Edges::default(),
        hsla(0.0, 0.0, 0.0, 0.0),
        gpui::BorderStyle::default(),
    ));
    if fraction > 0.0 {
        window.paint_quad(quad(
            Bounds {
                origin: Point::new(ctx.bounds.origin.x, y),
                size: gpui::size(ctx.bounds.size.width * fraction.min(1.0), height),
            },
            Corners::default(),
            fill,
            Edges::default(),
            hsla(0.0, 0.0, 0.0, 0.0),
            gpui::BorderStyle::default(),
        ));
    }
}

fn image_dimension_pixels(dimension: ImageDimension, cell_size: f32) -> f32 {
    match dimension {
        ImageDimension::Cells(cells) => cells as f32 * cell_size,
        ImageDimension::Pixels(pixels) => pixels as f32,
    }
}

fn image_dimension_cells(dimension: Option<ImageDimension>) -> Option<usize> {
    match dimension {
        Some(ImageDimension::Cells(cells)) => Some(cells),
        Some(ImageDimension::Pixels(_)) | None => None,
    }
}

fn terminal_image_origins(
    image: &TerminalImage,
    images: &[TerminalImage],
    snapshot: &Snapshot,
    top_line: i64,
    depth: usize,
) -> Vec<(i64, i64)> {
    if depth >= 8 {
        return Vec::new();
    }
    if let Some(parent_id) = image.relative_image_id {
        let Some(parent) = images.iter().rev().find(|parent| {
            parent.kitty_id == Some(parent_id)
                && image
                    .relative_placement_id
                    .is_none_or(|placement| parent.placement_id == Some(placement))
        }) else {
            return Vec::new();
        };
        return terminal_image_origins(parent, images, snapshot, top_line, depth + 1)
            .into_iter()
            .map(|(row, column)| {
                (
                    row.saturating_add(i64::from(image.relative_offset_y)),
                    column.saturating_add(i64::from(image.relative_offset_x)),
                )
            })
            .collect();
    }

    if image.virtual_placement {
        let Some(image_id) = image.kitty_id else {
            return Vec::new();
        };
        let mut origins = Vec::new();
        for placeholder in snapshot
            .kitty_placeholders
            .iter()
            .filter(|placeholder| placeholder.image_id == image_id)
            .filter(|placeholder| {
                image.placement_id.is_none() || image.placement_id == placeholder.placement_id
            })
        {
            let origin = (
                placeholder.viewport_row as i64 - placeholder.row as i64,
                placeholder.viewport_column as i64 - placeholder.column as i64,
            );
            if !origins.contains(&origin) {
                origins.push(origin);
            }
        }
        origins
    } else {
        vec![(image.origin_line - top_line, image.origin_col as i64)]
    }
}

fn paint_snapshot(ctx: &PaintContext, window: &mut Window, cx: &mut App) {
    let snapshot = ctx.snapshot;
    let ime_marked_text = ctx.ime_marked_text;
    let bounds = ctx.bounds;
    let cell_w = ctx.cell_w;
    let line_h = ctx.line_h;
    let font_size = ctx.font_size;
    let show_timestamps = ctx.show_timestamps;
    let font = ctx.font;
    let default_fg = ctx.default_fg;
    let default_bg = ctx.default_bg;
    let cell_wf = cell_w.as_f32();
    let line_hf = line_h.as_f32();

    if show_timestamps {
        paint_timestamp_gutter(ctx, window, cx);
    }

    // 诊断：打印每帧实际拿到的 snapshot 内容（非空行），用于判断
    // 是「读不到内容」还是「画不出来」。trace 级别，避免 debug 下日志爆炸。
    if log::log_enabled!(log::Level::Trace) {
        log::trace!(
            "paint_snapshot: {} rows, bounds={}x{} cell_w={} line_h={}",
            snapshot.rows.len(),
            bounds.size.width.as_f32() as u32,
            bounds.size.height.as_f32() as u32,
            cell_wf,
            line_hf
        );
        for (r, row) in snapshot.rows.iter().enumerate() {
            let s: String = row
                .iter()
                .map(|c| {
                    if c.spacer || c.kitty_placeholder {
                        ' '
                    } else {
                        c.ch
                    }
                })
                .collect();
            let t = s.trim_end();
            if !t.is_empty() {
                log::trace!("  snapshot row {:2}: {:?}", r, t);
            }
        }
    }

    // 背景填充整个视口。
    window.paint_quad(quad(
        bounds,
        Corners::default(),
        default_bg,
        Edges::default(),
        hsla(0., 0., 0., 0.),
        gpui::BorderStyle::default(),
    ));

    // Kitty negative z-index placements sit below terminal text. They are
    // painted after the base fill but before non-default cell backgrounds.
    paint_terminal_images(ctx, window, cx, true, true);

    // Paint cell backgrounds separately from glyphs. GPUI text backgrounds are
    // glyph-run decorations and do not reliably cover blank cells or preserve
    // the terminal selection layer.
    paint_cell_backgrounds(snapshot, bounds, cell_wf, line_h, default_bg, window);

    // Ordinary negative z-index images sit above non-default cell backgrounds
    // but below text. Kitty reserves lower-than-INT32_MIN/2 z values for
    // images that should also be covered by cell backgrounds.
    paint_terminal_images(ctx, window, cx, true, false);

    // 选择高亮使用主题的 mint/teal 色，提高深色终端中的可见度。
    let sel_bg = hsla(0.43, 0.58, 0.42, 0.78);

    for (r, row) in snapshot.rows.iter().enumerate() {
        // 绘制选择高亮背景。
        if let Some(((ax, ay), (bx, by))) = snapshot.selection {
            let r0 = ay.min(by);
            let r1 = ay.max(by);
            if r >= r0 && r <= r1 {
                let cols = snapshot.cols;
                let (c0, c1) = if r == r0 && r == r1 {
                    (ax.min(bx), ax.max(bx))
                } else if r == r0 {
                    (ax.min(bx), cols.saturating_sub(1))
                } else if r == r1 {
                    (0, ax.max(bx))
                } else {
                    (0, cols.saturating_sub(1))
                };
                if c0 <= c1 {
                    let x = bounds.origin.x + px(c0 as f32 * cell_wf);
                    let w = px((c1 - c0 + 1) as f32 * cell_wf);
                    let y = bounds.origin.y + px(r as f32 * line_hf);
                    window.paint_quad(quad(
                        Bounds {
                            origin: Point::new(x, y),
                            size: gpui::size(w, line_h),
                        },
                        Corners::default(),
                        sel_bg,
                        Edges::default(),
                        hsla(0., 0., 0., 0.),
                        gpui::BorderStyle::default(),
                    ));
                }
            }
        }

        let row_y = bounds.origin.y + px(r as f32 * line_hf);
        for run in terminal_text_runs(row) {
            let underline = match run.underline {
                UnderlineKind::None if !run.is_url => None,
                UnderlineKind::Wavy => Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(run.underline_color),
                    wavy: true,
                }),
                UnderlineKind::Solid | UnderlineKind::None => Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(run.underline_color),
                    wavy: false,
                }),
            };
            let strikethrough = run.strikeout.then(|| StrikethroughStyle {
                thickness: px(1.0),
                color: Some(run.fg),
            });
            let text_len = run.text.len();
            let text_run = TextRun {
                len: text_len,
                font: Font {
                    weight: if run.bold {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    },
                    style: if run.italic {
                        gpui::FontStyle::Italic
                    } else {
                        gpui::FontStyle::Normal
                    },
                    family: font.family.clone(),
                    features: font.features.clone(),
                    fallbacks: font.fallbacks.clone(),
                },
                color: run.fg,
                background_color: None,
                underline,
                strikethrough,
            };
            let shaped = window.text_system().shape_line(
                SharedString::from(run.text),
                px(font_size),
                &[text_run],
                Some(px(cell_wf * run.force_width_cells as f32)),
            );
            let origin = Point::new(bounds.origin.x + px(run.start_col as f32 * cell_wf), row_y);
            if let Err(e) = shaped.paint(origin, line_h, TextAlign::Left, None, window, cx) {
                log::warn!("paint row {r} failed: {e}");
            }
        }
    }

    paint_terminal_images(ctx, window, cx, false, false);
    paint_terminal_progress(ctx, window);

    // 光标形状由 DECSCUSR/OSC 50 控制；blink 已在快照阶段按终端状态处理。
    if snapshot.cursor_visible
        && ime_marked_text.is_empty()
        && let Some((col, row)) = snapshot.cursor
        && snapshot
            .rows
            .get(row)
            .and_then(|cells| cells.get(col))
            .is_some()
        && snapshot.cursor_shape != CursorShape::Hidden
    {
        let row_cells = &snapshot.rows[row];
        let (cursor_col, cursor_cell_count, glyph_col) = cursor_visual_span(row_cells, col);
        let cursor_cell = &row_cells[glyph_col];
        let cursor_width = px(cursor_cell_count as f32 * cell_wf);
        let x = bounds.origin.x + px(cursor_col as f32 * cell_wf);
        let y = bounds.origin.y + px(row as f32 * line_hf);
        let (cursor_origin, cursor_size, cursor_fill, cursor_edges) = match snapshot.cursor_shape {
            CursorShape::Beam => (
                Point::new(x, y),
                gpui::size(px(cell_wf.clamp(1.0, 2.0)), line_h),
                cursor_cell.fg,
                Edges::default(),
            ),
            CursorShape::Underline => {
                let height = px(2.0).min(line_h);
                (
                    Point::new(x, y + line_h - height),
                    gpui::size(cursor_width, height),
                    cursor_cell.fg,
                    Edges::default(),
                )
            }
            CursorShape::HollowBlock => (
                Point::new(x, y),
                gpui::size(cursor_width, line_h),
                hsla(0., 0., 0., 0.),
                Edges::all(px(1.)),
            ),
            CursorShape::Block | CursorShape::Hidden => (
                Point::new(x, y),
                gpui::size(cursor_width, line_h),
                cursor_cell.fg,
                Edges::default(),
            ),
        };
        let cb = Bounds {
            origin: cursor_origin,
            size: cursor_size,
        };
        window.paint_quad(quad(
            cb,
            Corners::default(),
            cursor_fill,
            cursor_edges,
            cursor_cell.fg,
            gpui::BorderStyle::default(),
        ));

        // The cursor quad is painted after the row text, so repaint the cell's
        // glyph with the effective background color to keep the character
        // readable instead of hiding it beneath the cursor block.
        if snapshot.cursor_shape == CursorShape::Block && !cursor_cell.spacer {
            let mut cursor_text =
                String::with_capacity(cursor_cell.ch.len_utf8() + cursor_cell.zero_width.len());
            cursor_text.push(cursor_cell.ch);
            cursor_text.push_str(&cursor_cell.zero_width);
            let underline = match cursor_cell.underline {
                UnderlineKind::None => None,
                UnderlineKind::Solid => Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(cursor_cell.underline_color),
                    wavy: false,
                }),
                UnderlineKind::Wavy => Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(cursor_cell.underline_color),
                    wavy: true,
                }),
            };
            let text_run = TextRun {
                len: cursor_text.len(),
                font: Font {
                    weight: if cursor_cell.bold {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    },
                    style: if cursor_cell.italic {
                        gpui::FontStyle::Italic
                    } else {
                        gpui::FontStyle::Normal
                    },
                    family: font.family.clone(),
                    features: font.features.clone(),
                    fallbacks: font.fallbacks.clone(),
                },
                color: if cursor_cell.bg == default_bg {
                    default_bg
                } else {
                    cursor_cell.bg
                },
                background_color: None,
                underline,
                strikethrough: cursor_cell.strikeout.then(|| StrikethroughStyle {
                    thickness: px(1.0),
                    color: Some(cursor_cell.bg),
                }),
            };
            let shaped = window.text_system().shape_line(
                SharedString::from(cursor_text),
                px(font_size),
                &[text_run],
                None,
            );
            if let Err(error) =
                shaped.paint(Point::new(x, y), line_h, TextAlign::Left, None, window, cx)
            {
                log::warn!("paint cursor glyph failed: {error}");
            }
        }
    }

    // 合成阶段的拼音由输入法暂存，不能提前写入 PTY；在光标处绘制出来，
    // 让用户能看到当前正在组合的文本，提交后才由 replace_text_in_range 发送。
    if !ime_marked_text.is_empty()
        && let Some((col, row)) = snapshot.cursor
    {
        let origin = Point::new(
            bounds.origin.x + px(col as f32 * cell_wf),
            bounds.origin.y + px(row as f32 * line_hf),
        );
        let marked_runs = [TextRun {
            len: ime_marked_text.len(),
            font: font.clone(),
            color: default_fg,
            background_color: Some(default_bg),
            underline: Some(gpui::UnderlineStyle {
                thickness: px(1.0),
                color: Some(default_fg),
                wavy: false,
            }),
            strikethrough: None,
        }];
        let shaped = window.text_system().shape_line(
            SharedString::from(ime_marked_text.to_string()),
            px(font_size),
            &marked_runs,
            None,
        );
        let width = shaped.width().max(cell_w);
        window.paint_quad(quad(
            Bounds {
                origin,
                size: gpui::size(width, line_h),
            },
            Corners::default(),
            default_bg,
            Edges::default(),
            hsla(0., 0., 0., 0.),
            gpui::BorderStyle::default(),
        ));
        if let Err(e) = shaped.paint(origin, line_h, TextAlign::Left, None, window, cx) {
            log::warn!("paint IME marked text failed: {e}");
        }
    }

    // 滚动条指示器（右侧窄条）。
    let display_offset = snapshot.display_offset;
    let history_len = snapshot.history_len;
    if history_len > 0 && display_offset > 0 {
        let sb_w = px(6.);
        let sb_x = bounds.right() - sb_w;
        let sb_h = bounds.size.height;
        let thumb_h =
            sb_h * (snapshot.rows.len() as f32 / (history_len + snapshot.rows.len()) as f32);
        let thumb_y = sb_h * ((history_len - display_offset) as f32 / history_len as f32);
        window.paint_quad(quad(
            Bounds {
                origin: Point::new(sb_x, bounds.origin.y),
                size: gpui::size(sb_w, sb_h),
            },
            Corners::default(),
            hsla(0., 0., 0.2, 0.15),
            Edges::default(),
            hsla(0., 0., 0., 0.),
            gpui::BorderStyle::default(),
        ));
        window.paint_quad(quad(
            Bounds {
                origin: Point::new(sb_x, bounds.origin.y + thumb_y),
                size: gpui::size(sb_w, thumb_h.min(sb_h)),
            },
            Corners::default(),
            hsla(0., 0., 0.5, 0.3),
            Edges::default(),
            hsla(0., 0., 0., 0.),
            gpui::BorderStyle::default(),
        ));
    }
}

fn bg_of(term: &Term<NoopListener>) -> Hsla {
    color_to_hsla(&Color::Named(NamedColor::Background), term.colors())
        .unwrap_or_else(|| default_palette(&NamedColor::Background))
}
fn fg_of(term: &Term<NoopListener>) -> Hsla {
    color_to_hsla(&Color::Named(NamedColor::Foreground), term.colors())
        .unwrap_or_else(|| default_palette(&NamedColor::Foreground))
}

const MAX_KITTY_NOTIFICATION_TEXT_BYTES: usize = 8 * 1024;

fn append_bounded_notification_text(target: &mut String, value: &str) {
    let remaining = MAX_KITTY_NOTIFICATION_TEXT_BYTES.saturating_sub(target.len());
    if remaining == 0 {
        return;
    }
    let mut end = remaining.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    target.push_str(&value[..end]);
}

fn terminal_image_format(bytes: &[u8]) -> Option<gpui::ImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(gpui::ImageFormat::Png)
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some(gpui::ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(gpui::ImageFormat::Gif)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(gpui::ImageFormat::Webp)
    } else {
        None
    }
}

fn terminal_image_within_limits(data: &[u8], format: gpui::ImageFormat) -> bool {
    if format != gpui::ImageFormat::Png {
        return true;
    }
    let Ok(reader) = png::Decoder::new(Cursor::new(data)).read_info() else {
        return false;
    };
    let width = reader.info().width as usize;
    let height = reader.info().height as usize;
    width > 0
        && height > 0
        && width <= MAX_IMAGE_DIMENSION
        && height <= MAX_IMAGE_DIMENSION
        && width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .is_some_and(|bytes| bytes <= MAX_DECODED_IMAGE_BYTES)
}

fn encode_rgba_png(pixels: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        return None;
    }
    let expected = width.checked_mul(height)?.checked_mul(4)?;
    if expected != pixels.len() || pixels.len() > MAX_DECODED_IMAGE_BYTES {
        return None;
    }

    let mut encoded = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut encoded, width as u32, height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(pixels).ok()?;
    }
    Some(encoded)
}

fn kitty_raw_to_png(data: &[u8], width: usize, height: usize, channels: usize) -> Option<Vec<u8>> {
    if !matches!(channels, 3 | 4) {
        return None;
    }
    let pixel_count = width.checked_mul(height)?;
    let expected = pixel_count.checked_mul(channels)?;
    if expected != data.len() {
        return None;
    }

    if channels == 4 {
        return encode_rgba_png(data, width, height);
    }

    let mut rgba = Vec::with_capacity(pixel_count.checked_mul(4)?);
    for rgb in data.chunks_exact(3) {
        rgba.extend_from_slice(rgb);
        rgba.push(0xff);
    }
    encode_rgba_png(&rgba, width, height)
}

fn crop_kitty_image(data: &[u8], placement: KittyPlacement) -> Option<Vec<u8>> {
    if placement.source_x.is_none()
        && placement.source_y.is_none()
        && placement.source_width.is_none()
        && placement.source_height.is_none()
    {
        return Some(data.to_vec());
    }
    if terminal_image_format(data) != Some(gpui::ImageFormat::Png) {
        return None;
    }

    let mut decoder = png::Decoder::new(Cursor::new(data));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let width = reader.info().width as usize;
    let height = reader.info().height as usize;
    if width == 0 || height == 0 {
        return None;
    }
    let decoded_size = width.checked_mul(height)?.checked_mul(4)?;
    if decoded_size > MAX_DECODED_IMAGE_BYTES {
        return None;
    }
    let mut buffer = vec![0; reader.output_buffer_size()];
    let output = reader.next_frame(&mut buffer).ok()?;
    let bytes = &buffer[..output.buffer_size()];
    let rgba = match output.color_type {
        png::ColorType::Rgba if output.bit_depth == png::BitDepth::Eight => bytes.to_vec(),
        png::ColorType::Rgb if output.bit_depth == png::BitDepth::Eight => {
            let mut rgba = Vec::with_capacity(decoded_size);
            for pixel in bytes.chunks_exact(3) {
                rgba.extend_from_slice(pixel);
                rgba.push(0xff);
            }
            rgba
        }
        png::ColorType::Grayscale if output.bit_depth == png::BitDepth::Eight => {
            let mut rgba = Vec::with_capacity(decoded_size);
            for &gray in bytes {
                rgba.extend_from_slice(&[gray, gray, gray, 0xff]);
            }
            rgba
        }
        png::ColorType::GrayscaleAlpha if output.bit_depth == png::BitDepth::Eight => {
            let mut rgba = Vec::with_capacity(decoded_size);
            for pixel in bytes.chunks_exact(2) {
                rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
            rgba
        }
        _ => return None,
    };

    let x = placement.source_x.unwrap_or(0).min(width);
    let y = placement.source_y.unwrap_or(0).min(height);
    let crop_width = placement
        .source_width
        .unwrap_or(width.saturating_sub(x))
        .min(width.saturating_sub(x));
    let crop_height = placement
        .source_height
        .unwrap_or(height.saturating_sub(y))
        .min(height.saturating_sub(y));
    if crop_width == 0 || crop_height == 0 {
        return None;
    }
    let mut cropped = Vec::with_capacity(crop_width.checked_mul(crop_height)?.checked_mul(4)?);
    for row in y..y + crop_height {
        let start = row.checked_mul(width)?.checked_add(x)?.checked_mul(4)?;
        let end = start.checked_add(crop_width.checked_mul(4)?)?;
        cropped.extend_from_slice(rgba.get(start..end)?);
    }
    encode_rgba_png(&cropped, crop_width, crop_height)
}

fn kitty_zlib_decode(data: &[u8]) -> Option<Vec<u8>> {
    let decoder = ZlibDecoder::new(data);
    let mut decoded = Vec::new();
    decoder
        .take((MAX_DECODED_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut decoded)
        .ok()?;
    (decoded.len() <= MAX_DECODED_IMAGE_BYTES).then_some(decoded)
}

fn kitty_parameter<'a>(control: &'a str, key: &str) -> Option<&'a str> {
    control.split(',').find_map(|field| {
        let (field_key, value) = field.split_once('=')?;
        (field_key == key).then_some(value)
    })
}

fn sanitize_kitty_notification_id(value: &str) -> Option<String> {
    let id = value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '+' | '.')
        })
        .take(128)
        .collect::<String>();
    (!id.is_empty()).then_some(id)
}

fn kitty_image_action<'a>(control: &'a str, stored_action: Option<&'a str>) -> &'a str {
    kitty_parameter(control, "a")
        .or(stored_action)
        .unwrap_or("t")
}

/// 把 alacritty/vte 的 Color 解析为 Hsla。Named/Indexed 走终端调色板，Spec 直传。
fn color_to_hsla(color: &Color, colors: &alacritty_terminal::term::color::Colors) -> Option<Hsla> {
    match color {
        Color::Spec(rgb) => Some(rgb_to_hsla(*rgb)),
        Color::Named(n) => {
            let idx = *n as usize;
            colors[idx]
                .map(rgb_to_hsla)
                .or_else(|| Some(default_palette(n)))
        }
        Color::Indexed(i) => {
            let idx = *i as usize;
            if idx < 256 {
                colors[idx]
                    .map(rgb_to_hsla)
                    .or_else(|| Some(default_palette_indexed(idx)))
            } else {
                None
            }
        }
    }
}

fn rgb_to_hsla(Rgb { r, g, b }: Rgb) -> Hsla {
    Hsla::from(gpui::rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32))
}

/// 内置默认 16 色调色板（极简）。
fn default_palette(n: &NamedColor) -> Hsla {
    rgb_to_hsla(default_palette_rgb(n))
}

fn default_palette_rgb(n: &NamedColor) -> Rgb {
    use NamedColor::*;
    let rgb = match n {
        Black | DimBlack => [0x00, 0x00, 0x00],
        Red | DimRed => [0xc5, 0x28, 0x28],
        Green | DimGreen => [0x23, 0xa1, 0x2e],
        Yellow | DimYellow => [0xc0, 0x8c, 0x1e],
        Blue | DimBlue => [0x10, 0x7d, 0xcf],
        Magenta | DimMagenta => [0xbe, 0x3e, 0xbe],
        Cyan | DimCyan => [0x12, 0x9a, 0xa1],
        White | DimWhite => [0xc0, 0xc0, 0xc0],
        BrightBlack => [0x76, 0x76, 0x76],
        BrightRed => [0xff, 0x6b, 0x6b],
        BrightGreen => [0x52, 0xd4, 0x52],
        BrightYellow => [0xff, 0xd1, 0x73],
        BrightBlue => [0x6b, 0xb6, 0xff],
        BrightMagenta => [0xff, 0x7e, 0xff],
        BrightCyan => [0x6b, 0xe7, 0xeb],
        BrightWhite | BrightForeground => [0xff, 0xff, 0xff],
        Foreground => [0xe7, 0xed, 0xf1],
        Background => [0x0f, 0x11, 0x14],
        Cursor => [0x69, 0xd7, 0xb0],
        DimForeground => [0x9a, 0xa6, 0xb0],
    };
    Rgb {
        r: rgb[0],
        g: rgb[1],
        b: rgb[2],
    }
}

/// 256 色的回退（xterm 配色：16 色 + 6×6×6 立方 + 24 级灰度）。
fn default_palette_indexed(i: usize) -> Hsla {
    rgb_to_hsla(default_palette_indexed_rgb(i))
}

fn default_palette_indexed_rgb(i: usize) -> Rgb {
    if i < 16 {
        // 前 16 色用内置调色板里的对应项。
        let n = match i {
            0 => NamedColor::Black,
            1 => NamedColor::Red,
            2 => NamedColor::Green,
            3 => NamedColor::Yellow,
            4 => NamedColor::Blue,
            5 => NamedColor::Magenta,
            6 => NamedColor::Cyan,
            7 => NamedColor::White,
            8 => NamedColor::BrightBlack,
            9 => NamedColor::BrightRed,
            10 => NamedColor::BrightGreen,
            11 => NamedColor::BrightYellow,
            12 => NamedColor::BrightBlue,
            13 => NamedColor::BrightMagenta,
            14 => NamedColor::BrightCyan,
            _ => NamedColor::BrightWhite,
        };
        default_palette_rgb(&n)
    } else if i < 232 {
        let i = i - 16;
        let r = i / 36;
        let g = (i / 6) % 6;
        let b = i % 6;
        let v = |x: usize| if x == 0 { 0 } else { 0x37 + 0x28 * x };
        Rgb {
            r: v(r) as u8,
            g: v(g) as u8,
            b: v(b) as u8,
        }
    } else {
        let v = 8 + (i - 232) * 10;
        Rgb {
            r: v.min(255) as u8,
            g: v.min(255) as u8,
            b: v.min(255) as u8,
        }
    }
}

fn default_palette_rgb_index(index: usize) -> Option<Rgb> {
    let named = match index {
        256 => NamedColor::Foreground,
        257 => NamedColor::Background,
        258 => NamedColor::Cursor,
        259 => NamedColor::DimBlack,
        260 => NamedColor::DimRed,
        261 => NamedColor::DimGreen,
        262 => NamedColor::DimYellow,
        263 => NamedColor::DimBlue,
        264 => NamedColor::DimMagenta,
        265 => NamedColor::DimCyan,
        266 => NamedColor::DimWhite,
        267 => NamedColor::BrightForeground,
        268 => NamedColor::DimForeground,
        _ => return (index < 256).then(|| default_palette_indexed_rgb(index)),
    };
    Some(default_palette_rgb(&named))
}

fn dimen(c: Hsla) -> Hsla {
    Hsla { a: c.a * 0.6, ..c }
}

// ─── 鼠标编码 ────────────────────────────────────────────────────────────────
/// 将鼠标按钮转换为 xterm 编码值（左=0, 中=1, 右=2）。
fn mouse_button_code(btn: MouseButton) -> Option<u8> {
    match btn {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        MouseButton::Navigate(_) => None,
    }
}

fn mouse_modifier_bits(mods: &Modifiers) -> u8 {
    let mut bits = 0;
    if mods.shift {
        bits |= 4;
    }
    if mods.alt {
        bits |= 8;
    }
    if mods.control {
        bits |= 16;
    }
    bits
}

/// 按当前终端模式生成 SGR、urxvt、UTF-8 扩展或传统 xterm 鼠标序列。
fn encode_mouse_report(
    button: u8,
    col: usize,
    row: usize,
    pressed: bool,
    mods: &Modifiers,
    mode: TermMode,
    urxvt_mouse: bool,
) -> Option<Vec<u8>> {
    let button = if pressed { button } else { 3 };
    let cb = button | mouse_modifier_bits(mods);

    if urxvt_mouse {
        return Some(format!("\x1b[{};{};{}M", cb, col + 1, row + 1).into_bytes());
    }

    if mode.contains(TermMode::SGR_MOUSE) {
        let suffix = if pressed { 'M' } else { 'm' };
        return Some(format!("\x1b[<{};{};{}{}", cb, col + 1, row + 1, suffix).into_bytes());
    }

    encode_normal_mouse(cb, col, row, mode.contains(TermMode::UTF8_MOUSE))
}

/// 传统 `ESC[M` 鼠标协议最多只能表示 223 列/行；UTF-8 变体可扩展到 2015。
fn encode_normal_mouse(button: u8, col: usize, row: usize, utf8: bool) -> Option<Vec<u8>> {
    let max_point = if utf8 { 2015 } else { 223 };
    if col >= max_point || row >= max_point {
        return None;
    }

    let encode_position = |position: usize| -> Vec<u8> {
        let position = position + 33;
        if utf8 && position >= 128 {
            vec![(0xc0 + position / 64) as u8, (0x80 + (position & 63)) as u8]
        } else {
            vec![position as u8]
        }
    };

    let mut bytes = vec![0x1b, b'[', b'M', 32 + button];
    bytes.extend(encode_position(col));
    bytes.extend(encode_position(row));
    Some(bytes)
}

// ─── 输入编码 ───────────────────────────────────────────────────────────────
fn osc52_text_within_limit(text: &str) -> bool {
    text.len() <= MAX_OSC52_CLIPBOARD_BYTES
}

fn osc52_mode(is_local: bool) -> Osc52 {
    if is_local {
        Osc52::CopyPaste
    } else {
        Osc52::OnlyCopy
    }
}

fn osc52_load_allowed(is_local: bool) -> bool {
    is_local
}

fn take_protocol_responses(queue: &ProtocolResponseQueue) -> Vec<Vec<u8>> {
    match queue.lock() {
        Ok(mut queue) => queue.drain(..).collect::<Vec<_>>(),
        Err(poisoned) => poisoned.into_inner().drain(..).collect::<Vec<_>>(),
    }
}

fn format_osc52_response(
    formatter: &Arc<dyn Fn(&str) -> String + Sync + Send + 'static>,
    text: &str,
) -> Option<Vec<u8>> {
    if !osc52_text_within_limit(text) {
        return None;
    }
    let response = formatter(text);
    (response.len() <= MAX_OSC52_RESPONSE_BYTES).then(|| response.into_bytes())
}

fn decode_hex_bytes(value: &[u8]) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .chunks_exact(2)
        .map(|pair| Some((hex_value(pair[0])? << 4) | hex_value(pair[1])?))
        .collect()
}

fn encode_hex_bytes(value: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(value.len() * 2);
    for &byte in value {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// 调试用：把字节流转成可读字符串（控制字符转义，ESC 显示为 \x1b）。
fn debug_bytes(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len());
    for &byte in b {
        match byte {
            b'\r' => out.push_str("\\r"),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7e => out.push(byte as char),
            _ => out.push_str(&format!("\\x{:02x}", byte)),
        }
    }
    out
}

/// 临时尺寸结构（避免与 gpui::Size 命名冲突的内部用）。
struct Size {
    w: f32,
    h: f32,
}

/// 把 gpui Keystroke 编码成发给 PTY 的字节序列（xterm 风格）。
#[allow(dead_code)]
pub fn encode_keystroke(ks: &gpui::Keystroke) -> Option<Vec<u8>> {
    encode_keystroke_with_mode(ks, TermMode::NONE)
}

fn encode_keystroke_with_mode(ks: &gpui::Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    encode_keystroke_with_event(ks, mode, 1)
}

fn encode_keystroke_with_event(
    ks: &gpui::Keystroke,
    mode: TermMode,
    event_type: u8,
) -> Option<Vec<u8>> {
    encode_keystroke_with_options(ks, mode, event_type, 0)
}

fn encode_keystroke_with_options(
    ks: &gpui::Keystroke,
    mode: TermMode,
    event_type: u8,
    modify_other_keys: u8,
) -> Option<Vec<u8>> {
    let m = &ks.modifiers;
    let key = ks.key.as_str();
    let has_modifiers = m.shift || m.alt || m.control || m.platform;

    if let Some(bytes) = encode_kitty_keystroke(ks, mode, event_type) {
        return Some(bytes);
    }

    if let Some(bytes) = encode_modify_other_keys(ks, modify_other_keys) {
        return Some(bytes);
    }

    // Ctrl+letter and the ASCII control punctuation are single-byte controls.
    // Keep Alt+Ctrl as ESC followed by the control byte, which is what shells
    // and most full-screen applications expect for a Meta control key.
    if m.control
        && !m.platform
        && let Some(control) = control_code(key)
    {
        if m.alt {
            return Some(vec![0x1b, control]);
        }
        return Some(vec![control]);
    }

    match key {
        "enter" | "return" => {
            if m.shift && !m.alt && !m.control && !m.platform {
                return Some(vec![b'\n']);
            }
            if m.alt && !m.control && !m.platform {
                return Some(if m.shift {
                    vec![0x1b, b'\n']
                } else {
                    vec![0x1b, b'\r']
                });
            }
            if m.control && !m.alt && !m.platform {
                return Some(vec![b'\n']);
            }
            if !has_modifiers {
                return Some(vec![b'\r']);
            }
        }
        "back" | "backspace" => {
            if m.alt && !m.platform {
                return Some(vec![0x1b, 0x7f]);
            }
            if m.control && !m.platform {
                return Some(vec![0x08]);
            }
            if !m.platform {
                return Some(vec![0x7f]);
            }
        }
        "tab" => {
            if m.shift && !m.alt && !m.control && !m.platform {
                return Some(b"\x1b[Z".to_vec());
            }
            if m.alt && !m.platform {
                return Some(vec![0x1b, b'\t']);
            }
            if !m.control && !m.platform {
                return Some(vec![b'\t']);
            }
        }
        "escape" if !m.platform => {
            return Some(if m.alt { vec![0x1b, 0x1b] } else { vec![0x1b] });
        }
        "space" => {
            if m.control && !m.platform {
                return Some(vec![0]);
            }
            if !m.control && !m.platform {
                return Some(if m.alt { vec![0x1b, b' '] } else { vec![b' '] });
            }
        }
        _ => {}
    }

    if !has_modifiers {
        if let Some(bytes) = keypad_key(key, mode) {
            return Some(bytes);
        }
        // A plain printable key is not a special key. Only return here when
        // the lookup actually matched; otherwise continue to the text path.
        if let Some(bytes) = plain_special_key(key, mode) {
            return Some(bytes);
        }
    } else if let Some(bytes) = modified_special_key(key, m) {
        return Some(bytes);
    }

    // 可打印字符：优先用 key_char（已含 shift/option 组合结果），再回退到
    // ASCII key。Cmd/platform 组合由上层快捷键处理，不发送为普通文本。
    if !m.control && !m.platform {
        let ch = ks
            .key_char
            .as_ref()
            .and_then(|value| value.chars().next())
            .or_else(|| key.chars().next())?;
        let ch = if m.shift && ks.key_char.is_none() {
            shifted_ascii_char(ch)
        } else {
            ch
        };
        if !ch.is_control() {
            let mut text = ch.to_string();
            if m.alt {
                text.insert(0, '\x1b');
            }
            return Some(text.into_bytes());
        }
    }
    None
}

/// Encode xterm's modifyOtherKeys extension. The extension deliberately only
/// handles ordinary keys; arrows, function keys, keypad keys, and the common
/// special keys keep their established encodings. Level 3 also reports
/// unmodified ordinary keys.
fn encode_modify_other_keys(ks: &gpui::Keystroke, level: u8) -> Option<Vec<u8>> {
    if !matches!(level, 1..=3) || ks.modifiers.platform {
        return None;
    }
    let key = ks.key.as_str();
    if is_kitty_functional_key(key)
        || matches!(
            key,
            "enter" | "return" | "tab" | "back" | "backspace" | "escape"
        )
    {
        return None;
    }
    let modifiers = &ks.modifiers;
    let modified = modifiers.shift || modifiers.alt || modifiers.control;
    if level != 3
        && (!modified || (level == 1 && modifiers.alt && !modifiers.shift && !modifiers.control))
    {
        return None;
    }
    if level == 1 && control_code(key).is_some() {
        return None;
    }
    let (code, _) = kitty_text_key_code(ks)?;
    Some(format!("\x1b[27;{};{}~", modifier_code(modifiers), code).into_bytes())
}

/// 生成 Kitty 键盘协议的增强编码。
///
/// 保持 Enter/Tab/Backspace 在 disambiguate 模式下的传统字节，避免应用
/// 崩溃后用户无法在 shell 中输入 `reset`；REPORT_ALL 模式则按协议编码全部键。
fn encode_kitty_keystroke(ks: &gpui::Keystroke, mode: TermMode, event_type: u8) -> Option<Vec<u8>> {
    let report_all = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let disambiguate = mode.contains(TermMode::DISAMBIGUATE_ESC_CODES);
    if !report_all && !disambiguate {
        return None;
    }

    let key = ks.key.as_str();
    let modifiers = &ks.modifiers;
    if !report_all && matches!(key, "enter" | "return" | "tab" | "back" | "backspace") {
        return None;
    }

    if report_all || (disambiguate && mode.contains(TermMode::REPORT_EVENT_TYPES)) {
        if let Some(bytes) = encode_kitty_functional_key(key, modifiers, mode, event_type) {
            return Some(bytes);
        }
    } else if let Some(code) = kitty_private_key_code(key) {
        return Some(encode_kitty_u(code, modifiers, mode, event_type, None));
    }

    // In disambiguate mode only Escape and modified text keys switch from the
    // legacy byte encoding to CSI u. REPORT_ALL applies this to plain text too.
    let needs_escape_encoding = report_all
        || key == "escape"
        || (disambiguate && (modifiers.alt || modifiers.control || modifiers.platform));
    if !needs_escape_encoding || is_kitty_functional_key(key) {
        return None;
    }

    let (code, alternate) = kitty_text_key_code(ks)?;
    let key_code = if mode.contains(TermMode::REPORT_ALTERNATE_KEYS) {
        alternate
            .map(|alternate| format!("{code}:{alternate}"))
            .unwrap_or_else(|| code.to_string())
    } else {
        code.to_string()
    };
    let associated_text =
        if report_all && event_type == 1 && mode.contains(TermMode::REPORT_ASSOCIATED_TEXT) {
            associated_text_code(ks)
        } else {
            None
        };
    Some(encode_kitty_u(
        key_code,
        modifiers,
        mode,
        event_type,
        associated_text,
    ))
}

fn encode_kitty_functional_key(
    key: &str,
    modifiers: &Modifiers,
    mode: TermMode,
    event_type: u8,
) -> Option<Vec<u8>> {
    let modifier = modifier_code(modifiers);
    let report_event = mode.contains(TermMode::REPORT_EVENT_TYPES);

    let final_byte = match key {
        "up" => Some('A'),
        "down" => Some('B'),
        "right" => Some('C'),
        "left" => Some('D'),
        "home" => Some('H'),
        "end" => Some('F'),
        _ => None,
    };
    if let Some(final_byte) = final_byte {
        let sequence = if modifier == 1 && !report_event {
            format!("\x1b[{final_byte}")
        } else {
            let event = if report_event {
                format!(":{event_type}")
            } else {
                String::new()
            };
            format!("\x1b[1;{modifier}{event}{final_byte}")
        };
        return Some(sequence.into_bytes());
    }

    let tilde_code = match key {
        "insert" => Some(2),
        "delete" => Some(3),
        "pageup" => Some(5),
        "pagedown" => Some(6),
        "f1" => Some(11),
        "f2" => Some(12),
        "f3" => Some(13),
        "f4" => Some(14),
        "f5" => Some(15),
        "f6" => Some(17),
        "f7" => Some(18),
        "f8" => Some(19),
        "f9" => Some(20),
        "f10" => Some(21),
        "f11" => Some(23),
        "f12" => Some(24),
        _ => None,
    };
    if let Some(tilde_code) = tilde_code {
        let sequence = if modifier == 1 && !report_event {
            format!("\x1b[{tilde_code}~")
        } else {
            let event = if report_event {
                format!(":{event_type}")
            } else {
                String::new()
            };
            format!("\x1b[{tilde_code};{modifier}{event}~")
        };
        return Some(sequence.into_bytes());
    }

    let private_code = kitty_private_key_code(key)?;
    Some(encode_kitty_u(
        private_code,
        modifiers,
        mode,
        event_type,
        None,
    ))
}

fn encode_kitty_u(
    key_code: impl std::fmt::Display,
    modifiers: &Modifiers,
    mode: TermMode,
    event_type: u8,
    associated_text: Option<u32>,
) -> Vec<u8> {
    let modifier = modifier_code(modifiers);
    let report_event = mode.contains(TermMode::REPORT_EVENT_TYPES);
    let needs_modifier = modifier != 1 || report_event || associated_text.is_some();
    let mut sequence = format!("\x1b[{}", key_code);
    if needs_modifier {
        sequence.push(';');
        sequence.push_str(&modifier.to_string());
        if report_event {
            sequence.push(':');
            sequence.push_str(&event_type.to_string());
        }
    }
    if let Some(text) = associated_text {
        sequence.push(';');
        sequence.push_str(&text.to_string());
    }
    sequence.push('u');
    sequence.into_bytes()
}

fn kitty_text_key_code(ks: &gpui::Keystroke) -> Option<(u32, Option<u32>)> {
    let key = ks.key.as_str();
    let base = match key {
        "escape" => 27,
        "enter" | "return" => 13,
        "tab" => 9,
        "back" | "backspace" => 127,
        "space" => 32,
        _ if is_kitty_functional_key(key) => return None,
        _ => {
            let ch = key.chars().next()?;
            if ch.is_control() {
                return None;
            }
            ch as u32
        }
    };
    let base = if base <= 0x7f {
        (base as u8 as char).to_ascii_lowercase() as u32
    } else {
        base
    };

    let alternate = ks
        .key_char
        .as_ref()
        .and_then(|value| value.chars().next())
        .or_else(|| {
            ks.modifiers
                .shift
                .then(|| key.chars().next())
                .flatten()
                .map(shifted_ascii_char)
        })
        .filter(|_| ks.modifiers.shift)
        .filter(|ch| !ch.is_control())
        .map(|ch| ch as u32)
        .filter(|alternate| *alternate != base);
    Some((base, alternate))
}

fn associated_text_code(ks: &gpui::Keystroke) -> Option<u32> {
    if ks.modifiers.control || ks.modifiers.platform {
        return None;
    }
    let ch = ks
        .key_char
        .as_ref()
        .and_then(|value| value.chars().next())
        .or_else(|| ks.key.chars().next())
        .map(|ch| {
            if ks.modifiers.shift && ks.key_char.is_none() {
                shifted_ascii_char(ch)
            } else {
                ch
            }
        })?;
    if ch.is_control() || (0x80..=0x9f).contains(&(ch as u32)) {
        None
    } else {
        Some(ch as u32)
    }
}

/// Encode the xterm application keypad. GPUI uses slightly different key
/// names across desktop backends, so accept the common aliases here.
fn keypad_key(key: &str, mode: TermMode) -> Option<Vec<u8>> {
    let (normal, application) = match key {
        "kp0" | "numpad0" | "num0" => (b'0', "\x1bOp"),
        "kp1" | "numpad1" | "num1" => (b'1', "\x1bOq"),
        "kp2" | "numpad2" | "num2" => (b'2', "\x1bOr"),
        "kp3" | "numpad3" | "num3" => (b'3', "\x1bOs"),
        "kp4" | "numpad4" | "num4" => (b'4', "\x1bOt"),
        "kp5" | "numpad5" | "num5" => (b'5', "\x1bOu"),
        "kp6" | "numpad6" | "num6" => (b'6', "\x1bOv"),
        "kp7" | "numpad7" | "num7" => (b'7', "\x1bOw"),
        "kp8" | "numpad8" | "num8" => (b'8', "\x1bOx"),
        "kp9" | "numpad9" | "num9" => (b'9', "\x1bOy"),
        "kpdecimal" | "numpaddecimal" => (b'.', "\x1bOn"),
        "kpcomma" | "numpadcomma" => (b',', "\x1bOl"),
        "kpminus" | "numpadminus" => (b'-', "\x1bOm"),
        "kpplus" | "numpadplus" => (b'+', "\x1bOk"),
        "kpmultiply" | "numpadmultiply" => (b'*', "\x1bOj"),
        "kpdivide" | "numpaddivide" => (b'/', "\x1bOo"),
        "kpenter" | "numpadenter" => (b'\r', "\x1bOM"),
        _ => return None,
    };
    if mode.contains(TermMode::APP_KEYPAD) {
        Some(application.as_bytes().to_vec())
    } else {
        Some(vec![normal])
    }
}

fn shifted_ascii_char(ch: char) -> char {
    match ch {
        'a'..='z' => ch.to_ascii_uppercase(),
        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '0' => ')',
        '-' => '_',
        '=' => '+',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        ',' => '<',
        '.' => '>',
        '/' => '?',
        '`' => '~',
        _ => ch,
    }
}

fn is_kitty_functional_key(key: &str) -> bool {
    matches!(
        key,
        "up" | "down"
            | "left"
            | "right"
            | "home"
            | "end"
            | "insert"
            | "delete"
            | "pageup"
            | "pagedown"
            | "f1"
            | "f2"
            | "f3"
            | "f4"
            | "f5"
            | "f6"
            | "f7"
            | "f8"
            | "f9"
            | "f10"
            | "f11"
            | "f12"
            | "f13"
            | "f14"
            | "f15"
            | "f16"
            | "f17"
            | "f18"
            | "f19"
            | "f20"
            | "f21"
            | "f22"
            | "f23"
            | "f24"
            | "f25"
            | "f26"
            | "f27"
            | "f28"
            | "f29"
            | "f30"
            | "f31"
            | "f32"
            | "f33"
            | "f34"
            | "f35"
    )
}

fn kitty_private_key_code(key: &str) -> Option<u32> {
    let function = key.strip_prefix('f')?.parse::<u32>().ok()?;
    if (13..=35).contains(&function) {
        Some(57376 + function - 13)
    } else {
        None
    }
}

fn control_code(key: &str) -> Option<u8> {
    if key.chars().count() != 1 {
        return None;
    }
    let ch = key.chars().next()?.to_ascii_lowercase();
    match ch {
        'a'..='z' => Some(ch as u8 - b'a' + 1),
        '@' => Some(0),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        '?' => Some(0x7f),
        _ => None,
    }
}

fn plain_special_key(key: &str, mode: TermMode) -> Option<Vec<u8>> {
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let sequence = match key {
        "up" => {
            if app_cursor {
                "\x1bOA"
            } else {
                "\x1b[A"
            }
        }
        "down" => {
            if app_cursor {
                "\x1bOB"
            } else {
                "\x1b[B"
            }
        }
        "right" => {
            if app_cursor {
                "\x1bOC"
            } else {
                "\x1b[C"
            }
        }
        "left" => {
            if app_cursor {
                "\x1bOD"
            } else {
                "\x1b[D"
            }
        }
        "home" => {
            if app_cursor {
                "\x1bOH"
            } else {
                "\x1b[H"
            }
        }
        "end" => {
            if app_cursor {
                "\x1bOF"
            } else {
                "\x1b[F"
            }
        }
        "insert" => "\x1b[2~",
        "delete" => "\x1b[3~",
        "pageup" => "\x1b[5~",
        "pagedown" => "\x1b[6~",
        "f1" => "\x1bOP",
        "f2" => "\x1bOQ",
        "f3" => "\x1bOR",
        "f4" => "\x1bOS",
        "f5" => "\x1b[15~",
        "f6" => "\x1b[17~",
        "f7" => "\x1b[18~",
        "f8" => "\x1b[19~",
        "f9" => "\x1b[20~",
        "f10" => "\x1b[21~",
        "f11" => "\x1b[23~",
        "f12" => "\x1b[24~",
        "f13" => "\x1b[25~",
        "f14" => "\x1b[26~",
        "f15" => "\x1b[28~",
        "f16" => "\x1b[29~",
        "f17" => "\x1b[31~",
        "f18" => "\x1b[32~",
        "f19" => "\x1b[33~",
        "f20" => "\x1b[34~",
        "f21" => "\x1b[38~",
        "f22" => "\x1b[39~",
        "f23" => "\x1b[40~",
        "f24" => "\x1b[41~",
        "f25" => "\x1b[42~",
        "f26" => "\x1b[43~",
        "f27" => "\x1b[44~",
        "f28" => "\x1b[45~",
        "f29" => "\x1b[46~",
        "f30" => "\x1b[47~",
        "f31" => "\x1b[48~",
        "f32" => "\x1b[49~",
        "f33" => "\x1b[50~",
        "f34" => "\x1b[51~",
        "f35" => "\x1b[52~",
        _ => return None,
    };
    Some(sequence.as_bytes().to_vec())
}

fn modified_special_key(key: &str, modifiers: &Modifiers) -> Option<Vec<u8>> {
    let code = modifier_code(modifiers);
    let arrow = match key {
        "up" => Some('A'),
        "down" => Some('B'),
        "right" => Some('C'),
        "left" => Some('D'),
        "home" => Some('H'),
        "end" => Some('F'),
        "f1" => Some('P'),
        "f2" => Some('Q'),
        "f3" => Some('R'),
        "f4" => Some('S'),
        _ => None,
    };
    if let Some(final_byte) = arrow {
        return Some(format!("\x1b[1;{}{}", code, final_byte).into_bytes());
    }

    let tilde_code = match key {
        "insert" => Some(2),
        "delete" => Some(3),
        "pageup" => Some(5),
        "pagedown" => Some(6),
        "f5" => Some(15),
        "f6" => Some(17),
        "f7" => Some(18),
        "f8" => Some(19),
        "f9" => Some(20),
        "f10" => Some(21),
        "f11" => Some(23),
        "f12" => Some(24),
        "f13" => Some(25),
        "f14" => Some(26),
        "f15" => Some(28),
        "f16" => Some(29),
        "f17" => Some(31),
        "f18" => Some(32),
        "f19" => Some(33),
        "f20" => Some(34),
        "f21" => Some(38),
        "f22" => Some(39),
        "f23" => Some(40),
        "f24" => Some(41),
        "f25" => Some(42),
        "f26" => Some(43),
        "f27" => Some(44),
        "f28" => Some(45),
        "f29" => Some(46),
        "f30" => Some(47),
        "f31" => Some(48),
        "f32" => Some(49),
        "f33" => Some(50),
        "f34" => Some(51),
        "f35" => Some(52),
        _ => None,
    }?;
    Some(format!("\x1b[{};{}~", tilde_code, code).into_bytes())
}

fn is_shell_shortcut(ks: &gpui::Keystroke) -> bool {
    if !(ks.modifiers.platform || ks.modifiers.control) {
        return false;
    }
    matches!(
        ks.key.as_str(),
        "w" | "t" | "tab" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"
    )
}

/// 计算带修饰键时的 CSI 修饰码（shift=1, alt=2, control=4, meta=8，再 +1）。
fn modifier_code(m: &Modifiers) -> u8 {
    let mut code = 1;
    if m.shift {
        code += 1;
    }
    if m.alt {
        code += 2;
    }
    if m.control {
        code += 4;
    }
    if m.platform {
        code += 8;
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::index::{Column, Line};
    use async_channel::TrySendError;

    fn keystroke(source: &str) -> gpui::Keystroke {
        gpui::Keystroke::parse(source).expect("valid test keystroke")
    }

    #[test]
    fn standard_system_notification_can_be_resolved_by_tag() {
        let states = HashMap::from([(
            "system-0".to_string(),
            NotificationState {
                tag: "crossh-terminal-7-0".to_string(),
                kitty_id: None,
                report_activation: false,
                report_close: false,
                focus_on_activation: true,
            },
        )]);

        let (key, state) =
            notification_state_for_tag(&states, "crossh-terminal-7-0").expect("notification");
        assert_eq!(key, "system-0");
        assert!(state.kitty_id.is_none());
        assert!(state.focus_on_activation);
        assert!(notification_state_for_tag(&states, "missing").is_none());
    }

    #[test]
    fn protocol_parser_tracks_chunked_command_markers() {
        let mut parser = TerminalProtocolParser::default();
        assert!(parser.feed(b"output\x1b]13").is_empty());
        assert_eq!(
            parser.feed(b"3;C\x07command output"),
            vec![ProtocolEvent::Shell(ShellEvent::CommandStart)]
        );
        assert_eq!(
            parser.feed(b"\x1b]133;D;0\x1b\\"),
            vec![ProtocolEvent::Shell(ShellEvent::CommandFinished {
                status: Some(0)
            })]
        );
        assert_eq!(
            parser.feed(b"\x1b]133;A\x07prompt"),
            vec![ProtocolEvent::Shell(ShellEvent::PromptStart)]
        );
    }

    #[test]
    fn selection_columns_are_ordered_for_same_line_drags() {
        assert_eq!(selection_column_bounds(2, 4, 8, 4), (2, 8));
        assert_eq!(selection_column_bounds(8, 4, 2, 4), (2, 8));
    }

    #[test]
    fn timestamps_use_fixed_millisecond_precision() {
        let timestamp = format_timestamp(Local::now());
        assert_eq!(timestamp.len(), 12);
        assert_eq!(timestamp.as_bytes()[8], b'.');
        assert!(
            timestamp
                .bytes()
                .enumerate()
                .all(|(index, byte)| matches!(index, 2 | 5 | 8) || byte.is_ascii_digit())
        );
    }

    #[test]
    fn timestamp_visibility_controls_terminal_content_origin() {
        let bounds = Bounds {
            origin: Point::new(px(10.), px(20.)),
            size: gpui::size(px(640.), px(300.)),
        };
        let with_gutter = terminal_bounds_for(bounds, true);
        assert_eq!(with_gutter.origin.x.as_f32(), 122.0);
        assert_eq!(with_gutter.size.width.as_f32(), 528.0);

        let without_gutter = terminal_bounds_for(bounds, false);
        assert_eq!(without_gutter.origin.x.as_f32(), 10.0);
        assert_eq!(without_gutter.size.width.as_f32(), 640.0);
    }

    #[test]
    fn timestamp_tracker_preserves_rows_when_scrollback_grows() {
        let config = Config {
            scrolling_history: SCROLLBACK,
            ..Default::default()
        };
        let mut term: Term<NoopListener> = Term::new(
            config,
            &TermSize { cols: 20, rows: 2 },
            NoopListener::default(),
        );
        let mut parser: Processor = Processor::new();
        let mut tracker = TerminalTimestampState::default();

        parser.advance(&mut term, b"one\r\ntwo");
        tracker.observe(&term, "10:00:00.001".to_string());
        parser.advance(&mut term, b"\r\nthree");
        tracker.observe(&term, "10:00:00.002".to_string());

        assert_eq!(
            tracker.visible(&term),
            vec![
                Some("10:00:00.001".to_string()),
                Some("10:00:00.002".to_string())
            ]
        );
    }

    #[test]
    fn timestamp_tracker_hides_wrapped_rows_and_alternate_screen() {
        let config = Config {
            scrolling_history: SCROLLBACK,
            ..Default::default()
        };
        let mut term: Term<NoopListener> = Term::new(
            config,
            &TermSize { cols: 5, rows: 3 },
            NoopListener::default(),
        );
        let mut parser: Processor = Processor::new();
        let mut tracker = TerminalTimestampState::default();

        parser.advance(&mut term, b"abcdef");
        tracker.observe(&term, "10:00:00.003".to_string());
        let visible = tracker.visible(&term);
        assert_eq!(visible[0], Some("10:00:00.003".to_string()));
        assert_eq!(visible[1], None);

        parser.advance(&mut term, b"\x1b[?1049h\x1b[2Jtui");
        assert!(term.mode().contains(TermMode::ALT_SCREEN));
        assert!(
            tracker
                .visible(&term)
                .into_iter()
                .all(|stamp| stamp.is_none())
        );

        parser.advance(&mut term, b"\x1b[?1049l");
        assert!(!term.mode().contains(TermMode::ALT_SCREEN));
        assert_eq!(tracker.visible(&term)[0], Some("10:00:00.003".to_string()));
    }

    #[test]
    fn timestamp_tracker_detects_capped_scrollback_shift() {
        let signature = |value: &str| {
            let mut hasher = DefaultHasher::new();
            value.hash(&mut hasher);
            RowSignature {
                hash: hasher.finish(),
                has_content: true,
                text: value.to_string(),
                wraps_to_next: false,
            }
        };
        let old = [
            signature("one"),
            signature("two"),
            signature("three"),
            signature("four"),
        ];
        let new = [
            signature("two"),
            signature("three"),
            signature("four"),
            signature("five"),
        ];
        assert_eq!(detect_scroll_shift(&old, &new), Some(1));
    }

    #[test]
    fn timestamp_tracker_preserves_rows_after_resize_reflow() {
        let config = Config {
            scrolling_history: SCROLLBACK,
            ..Default::default()
        };
        let mut term: Term<NoopListener> = Term::new(
            config,
            &TermSize { cols: 5, rows: 4 },
            NoopListener::default(),
        );
        let mut parser: Processor = Processor::new();
        let mut tracker = TerminalTimestampState::default();

        parser.advance(&mut term, b"abcdefghij");
        tracker.observe(&term, "10:00:00.004".to_string());
        parser.advance(&mut term, b"\r\nnext");
        tracker.observe(&term, "10:00:00.005".to_string());

        term.resize(TermSize { cols: 20, rows: 6 });
        tracker.sync_to_term(&term);
        let visible = tracker.visible(&term);

        assert!(
            visible
                .iter()
                .any(|timestamp| timestamp.as_deref() == Some("10:00:00.004"))
        );
        assert!(
            visible
                .iter()
                .any(|timestamp| timestamp.as_deref() == Some("10:00:00.005"))
        );

        term.resize(TermSize { cols: 5, rows: 4 });
        tracker.sync_to_term(&term);
        let visible = tracker.visible(&term);
        assert!(
            visible
                .iter()
                .any(|timestamp| timestamp.as_deref() == Some("10:00:00.004"))
        );
        assert!(
            visible
                .iter()
                .any(|timestamp| timestamp.as_deref() == Some("10:00:00.005"))
        );
    }

    #[test]
    fn encodes_navigation_keys_for_terminal_modes() {
        assert_eq!(encode_keystroke(&keystroke("up")), Some(b"\x1b[A".to_vec()));
        assert_eq!(
            encode_keystroke_with_mode(&keystroke("up"), TermMode::APP_CURSOR),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            encode_keystroke(&keystroke("shift-left")),
            Some(b"\x1b[1;2D".to_vec())
        );
        assert_eq!(
            encode_keystroke(&keystroke("ctrl-right")),
            Some(b"\x1b[1;5C".to_vec())
        );
        assert_eq!(
            encode_keystroke(&keystroke("shift-tab")),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(
            encode_keystroke(&keystroke("f12")),
            Some(b"\x1b[24~".to_vec())
        );
        assert_eq!(
            encode_keystroke(&keystroke("ctrl-f12")),
            Some(b"\x1b[24;5~".to_vec())
        );
    }

    #[test]
    fn scroll_wheel_routes_to_the_expected_terminal_layer() {
        assert_eq!(
            wheel_route(TermMode::empty(), false),
            WheelRoute::LocalScrollback
        );
        assert_eq!(
            wheel_route(TermMode::MOUSE_DRAG, false),
            WheelRoute::MouseReport
        );
        assert_eq!(
            wheel_route(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL, false),
            WheelRoute::AlternateScroll
        );
        assert_eq!(
            wheel_route(
                TermMode::MOUSE_DRAG | TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL,
                true,
            ),
            WheelRoute::LocalScrollback
        );
    }

    #[test]
    fn trackpad_scroll_waits_for_a_complete_terminal_line() {
        let mut scroll_acc = 0.;
        let pixels = |y| ScrollDelta::Pixels(Point::new(px(0.), px(y)));
        let lines = |y| ScrollDelta::Lines(Point::new(0., y));

        assert_eq!(
            wheel_lines_for_phase(TouchPhase::Started, pixels(5.), &mut scroll_acc, 10.),
            None
        );
        assert_eq!(
            wheel_lines_for_phase(TouchPhase::Moved, pixels(5.), &mut scroll_acc, 10.),
            None
        );
        assert_eq!(
            wheel_lines_for_phase(TouchPhase::Moved, pixels(5.), &mut scroll_acc, 10.),
            Some(1)
        );
        assert_eq!(
            wheel_lines_for_phase(TouchPhase::Cancelled, pixels(5.), &mut scroll_acc, 10.),
            None
        );
        assert_eq!(
            wheel_lines_for_phase(TouchPhase::Moved, lines(0.5), &mut scroll_acc, 10.),
            None
        );
        assert_eq!(
            wheel_lines_for_phase(TouchPhase::Moved, lines(0.5), &mut scroll_acc, 10.),
            Some(1)
        );
    }

    #[test]
    fn alternate_scroll_follows_application_cursor_mode() {
        assert_eq!(
            alternate_scroll_sequence(TermMode::empty(), b'A'),
            [0x1b, b'[', b'A']
        );
        assert_eq!(
            alternate_scroll_sequence(TermMode::APP_CURSOR, b'A'),
            [0x1b, b'O', b'A']
        );
    }

    #[test]
    fn encodes_control_and_printable_keys() {
        assert_eq!(encode_keystroke(&keystroke("a")), Some(b"a".to_vec()));
        assert_eq!(encode_keystroke(&keystroke("1")), Some(b"1".to_vec()));
        assert_eq!(encode_keystroke(&keystroke("-")), Some(b"-".to_vec()));
        assert_eq!(
            encode_keystroke(&keystroke("é")),
            Some("é".as_bytes().to_vec())
        );
        assert_eq!(
            encode_keystroke_with_mode(&keystroke("a"), TermMode::DISAMBIGUATE_ESC_CODES),
            Some(b"a".to_vec())
        );
        assert_eq!(encode_keystroke(&keystroke("space")), Some(b" ".to_vec()));
        assert_eq!(encode_keystroke(&keystroke("ctrl-c")), Some(vec![3]));
        assert_eq!(encode_keystroke(&keystroke("ctrl-@")), Some(vec![0]));
        assert_eq!(
            encode_keystroke(&keystroke("alt-x")),
            Some(b"\x1bx".to_vec())
        );
        assert_eq!(encode_keystroke(&keystroke("shift-a")), Some(b"A".to_vec()));
        assert_eq!(encode_keystroke(&keystroke("shift-1")), Some(b"!".to_vec()));
    }

    #[test]
    fn kitty_raw_images_are_normalized_to_png() {
        let rgba = kitty_raw_to_png(&[255, 0, 0, 255], 1, 1, 4).expect("RGBA PNG");
        assert_eq!(terminal_image_format(&rgba), Some(gpui::ImageFormat::Png));
        let rgb = kitty_raw_to_png(&[0, 255, 0], 1, 1, 3).expect("RGB PNG");
        assert_eq!(terminal_image_format(&rgb), Some(gpui::ImageFormat::Png));
        assert!(kitty_raw_to_png(&[0, 0, 0], 2, 1, 3).is_none());
    }

    #[test]
    fn kitty_zlib_images_are_bounded_and_decoded() {
        use std::io::Write;

        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(b"kitty image data").unwrap();
        let compressed = encoder.finish().unwrap();
        assert_eq!(
            kitty_zlib_decode(&compressed),
            Some(b"kitty image data".to_vec())
        );
        assert!(kitty_zlib_decode(b"not zlib").is_none());
    }

    #[test]
    fn kitty_graphics_chunks_keep_the_first_action() {
        assert_eq!(kitty_image_action("a=t,m=1", None), "t");
        assert_eq!(kitty_image_action("m=0", Some("t")), "t");
        assert_eq!(kitty_image_action("m=0", None), "t");
    }

    #[test]
    fn kitty_placeholders_decode_ids_coordinates_and_inheritance() {
        assert_eq!(kitty_placeholder_diacritic_value('\u{0305}'), Some(0));
        assert_eq!(kitty_placeholder_diacritic_value('\u{030d}'), Some(1));
        assert_eq!(kitty_placeholder_diacritic_value('\u{030e}'), Some(2));
        assert_eq!(kitty_placeholder_diacritic_value('\u{0300}'), None);

        let mut first = Cell {
            c: KITTY_PLACEHOLDER_CHAR,
            fg: Color::Indexed(42),
            ..Cell::default()
        };
        first.set_underline_color(Some(Color::Indexed(7)));
        let (placeholder, state) = decode_kitty_placeholder(&first, "\u{0305}\u{0305}", 4, 8, None)
            .expect("first placeholder");
        assert_eq!(placeholder.image_id, 42);
        assert_eq!(placeholder.placement_id, Some(7));
        assert_eq!((placeholder.row, placeholder.column), (0, 0));

        let mut second = Cell {
            c: KITTY_PLACEHOLDER_CHAR,
            fg: Color::Indexed(42),
            ..Cell::default()
        };
        second.set_underline_color(Some(Color::Indexed(7)));
        let (placeholder, _) = decode_kitty_placeholder(&second, "", 4, 9, Some(state))
            .expect("inherited placeholder");
        assert_eq!((placeholder.row, placeholder.column), (0, 1));
        assert_eq!(placeholder.placement_id, Some(7));

        let high_byte = Cell {
            c: KITTY_PLACEHOLDER_CHAR,
            fg: Color::Indexed(42),
            ..Cell::default()
        };
        let (placeholder, _) =
            decode_kitty_placeholder(&high_byte, "\u{0305}\u{0305}\u{030e}", 4, 10, None)
                .expect("high image id byte");
        assert_eq!(placeholder.image_id, 42 | (2 << 24));
    }

    #[test]
    fn terminal_snapshot_extracts_kitty_placeholders_from_the_grid() {
        let config = Config {
            scrolling_history: SCROLLBACK,
            ..Default::default()
        };
        let mut term: Term<NoopListener> = Term::new(
            config,
            &TermSize { cols: 4, rows: 2 },
            NoopListener::default(),
        );
        let mut parser: Processor = Processor::new();
        let bytes = format!(
            "\x1b[38;5;42m{p}{r0}{c0}{p}{r0}{c1}\r\n{p}{r1}{c0}\x1b[0m",
            p = KITTY_PLACEHOLDER_CHAR,
            r0 = '\u{0305}',
            r1 = '\u{030d}',
            c0 = '\u{0305}',
            c1 = '\u{030d}',
        );
        parser.advance(&mut term, bytes.as_bytes());

        let snapshot = snapshot_visible(&term, None, 4, false, &[]);
        assert_eq!(snapshot.kitty_placeholders.len(), 3);
        assert_eq!(
            snapshot
                .kitty_placeholders
                .iter()
                .map(|placeholder| (placeholder.row, placeholder.column))
                .collect::<Vec<_>>(),
            vec![(0, 0), (0, 1), (1, 0)]
        );
        assert!(snapshot.rows[0][0].kitty_placeholder);
        assert!(
            terminal_text_runs(&snapshot.rows[0])
                .iter()
                .all(|run| run.start_col >= 2)
        );
    }

    #[test]
    fn encodes_kitty_keyboard_modes() {
        let disambiguate = TermMode::DISAMBIGUATE_ESC_CODES;
        assert_eq!(
            encode_keystroke_with_mode(&keystroke("ctrl-c"), disambiguate),
            Some(b"\x1b[99;5u".to_vec())
        );
        assert_eq!(
            encode_keystroke_with_mode(&keystroke("alt-x"), disambiguate),
            Some(b"\x1b[120;3u".to_vec())
        );
        assert_eq!(
            encode_keystroke_with_mode(&keystroke("escape"), disambiguate),
            Some(b"\x1b[27u".to_vec())
        );
        assert_eq!(
            encode_keystroke_with_mode(&keystroke("enter"), disambiguate),
            Some(b"\r".to_vec())
        );

        let report_all = TermMode::REPORT_ALL_KEYS_AS_ESC;
        assert_eq!(
            encode_keystroke_with_mode(&keystroke("a"), report_all),
            Some(b"\x1b[97u".to_vec())
        );
        assert_eq!(
            encode_keystroke_with_mode(&keystroke("up"), report_all),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode_keystroke_with_mode(&keystroke("ctrl-up"), report_all),
            Some(b"\x1b[1;5A".to_vec())
        );
        assert_eq!(
            encode_keystroke_with_mode(&keystroke("f13"), report_all),
            Some(b"\x1b[57376u".to_vec())
        );

        let report_events = report_all | TermMode::REPORT_EVENT_TYPES;
        assert_eq!(
            encode_keystroke_with_event(&keystroke("a"), report_events, 2),
            Some(b"\x1b[97;1:2u".to_vec())
        );
        assert_eq!(
            encode_keystroke_with_event(&keystroke("a"), report_events, 3),
            Some(b"\x1b[97;1:3u".to_vec())
        );

        assert_eq!(
            encode_modify_other_keys(&keystroke("ctrl-;"), 2),
            Some(b"\x1b[27;5;59~".to_vec())
        );
        assert_eq!(encode_modify_other_keys(&keystroke("ctrl-c"), 1), None);
        assert_eq!(
            encode_modify_other_keys(&keystroke("a"), 3),
            Some(b"\x1b[27;1;97~".to_vec())
        );

        let disambiguate_events =
            disambiguate | TermMode::REPORT_EVENT_TYPES | TermMode::REPORT_ALTERNATE_KEYS;
        assert_eq!(
            encode_kitty_keystroke(&keystroke("a"), disambiguate_events, 3),
            None
        );
        assert_eq!(
            encode_kitty_keystroke(&keystroke("backspace"), disambiguate_events, 3),
            None
        );
        assert_eq!(
            encode_kitty_keystroke(&keystroke("up"), disambiguate_events, 3),
            Some(b"\x1b[1;1:3A".to_vec())
        );

        let enhanced = report_all
            | TermMode::REPORT_EVENT_TYPES
            | TermMode::REPORT_ALTERNATE_KEYS
            | TermMode::REPORT_ASSOCIATED_TEXT;
        assert_eq!(
            encode_keystroke_with_mode(&keystroke("shift-a"), enhanced),
            Some(b"\x1b[97:65;2:1;65u".to_vec())
        );
    }

    #[test]
    fn encodes_mouse_protocols_and_buttons() {
        let plain_mode = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(mouse_button_code(MouseButton::Left), Some(0));
        assert_eq!(mouse_button_code(MouseButton::Middle), Some(1));
        assert_eq!(mouse_button_code(MouseButton::Right), Some(2));
        assert_eq!(
            encode_mouse_report(0, 0, 0, true, &Modifiers::default(), plain_mode, false),
            Some(vec![0x1b, b'[', b'M', 32, 33, 33])
        );

        let sgr_mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let modifiers = Modifiers {
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            encode_mouse_report(2, 7, 4, true, &modifiers, sgr_mode, false),
            Some(b"\x1b[<6;8;5M".to_vec())
        );
        assert_eq!(
            encode_mouse_report(2, 7, 4, false, &modifiers, sgr_mode, false),
            Some(b"\x1b[<7;8;5m".to_vec())
        );

        let utf8_mode = TermMode::MOUSE_REPORT_CLICK | TermMode::UTF8_MOUSE;
        assert_eq!(
            encode_mouse_report(0, 95, 0, true, &Modifiers::default(), utf8_mode, false),
            Some(vec![0x1b, b'[', b'M', 32, 0xc2, 0x80, 33])
        );
    }

    #[test]
    fn terminal_parser_tracks_application_and_mouse_modes() {
        let config = Config {
            scrolling_history: SCROLLBACK,
            kitty_keyboard: true,
            ..Default::default()
        };
        let mut term: Term<NoopListener> = Term::new(
            config,
            &TermSize { cols: 80, rows: 24 },
            NoopListener::default(),
        );
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, b"\x1b[?1h\x1b[?1002h\x1b[?1006h");

        let mode = *term.mode();
        assert!(mode.contains(TermMode::APP_CURSOR));
        assert!(mode.contains(TermMode::MOUSE_DRAG));
        assert!(mode.contains(TermMode::SGR_MOUSE));
    }

    #[test]
    fn terminal_listener_forwards_terminal_responses() {
        let (listener, _, _, responses) = NoopListener::for_bridge(80, 24);
        listener.send_event(Event::PtyWrite("\x1b[0n".to_string()));

        assert_eq!(
            take_protocol_responses(&responses),
            vec![b"\x1b[0n".to_vec()]
        );
    }

    #[test]
    fn terminal_listener_buffers_ui_side_effects() {
        let (listener, _, side_effects, _) = NoopListener::for_bridge(80, 24);
        listener.send_event(Event::Title("OpenCode".to_string()));

        let mut effects = side_effects.lock().expect("side effect queue");
        assert!(matches!(
            effects.pop_front(),
            Some(TerminalSideEffect::Title(title)) if title == "OpenCode"
        ));
    }

    #[test]
    fn protocol_responses_survive_a_saturated_input_queue_in_order() {
        let (listener, _, _, responses) = NoopListener::for_bridge(80, 24);
        listener.send_event(Event::PtyWrite("one".to_string()));
        listener.send_event(Event::PtyWrite("two".to_string()));
        listener.send_event(Event::PtyWrite("three".to_string()));

        let (input_tx, input_rx) = async_channel::bounded(1);
        input_tx
            .try_send(InputCmd::Write(b"user".to_vec()))
            .expect("fill input queue");
        let mut pending = VecDeque::new();
        pending.extend(
            take_protocol_responses(&responses)
                .into_iter()
                .map(InputCmd::Write),
        );

        while let Some(command) = pending.pop_front() {
            match input_tx.try_send(command) {
                Ok(()) => {}
                Err(TrySendError::Full(command)) => {
                    pending.push_front(command);
                    break;
                }
                Err(TrySendError::Closed(_)) => panic!("input queue unexpectedly closed"),
            }
        }

        assert!(matches!(
            input_rx.try_recv().expect("user input"),
            InputCmd::Write(bytes) if bytes == b"user"
        ));
        let mut observed = Vec::new();
        while let Some(command) = pending.pop_front() {
            input_tx.try_send(command).expect("drain response queue");
            let command = input_rx.try_recv().expect("queued response");
            match command {
                InputCmd::Write(bytes) => observed.push(bytes),
                InputCmd::Resize { .. } | InputCmd::Close => panic!("unexpected command"),
            }
        }
        assert_eq!(
            observed,
            vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]
        );
    }

    #[test]
    fn protocol_responses_are_flushed_before_close() {
        let (listener, _, _, responses) = NoopListener::for_bridge(80, 24);
        listener.send_event(Event::PtyWrite("reply".to_string()));

        let (input_tx, input_rx) = async_channel::bounded(1);
        input_tx
            .try_send(InputCmd::Write(b"user".to_vec()))
            .expect("fill input queue");
        let mut pending = VecDeque::new();
        for response in take_protocol_responses(&responses) {
            queue_input_nonblocking(&input_tx, &mut pending, InputCmd::Write(response));
        }
        queue_input_nonblocking(&input_tx, &mut pending, InputCmd::Close);

        assert!(matches!(
            input_rx.try_recv().expect("user input"),
            InputCmd::Write(bytes) if bytes == b"user"
        ));
        flush_pending_commands(&input_tx, &mut pending);
        assert!(matches!(
            input_rx.try_recv().expect("protocol response"),
            InputCmd::Write(bytes) if bytes == b"reply"
        ));
        flush_pending_commands(&input_tx, &mut pending);
        assert!(matches!(
            input_rx.try_recv().expect("close"),
            InputCmd::Close
        ));
        assert!(pending.is_empty());
    }

    #[test]
    fn osc52_policy_denies_remote_reads_and_rejects_oversized_payloads() {
        assert!(!osc52_load_allowed(false));
        assert!(osc52_load_allowed(true));
        assert_eq!(osc52_mode(false), Osc52::OnlyCopy);
        assert_eq!(osc52_mode(true), Osc52::CopyPaste);
        assert!(osc52_text_within_limit("safe"));
        let oversized = "x".repeat(MAX_OSC52_CLIPBOARD_BYTES + 1);
        assert!(!osc52_text_within_limit(&oversized));

        let formatter: Arc<dyn Fn(&str) -> String + Sync + Send> =
            Arc::new(|_| "x".repeat(MAX_OSC52_RESPONSE_BYTES + 1));
        assert!(format_osc52_response(&formatter, "safe").is_none());
    }

    #[test]
    fn ime_cursor_position_accounts_for_scrollback_offset() {
        assert_eq!(cursor_viewport_position(2, 9, 0, 24, 80), Some((9, 2)));
        assert_eq!(cursor_viewport_position(0, 99, 3, 24, 80), Some((79, 3)));
        assert_eq!(cursor_viewport_position(-4, 0, 3, 24, 80), None);
        assert_eq!(cursor_viewport_position(0, 0, 0, 0, 80), None);
    }

    #[test]
    fn ime_marked_text_length_uses_utf16_units() {
        let text = "中😀文";
        assert_eq!(utf16_len(text), 4);
        assert_eq!(utf16_len("中文"), 2);
    }

    #[test]
    fn terminal_render_keeps_wide_characters_on_their_grid_columns() {
        let config = Config {
            scrolling_history: SCROLLBACK,
            ..Default::default()
        };
        let mut term: Term<NoopListener> = Term::new(
            config,
            &TermSize { cols: 8, rows: 2 },
            NoopListener::default(),
        );
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, "a中b".as_bytes());

        let snapshot = snapshot_visible(&term, None, 8, true, &[]);
        assert!(snapshot.rows[0][1].wide);
        assert!(snapshot.rows[0][2].spacer);
        assert_eq!(cursor_visual_span(&snapshot.rows[0], 1), (1, 2, 1));
        assert_eq!(cursor_visual_span(&snapshot.rows[0], 2), (1, 2, 1));

        let runs = terminal_text_runs(&snapshot.rows[0][..4]);
        let positions: Vec<_> = runs
            .iter()
            .map(|run| {
                (
                    run.start_col,
                    run.cell_count,
                    run.force_width_cells,
                    run.text.clone(),
                )
            })
            .collect();
        assert_eq!(
            positions,
            vec![
                (0, 1, 1, "a".to_string()),
                (1, 2, 2, "中".to_string()),
                (3, 1, 1, "b".to_string()),
            ]
        );
    }

    #[test]
    fn terminal_snapshot_applies_inverse_after_palette_lookup() {
        let config = Config {
            scrolling_history: SCROLLBACK,
            ..Default::default()
        };
        let mut term: Term<NoopListener> = Term::new(
            config,
            &TermSize { cols: 4, rows: 1 },
            NoopListener::default(),
        );
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, b"\x1b[31;47mA\x1b[7mB\x1b[27mC");

        let snapshot = snapshot_visible(&term, None, 4, false, &[]);
        let red = default_palette(&NamedColor::Red);
        let white = default_palette(&NamedColor::White);

        assert_eq!(snapshot.rows[0][0].fg, red);
        assert_eq!(snapshot.rows[0][0].bg, white);
        assert_eq!(snapshot.rows[0][1].fg, white);
        assert_eq!(snapshot.rows[0][1].bg, red);
        assert_eq!(snapshot.rows[0][2].fg, red);
        assert_eq!(snapshot.rows[0][2].bg, white);
    }

    #[test]
    fn terminal_snapshot_preserves_text_decorations_and_hidden_text() {
        let config = Config {
            scrolling_history: SCROLLBACK,
            ..Default::default()
        };
        let mut term: Term<NoopListener> = Term::new(
            config,
            &TermSize { cols: 3, rows: 1 },
            NoopListener::default(),
        );
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, b"\x1b[4;9mX\x1b[0m\x1b[8mY");

        let snapshot = snapshot_visible(&term, None, 3, false, &[]);
        assert_eq!(snapshot.rows[0][0].underline, UnderlineKind::Solid);
        assert!(snapshot.rows[0][0].strikeout);
        assert_eq!(snapshot.rows[0][1].fg, snapshot.rows[0][1].bg);
    }

    #[test]
    fn terminal_snapshot_maps_plain_urls_to_cell_columns() {
        let config = Config {
            scrolling_history: SCROLLBACK,
            ..Default::default()
        };
        let mut term: Term<NoopListener> = Term::new(
            config,
            &TermSize { cols: 64, rows: 1 },
            NoopListener::default(),
        );
        let mut parser: Processor = Processor::new();
        parser.advance(
            &mut term,
            "中文 www.first.test https://second.test".as_bytes(),
        );

        let snapshot = snapshot_visible(&term, None, 64, false, &[]);
        assert_eq!(
            snapshot.urls,
            vec![
                (0, 5, 19, "www.first.test".to_string()),
                (0, 20, 39, "https://second.test".to_string()),
            ]
        );
        assert!(snapshot.rows[0][5].is_url);
        assert!(snapshot.rows[0][18].is_url);
        assert!(!snapshot.rows[0][19].is_url);
        assert!(snapshot.rows[0][20].is_url);
        assert!(snapshot.rows[0][38].is_url);
        assert!(!snapshot.rows[0][39].is_url);
    }

    #[test]
    fn terminal_snapshot_preserves_osc8_hyperlink_targets() {
        let config = Config {
            scrolling_history: SCROLLBACK,
            ..Default::default()
        };
        let mut term: Term<NoopListener> = Term::new(
            config,
            &TermSize { cols: 8, rows: 1 },
            NoopListener::default(),
        );
        let mut parser: Processor = Processor::new();
        parser.advance(
            &mut term,
            b"\x1b]8;;https://example.com\x07Crossh\x1b]8;;\x07",
        );

        let snapshot = snapshot_visible(&term, None, 8, false, &[]);
        assert_eq!(
            snapshot.rows[0][0].hyperlink.as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            snapshot.urls.first(),
            Some(&(0, 0, 6, "https://example.com".to_string()))
        );
    }

    #[test]
    fn bold_basic_colors_use_bright_palette_entries() {
        let cell = Cell {
            fg: Color::Named(NamedColor::Red),
            ..Cell::default()
        };
        let style = effective_cell_style(
            &Cell {
                flags: CellFlags::BOLD,
                ..cell
            },
            &alacritty_terminal::term::color::Colors::default(),
            default_palette(&NamedColor::Foreground),
            default_palette(&NamedColor::Background),
        );
        assert_eq!(style.fg, default_palette(&NamedColor::BrightRed));
    }

    /// 隔离测试：把真实 shell 输出（含 OSC 标题 / 颜色 / bracketed-paste /
    /// \r\n 换行）喂给 `vte::ansi::Processor + alacritty Term`，验证 grid 里
    /// 是否真的写入了 `ls` 的结果。用于把「解析」与「渲染」两个环节分开定位。
    #[test]
    fn term_parses_real_shell_ls_output() {
        let config = Config {
            scrolling_history: SCROLLBACK,
            ..Default::default()
        };
        let size = TermSize { cols: 80, rows: 10 };
        let mut term: Term<NoopListener> = Term::new(config, &size, NoopListener::default());
        let mut parser: Processor = Processor::new();

        // 取自 connect_and_run_ls 诊断的真实字节流（提示符 + echo ls + 结果 + 新提示符）。
        let bytes: &[u8] = b"\x1b[?2004h\x1b]0;ubuntu@vps: ~\x07\x1b[01;32mubuntu@vps\x1b[00m:\x1b[01;34m~\x1b[00m$ ls\r\n\x1b[?2004l\r\x1b[0m\x1b[01;34mbackup\x1b[0m  \x1b[01;34mcard\x1b[0m  \x1b[01;34medunest\x1b[0m\r\n\x1b[?2004h\x1b]0;ubuntu@vps: ~\x07\x1b[01;32mubuntu@vps\x1b[00m:\x1b[01;34m~\x1b[00m$ ";

        parser.advance(&mut term, bytes);

        // 打印整个屏幕 + scrollback 顶部若干行，便于诊断。
        let grid = term.grid();
        let cols = grid.columns();
        let screen = grid.screen_lines();
        println!("=== screen {}x{} ===", cols, screen);
        let mut screen_text = String::new();
        for r in 0..screen {
            let row = &grid[Line(r as i32)];
            let s: String = (0..cols).map(|c| row[Column(c)].c).collect();
            let t = s.trim_end();
            if !t.is_empty() {
                println!("row {:2}: {:?}", r, t);
            }
            screen_text.push_str(&s);
        }

        assert!(screen_text.contains("backup"), "grid missing 'backup'");
        assert!(screen_text.contains("card"), "grid missing 'card'");
    }

    /// 关键测试：模拟 GUI 的 maybe_resize 流程 —— 先解析输出，再 resize term，
    /// 然后用 snapshot_visible 的逻辑（display_offset 决定可见区）检查 ls 结果
    /// 是否还在可见区。用于定位「resize 后内容消失」类问题。
    #[test]
    fn term_resize_keeps_ls_visible() {
        let config = Config {
            scrolling_history: SCROLLBACK,
            ..Default::default()
        };
        let mut term: Term<NoopListener> = Term::new(
            config,
            &TermSize { cols: 80, rows: 10 },
            NoopListener::default(),
        );
        let mut parser: Processor = Processor::new();

        let bytes: &[u8] = b"\x1b[?2004h\x1b]0;ubuntu@vps: ~\x07\x1b[01;32mubuntu@vps\x1b[00m:\x1b[01;34m~\x1b[00m$ ls\r\n\x1b[?2004l\r\x1b[0m\x1b[01;34mbackup\x1b[0m  \x1b[01;34mcard\x1b[0m\r\n\x1b[?2004h\x1b]0;ubuntu@vps: ~\x07\x1b[01;32mubuntu@vps\x1b[00m:\x1b[01;34m~\x1b[00m$ ";
        parser.advance(&mut term, bytes);

        let grid = term.grid();
        println!(
            "before resize: display_offset={} total={} screen={}",
            grid.display_offset(),
            grid.total_lines(),
            grid.screen_lines()
        );

        // 模拟 maybe_resize：80x10 -> 100x30。
        term.resize(TermSize {
            cols: 100,
            rows: 30,
        });

        let grid = term.grid();
        println!(
            "after resize: display_offset={} total={} screen={}",
            grid.display_offset(),
            grid.total_lines(),
            grid.screen_lines()
        );

        // 用 snapshot_visible 的逻辑读取可见区。
        let display_offset = grid.display_offset();
        let cols = grid.columns();
        let rows = grid.screen_lines();
        let top_visible = Line(-(display_offset as i32));
        let mut all = String::new();
        for r in 0..rows {
            let line = Line(top_visible.0 + r as i32);
            let row = &grid[line];
            let s: String = (0..cols).map(|c| row[Column(c)].c).collect();
            let t = s.trim_end();
            if !t.is_empty() {
                println!("visible row {:2}: {:?}", r, t);
            }
            all.push_str(&s);
        }

        assert!(
            all.contains("backup"),
            "after resize, visible area missing 'backup'"
        );
        assert!(
            all.contains("card"),
            "after resize, visible area missing 'card'"
        );
    }

    /// 验证把字节流切成极小 chunk（模拟 drain 分批 advance）后，
    /// parser 仍能把 ls 结果正确写入 grid（跨 chunk 的 OSC/CSI 不断）。
    #[test]
    fn term_parses_chunked_output() {
        let config = Config {
            scrolling_history: SCROLLBACK,
            ..Default::default()
        };
        let mut term: Term<NoopListener> = Term::new(
            config,
            &TermSize { cols: 80, rows: 10 },
            NoopListener::default(),
        );
        let mut parser: Processor = Processor::new();

        let bytes: &[u8] = b"\x1b[?2004h\x1b]0;ubuntu@vps: ~\x07\x1b[01;32mubuntu@vps\x1b[00m:\x1b[01;34m~\x1b[00m$ ls\r\n\x1b[?2004l\r\x1b[0m\x1b[01;34mbackup\x1b[0m  \x1b[01;34mcard\x1b[0m\r\n\x1b[?2004h\x1b]0;ubuntu@vps: ~\x07\x1b[01;32mubuntu@vps\x1b[00m:\x1b[01;34m~\x1b[00m$ ";

        // 最严格：每字节一个 chunk。
        for chunk in bytes.chunks(1) {
            parser.advance(&mut term, chunk);
        }

        let grid = term.grid();
        let cols = grid.columns();
        let screen = grid.screen_lines();
        let mut screen_text = String::new();
        for r in 0..screen {
            let row = &grid[Line(r as i32)];
            let s: String = (0..cols).map(|c| row[Column(c)].c).collect();
            let t = s.trim_end();
            if !t.is_empty() {
                println!("chunked row {:2}: {:?}", r, t);
            }
            screen_text.push_str(&s);
        }

        assert!(screen_text.contains("backup"), "chunked: missing backup");
        assert!(screen_text.contains("card"), "chunked: missing card");
    }
}

//! 终端视图：gpui Entity，持有 Zed 的 terminal core，自研 Canvas 渲染器。
//!
//! 数据流：
//!  - 远端 → russh 读循环 → SessionEvent::Output → 本 Entity 的 drain 任务
//!    → `terminal.write_output(bytes)` → cx.notify() 触发重绘。
//!  - 键盘 → on_key_down → 编码为字节 → input_tx → russh 写循环。
//!
//! Zed terminal core 只在 gpui 主线程被触碰（drain 与 paint 都在主线程）。
use std::cell::Cell as StdCell;
use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alacritty_terminal::event::WindowSize;
#[cfg(test)]
use alacritty_terminal::event::{Event, EventListener};
#[cfg(test)]
use alacritty_terminal::term::ClipboardType;
use alacritty_terminal::term::TermMode;
#[cfg(test)]
use alacritty_terminal::term::cell::Flags as CellFlags;
#[cfg(test)]
use alacritty_terminal::term::{Config, Term};
use async_channel::{Receiver, Sender};
use chrono::Local;
use gpui::{
    App, AppContext, Bounds, Context, Entity, EntityInputHandler, EventEmitter, FocusHandle, Font,
    FontWeight, InputHandler, InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point,
    Render, ScrollDelta, ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled,
    Subscription, SystemNotificationAction, SystemNotificationResponse, Task, TextRun, TouchPhase,
    UTF16Selection, Window, canvas, div, hsla, px,
};
use terminal as zed_terminal;
#[cfg(test)]
use vte::ansi::{Processor, Rgb};

#[cfg(test)]
use alacritty_terminal::term::Osc52;
#[cfg(test)]
use alacritty_terminal::term::cell::Cell;
#[cfg(test)]
use gpui::Modifiers;
#[cfg(test)]
use std::collections::hash_map::DefaultHasher;
#[cfg(test)]
use std::hash::{Hash, Hasher};
#[cfg(test)]
use vte::ansi::{Color, NamedColor};

use crate::shared::i18n;
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
use super::image::*;
use super::input::{ShellInputBuffer, flush_pending_commands, queue_input_nonblocking};
use super::paint::*;
use super::render::*;
use super::settings::TerminalSettings;

#[path = "view_input.rs"]
mod view_input;
#[path = "view_state.rs"]
mod view_state;
use crate::features::workspace::pane::{PaneRisk, TerminalPaneInfo, WorkspacePane};
use view_input::TerminalInputHandler;
#[cfg(test)]
use view_input::selection_column_bounds;

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
pub(crate) const TIMESTAMP_GUTTER_WIDTH: f32 = 104.0;
/// 时间戳文本与 gutter 两侧的间距。
pub(crate) const TIMESTAMP_GUTTER_PADDING: f32 = 8.0;
/// gutter 分隔线与终端第 0 列之间的视觉留白。
pub(crate) const TIMESTAMP_GUTTER_GAP: f32 = 8.0;
/// OSC 52 单次剪贴板文本上限。
const MAX_TERMINAL_IMAGES: usize = 64;
const MAX_PENDING_KITTY_NOTIFICATIONS: usize = 128;
pub(crate) const MAX_KITTY_IMAGE_BYTES: usize = 6 * 1024 * 1024;
pub(crate) const MAX_DECODED_IMAGE_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_IMAGE_DIMENSION: usize = 16 * 1024;
pub(crate) const KITTY_BACKGROUND_Z_INDEX: i32 = -1_073_741_824;
pub(crate) const KITTY_PLACEHOLDER_CHAR: char = '\u{10eeee}';

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

/// Compatibility events used by the legacy terminal unit tests.
#[cfg(test)]
#[allow(dead_code)]
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

/// Translate the public mode subset exposed by Zed's terminal core into the
/// mode type used by Crossh's input encoder.
pub(crate) fn alacritty_mode(content: &zed_terminal::Content) -> TermMode {
    let mut mode = TermMode::empty();
    let zed_mode = content.mode;
    let mappings = [
        (zed_terminal::Modes::APP_CURSOR, TermMode::APP_CURSOR),
        (zed_terminal::Modes::APP_KEYPAD, TermMode::APP_KEYPAD),
        (zed_terminal::Modes::SHOW_CURSOR, TermMode::SHOW_CURSOR),
        (zed_terminal::Modes::LINE_WRAP, TermMode::LINE_WRAP),
        (zed_terminal::Modes::ORIGIN, TermMode::ORIGIN),
        (zed_terminal::Modes::INSERT, TermMode::INSERT),
        (
            zed_terminal::Modes::LINE_FEED_NEW_LINE,
            TermMode::LINE_FEED_NEW_LINE,
        ),
        (zed_terminal::Modes::FOCUS_IN_OUT, TermMode::FOCUS_IN_OUT),
        (
            zed_terminal::Modes::ALTERNATE_SCROLL,
            TermMode::ALTERNATE_SCROLL,
        ),
        (
            zed_terminal::Modes::BRACKETED_PASTE,
            TermMode::BRACKETED_PASTE,
        ),
        (zed_terminal::Modes::SGR_MOUSE, TermMode::SGR_MOUSE),
        (zed_terminal::Modes::UTF8_MOUSE, TermMode::UTF8_MOUSE),
        (zed_terminal::Modes::ALT_SCREEN, TermMode::ALT_SCREEN),
        (
            zed_terminal::Modes::MOUSE_REPORT_CLICK,
            TermMode::MOUSE_REPORT_CLICK,
        ),
        (zed_terminal::Modes::MOUSE_DRAG, TermMode::MOUSE_DRAG),
        (zed_terminal::Modes::MOUSE_MOTION, TermMode::MOUSE_MOTION),
        (zed_terminal::Modes::VI, TermMode::VI),
    ];
    for (zed_flag, alacritty_flag) in mappings {
        if zed_mode.contains(zed_flag) {
            mode.insert(alacritty_flag);
        }
    }
    mode
}

pub(crate) fn terminal_bounds_for_grid(
    cols: usize,
    rows: usize,
    cell_width: Pixels,
    line_height: Pixels,
) -> zed_terminal::TerminalBounds {
    zed_terminal::TerminalBounds::new(
        line_height.max(px(1.)),
        cell_width.max(px(1.)),
        Bounds {
            origin: Point::default(),
            size: gpui::size(
                cell_width.max(px(1.)) * cols as f32,
                line_height.max(px(1.)) * rows as f32,
            ),
        },
    )
}

impl TerminalView {
    pub(crate) fn terminal_mode(&self) -> TermMode {
        let mut mode = alacritty_mode(&self.terminal_content);
        mode.insert(TermMode::from_bits_retain(
            (self.kitty_keyboard_mode as u32) << 18,
        ));
        mode
    }
}

type WindowSizeHandle = Arc<Mutex<WindowSize>>;
#[cfg(test)]
type SideEffectQueue = Arc<Mutex<VecDeque<TerminalSideEffect>>>;
use super::input_encoding::*;

pub(crate) fn terminal_queues_for_bridge(
    cols: usize,
    rows: usize,
) -> (WindowSizeHandle, ProtocolResponseQueue) {
    let window_size = Arc::new(Mutex::new(WindowSize {
        num_lines: rows as u16,
        num_cols: cols as u16,
        cell_width: 8,
        cell_height: (FONT_SIZE * 1.3) as u16,
    }));
    (window_size, Arc::new(Mutex::new(VecDeque::new())))
}

/// Compatibility listener used by the legacy terminal unit tests.
#[derive(Clone)]
#[cfg(test)]
pub(crate) struct NoopListener {
    window_size: WindowSizeHandle,
    side_effects: SideEffectQueue,
    protocol_responses: ProtocolResponseQueue,
}

#[cfg(test)]
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

#[cfg(test)]
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
        let (window_size, protocol_responses) = terminal_queues_for_bridge(cols, rows);
        let side_effects = Arc::new(Mutex::new(VecDeque::new()));
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

#[cfg(test)]
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

/// 终端尺寸（用于测试旧的渲染辅助函数）。
#[cfg(test)]
struct TermSize {
    cols: usize,
    rows: usize,
}
#[cfg(test)]
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
pub(crate) struct TerminalImage {
    pub(crate) image: Arc<gpui::Image>,
    pub(crate) kitty_id: Option<u32>,
    pub(crate) placement_id: Option<u32>,
    /// Absolute line in the terminal grid: history size + screen-relative line.
    pub(crate) origin_line: i64,
    pub(crate) origin_col: usize,
    pub(crate) width: Option<ImageDimension>,
    pub(crate) height: Option<ImageDimension>,
    pub(crate) preserve_aspect_ratio: bool,
    pub(crate) offset_x: usize,
    pub(crate) offset_y: usize,
    pub(crate) z_index: i32,
    pub(crate) virtual_placement: bool,
    pub(crate) relative_image_id: Option<u32>,
    pub(crate) relative_placement_id: Option<u32>,
    pub(crate) relative_offset_x: i32,
    pub(crate) relative_offset_y: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct KittyPlacement {
    pub(crate) source_x: Option<usize>,
    pub(crate) source_y: Option<usize>,
    pub(crate) source_width: Option<usize>,
    pub(crate) source_height: Option<usize>,
    pub(crate) offset_x: usize,
    pub(crate) offset_y: usize,
    pub(crate) z_index: i32,
    pub(crate) relative_image_id: Option<u32>,
    pub(crate) relative_placement_id: Option<u32>,
    pub(crate) relative_offset_x: i32,
    pub(crate) relative_offset_y: i32,
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
pub(crate) struct KittyPlaceholder {
    pub(crate) image_id: u32,
    pub(crate) placement_id: Option<u32>,
    pub(crate) row: usize,
    pub(crate) column: usize,
    pub(crate) viewport_row: usize,
    pub(crate) viewport_column: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KittyPlaceholderState {
    pub(crate) foreground: u32,
    pub(crate) underline: Option<u32>,
    pub(crate) row: usize,
    pub(crate) column: usize,
    pub(crate) image_id_high: u8,
}

#[derive(Clone, Copy)]
pub(crate) struct TerminalProgress {
    pub(crate) state: u8,
    pub(crate) progress: Option<u8>,
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
    zed_terminal: Entity<zed_terminal::Terminal>,
    terminal_content: zed_terminal::Content,
    terminal_total_lines: usize,
    pending_terminal_output: Vec<u8>,
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
    /// Optional local shell line editor used to hide high-latency remote echo.
    low_latency_shell_input: bool,
    shell_input_buffer: ShellInputBuffer,
    remote_mouse_button: Option<u8>,
    /// 累积滚动偏移（trackpad 累加用）。
    scroll_acc: f32,
    /// 光标闪烁状态（true = 显示，false = 隐藏）。
    cursor_blink_on: bool,
    /// xterm's legacy CSI 1015 mouse mode is not modeled by alacritty yet.
    urxvt_mouse: bool,
    /// xterm modifyOtherKeys level (0 means the legacy encoding is active).
    modify_other_keys: u8,
    /// Kitty keyboard protocol state is not part of Zed's public `Modes` API,
    /// so Crossh tracks the small keyboard-mode side channel separately.
    kitty_keyboard_mode: u8,
    kitty_keyboard_stack: Vec<u8>,
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
    _blink_task: Option<Task<()>>,
    /// 最近一帧检测到的 URL（用于点击跳转）。
    detected_urls: Vec<(usize, usize, usize, String)>,
    /// 终端行的旁路时间戳；绝不写入 PTY 或 alacritty 的字符网格。
    line_timestamps: TerminalTimestampState,
    pending_timestamp: Option<String>,
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

pub(crate) struct TerminalWorkspacePane(pub(crate) Entity<TerminalView>);

pub(crate) fn workspace_pane(entity: Entity<TerminalView>) -> Box<dyn WorkspacePane> {
    Box::new(TerminalWorkspacePane(entity))
}

impl WorkspacePane for TerminalWorkspacePane {
    fn render(&self) -> gpui::AnyElement {
        self.0.clone().into_any_element()
    }

    fn title(&self, cx: &App) -> String {
        self.0.read(cx).tab_title("Terminal")
    }

    fn terminal_info(&self, cx: &App) -> Option<TerminalPaneInfo> {
        let terminal = self.0.read(cx);
        Some(TerminalPaneInfo {
            low_latency_enabled: terminal.low_latency_shell_input_enabled(),
            low_latency_available: terminal.low_latency_shell_input_available(),
        })
    }

    fn terminal_entity_id(&self) -> Option<gpui::EntityId> {
        Some(self.0.entity_id())
    }

    fn cwd(&self, cx: &App) -> Option<String> {
        self.0.read(cx).cwd.clone()
    }

    fn is_command_running(&self, cx: &App) -> bool {
        self.0.read(cx).is_command_running()
    }

    fn toggle_low_latency(&self, cx: &mut App) {
        self.0
            .update(cx, |terminal, _| terminal.toggle_low_latency_shell_input());
    }

    fn run_command(&self, command: &str, cx: &mut App) {
        self.0
            .update(cx, |terminal, _| terminal.run_command(command));
    }

    fn handle_system_notification_response(
        &self,
        response: &gpui::SystemNotificationResponse,
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
        self.0.update(cx, |terminal, _| terminal.request_close());
    }

    fn apply_terminal_settings(&self, settings: TerminalSettings, cx: &mut App) {
        self.0
            .update(cx, |terminal, cx| terminal.apply_settings(settings, cx));
    }

    fn notify_language(&self, _cx: &mut App) {}

    fn risk(&self, _cx: &App) -> PaneRisk {
        PaneRisk::default()
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
        if self.shell_input_active() && !text.chars().any(char::is_control) {
            self.insert_shell_input_text(text);
        } else {
            if self.shell_input_active() {
                self.flush_shell_input_buffer();
                self.low_latency_shell_input = false;
            }
            self.ime_marked_text.clear();
            if !text.is_empty() {
                self.send_input(text.as_bytes().to_vec());
            }
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
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.ime_cursor_bounds(element_bounds, window)
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
        self.zed_terminal.update(cx, |terminal, terminal_cx| {
            terminal.sync(window, terminal_cx);
        });
        let zed_terminal = self.zed_terminal.read(cx);
        self.terminal_content = zed_terminal.last_content().clone();
        self.terminal_total_lines = zed_terminal.total_lines();
        if let Some(timestamp) = self.pending_timestamp.take() {
            self.line_timestamps
                .observe_content(&self.terminal_content, timestamp);
        }

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
        let bg = bg_of_content(&self.terminal_content);
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
        let default_fg = fg_of_content(&self.terminal_content);

        let canvas_el = canvas(
            move |bounds, _window, cx| {
                // prepaint：可能 resize。
                if let Some(t) = weak.upgrade() {
                    t.update(cx, |this, _cx| {
                        this.anchor_bounds.set(Some(bounds));
                        let terminal_bounds = terminal_bounds_for(bounds, show_timestamps);
                        this.content_origin = terminal_bounds.origin;
                        this.maybe_resize(
                            Size {
                                w: terminal_bounds.size.width.as_f32(),
                                h: terminal_bounds.size.height.as_f32(),
                            },
                            _cx,
                        );
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
                    let (snapshot, ime_marked_text, shell_input, images, progress) = {
                        let this = t.read(cx);
                        let sel = this.sel_start.zip(this.sel_end);
                        let show_cur = this
                            .terminal_content
                            .mode
                            .contains(zed_terminal::Modes::SHOW_CURSOR)
                            && this.terminal_content.cursor.shape
                                != zed_terminal::CursorShape::Hidden
                            && this.cursor_blink_on;
                        let timestamps = if this.show_timestamps {
                            this.line_timestamps.visible_content(&this.terminal_content)
                        } else {
                            vec![None; this.terminal_content.terminal_bounds.num_lines()]
                        };
                        let shell_input = this.shell_input_active().then(|| ShellInputRender {
                            text: this.shell_input_buffer.text().to_owned(),
                            cursor: this.shell_input_buffer.cursor(),
                            ime_marked_text: this.ime_marked_text.clone(),
                        });
                        (
                            snapshot_visible_content(
                                &this.terminal_content,
                                sel,
                                show_cur,
                                &timestamps,
                                this.terminal_total_lines,
                            ),
                            if shell_input.is_some() {
                                String::new()
                            } else {
                                this.ime_marked_text.clone()
                            },
                            shell_input,
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
                            shell_input: shell_input.as_ref(),
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

// ─── 鼠标编码 ────────────────────────────────────────────────────────────────
// Mouse encoding helpers live in `input_encoding.rs`.
/// 临时尺寸结构（避免与 gpui::Size 命名冲突的内部用）。
struct Size {
    w: f32,
    h: f32,
}

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;

//! 终端视图：gpui Entity，持有 alacritty_terminal::Term，自研 Canvas 渲染器。
//!
//! 数据流：
//!  - 远端 → russh 读循环 → SessionEvent::Output → 本 Entity 的 drain 任务
//!    → `parser.advance(&mut term, bytes)` → cx.notify() 触发重绘。
//!  - 键盘 → on_key_down → 编码为字节 → input_tx → russh 写循环。
//!
//! Term 只在 gpui 主线程被触碰（drain 与 paint 都在主线程）。

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::{Config, Term};
use async_channel::{Receiver, Sender};
use gpui::{
    canvas, px, App, AppContext, Bounds, Context, Corners, Edges, Entity, FocusHandle, Font,
    FontWeight, Hsla, InteractiveElement, IntoElement, KeyDownEvent, Modifiers, ParentElement,
    Pixels, Point, Render, SharedString, StatefulInteractiveElement, Styled, TextAlign, Task,
    TextRun, Window, div, hsla, quad, rgb,
};
use vte::ansi::{Color, NamedColor, Processor, Rgb};

use crate::ssh::{InputCmd, SessionEvent};

/// 终端字体大小（像素）。
const FONT_SIZE: f32 = 14.0;
/// 滚动历史行数上限（控内存）。
const SCROLLBACK: usize = 1000;

/// 连接状态。
#[derive(Default, Clone, Debug)]
pub enum ConnState {
    #[default]
    Connecting,
    Connected,
    Error(String),
    Closed,
}

/// alacritty EventListener 的空实现（我们不处理终端事件回调）。
#[derive(Clone)]
struct NoopListener;
impl EventListener for NoopListener {
    fn send_event(&self, _event: Event) {}
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

pub struct TerminalView {
    term: Term<NoopListener>,
    parser: Processor,
    input_tx: Sender<InputCmd>,
    pub state: ConnState,
    focus: FocusHandle,
    // 字体度量（首次绘制时计算）。
    cell_w: Pixels,
    line_h: Pixels,
    cols: usize,
    rows: usize,
    font: Font,
    _drain: Option<Task<()>>,
    // 是否已自动聚焦过一次（打开/切回时需重新聚焦则置 false）。
    focused_once: bool,
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
        let config = Config {
            scrolling_history: SCROLLBACK,
            ..Default::default()
        };
        let size = TermSize { cols, rows };
        let font = Font {
            family: "Menlo".into(),
            weight: FontWeight::NORMAL,
            style: gpui::FontStyle::Normal,
            features: Default::default(),
            fallbacks: None,
        };

        let entity = cx.new(|cx| Self {
            term: Term::new(config, &size, NoopListener),
            parser: Processor::new(),
            input_tx: input_tx.clone(),
            state: ConnState::Connecting,
            focus: cx.focus_handle(),
            cell_w: px(0.),
            line_h: px(FONT_SIZE * 1.3),
            cols,
            rows,
            font,
            _drain: None,
            focused_once: false,
        });

        // drain：在主线程上从 event_rx 取事件喂给 Term。
        let weak = entity.downgrade();
        let drain = cx.spawn(async move |cx| {
            while let Ok(ev) = event_rx.recv().await {
                let applied = weak.update(cx, |this, cx| {
                    match ev {
                        SessionEvent::Connected => {
                            log::info!("terminal: connected");
                            this.state = ConnState::Connected;
                        }
                        SessionEvent::Output(b) => {
                            log::trace!("pty output ({}B): {}", b.len(), debug_bytes(&b));
                            this.parser.advance(&mut this.term, &b);
                        }
                        SessionEvent::Error(e) => {
                            log::warn!("terminal: error {e}");
                            this.state = ConnState::Error(e);
                        }
                        SessionEvent::Closed => {
                            log::info!("terminal: closed");
                            this.state = ConnState::Closed;
                        }
                    }
                    cx.notify();
                });
                if applied.is_err() {
                    break; // Entity 已销毁。
                }
            }
        });
        entity.update(cx, |this, _cx| this._drain = Some(drain));

        entity
    }

    /// 发送输入字节到远端。
    fn send_input(&self, bytes: Vec<u8>) {
        // async_channel 的 try_send 非阻塞；满了就丢弃（终端输入不应阻塞 UI）。
        log::trace!("pty write: {}", debug_bytes(&bytes));
        if let Err(e) = self.input_tx.try_send(InputCmd::Write(bytes)) {
            log::warn!("input_tx send failed (channel closed/full?): {e}");
        }
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
                self.cols, self.rows, new_cols, new_rows,
                bounds.w as u32, bounds.h as u32, self.cell_w.as_f32()
            );
            self.cols = new_cols;
            self.rows = new_rows;
            self.term.resize(TermSize {
                cols: new_cols,
                rows: new_rows,
            });
            let _ = self
                .input_tx
                .try_send(InputCmd::Resize { cols: new_cols as u16, rows: new_rows as u16 });
        }
    }

    fn handle_key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, _: &mut Context<Self>) {
        let ks = &ev.keystroke;
        match encode_keystroke(ks) {
            Some(bytes) => self.send_input(bytes),
            None => log::debug!("unhandled keystroke: key={} key_char={:?}", ks.key, ks.key_char),
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 打开/切回 tab 时自动聚焦终端，确保键盘输入直达 PTY（否则 on_key_down 不会触发）。
        if !self.focused_once && !matches!(self.state, ConnState::Error(_)) {
            self.focus.focus(window, cx);
            self.focused_once = true;
        }
        let focus = self.focus.clone();
        let entity = cx.entity();

        // 状态分支：未连接时显示提示。
        let status_overlay = match &self.state {
            ConnState::Connecting => Some("Connecting…"),
            ConnState::Error(e) => {
                return connecting_or_error_view(e, &focus).into_any_element();
            }
            ConnState::Closed => Some("Session closed."),
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
            let shaped = window.text_system().shape_line("M".into(), px(FONT_SIZE), &[run], None);
            self.cell_w = shaped.width().max(px(1.0));
        }

        // 捕获绘制所需的克隆。
        let bg = bg_of(&self.term);
        let font = self.font.clone();
        let cell_w = self.cell_w;
        let line_h = self.line_h;
        let weak = entity.downgrade();
        let weak2 = weak.clone();
        let default_fg = fg_of(&self.term);

        let canvas_el = canvas(
            move |bounds, _window, cx| {
                // prepaint：可能 resize。
                if let Some(t) = weak.upgrade() {
                    let _ = t.update(cx, |this, _cx| {
                        this.maybe_resize(Size {
                            w: bounds.size.width.as_f32(),
                            h: bounds.size.height.as_f32(),
                        });
                    });
                }
                bounds
            },
            move |bounds, _pre, window, cx| {
                // paint：先快照可见单元格（避免持有对 cx 的不可变借用），
                // 再用 &mut App 绘制。
                if let Some(t) = weak2.upgrade() {
                    let snapshot = {
                        let this = t.read(cx);
                        snapshot_visible(&this.term)
                    };
                    paint_snapshot(
                        &snapshot, bounds, cell_w, line_h, &font, default_fg, bg, window, cx,
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
                    .text_xs()
                    .text_color(hsla(0., 0., 0.8, 1.))
                    .child(SharedString::from(msg)),
            );
        }
        root.into_any_element()
    }
}

fn connecting_or_error_view(msg: &str, focus: &FocusHandle) -> impl IntoElement {
    div()
        .id("terminal-error")
        .size_full()
        .bg(rgb(0x1e1e20))
        .track_focus(focus)
        .flex()
        .items_center()
        .justify_center()
        .text_sm()
        .text_color(hsla(0.1, 0.6, 0.6, 1.))
        .child(SharedString::from(msg.to_string()))
}

/// 一个单元格的渲染快照（owned，避免绘制时持有对 term 的借用）。
#[derive(Clone, Copy)]
struct RenderCell {
    ch: char,
    fg: Hsla,
    bg: Option<Hsla>,
    bold: bool,
    italic: bool,
    spacer: bool,
}

/// 可见视口快照 + 光标位置。
struct Snapshot {
    rows: Vec<Vec<RenderCell>>,
    cursor: Option<(usize, usize)>, // (col, row_within_viewport)
}

/// 把 Term 可见区快照成 owned 数据。
fn snapshot_visible(term: &Term<NoopListener>) -> Snapshot {
    let grid = term.grid();
    let display_offset = grid.display_offset();
    let cols = term.columns();
    let rows = term.screen_lines();
    let top_visible = Line(-(display_offset as i32));
    let colors = term.colors();

    log::trace!(
        "snapshot_visible: display_offset={} top_visible={} cols={} rows={} total_lines={}",
        display_offset,
        top_visible.0,
        cols,
        rows,
        grid.total_lines()
    );

    let mut out_rows: Vec<Vec<RenderCell>> = Vec::with_capacity(rows);
    for r in 0..rows {
        let line = Line(top_visible.0 + r as i32);
        let row = &grid[line];
        let mut out: Vec<RenderCell> = Vec::with_capacity(cols);
        for c in 0..cols {
            let cell: &Cell = &row[Column(c)];
            out.push(RenderCell {
                ch: if cell.c == '\0' { ' ' } else { cell.c },
                fg: cell_fg(cell, colors),
                bg: cell_bg(cell, colors),
                bold: cell.flags.contains(CellFlags::BOLD),
                italic: cell.flags.contains(CellFlags::ITALIC),
                spacer: cell.flags.contains(CellFlags::WIDE_CHAR_SPACER),
            });
        }
        out_rows.push(out);
    }

    // 光标位置（视口内）。
    let cursor = {
        let cp = &grid.cursor.point;
        let cline = cp.line.0 - top_visible.0;
        if cline >= 0 && (cline as usize) < rows {
            Some((cp.column.0.min(cols.saturating_sub(1)), cline as usize))
        } else {
            None
        }
    };

    Snapshot { rows: out_rows, cursor }
}

/// 根据快照绘制。
fn paint_snapshot(
    snapshot: &Snapshot,
    bounds: Bounds<Pixels>,
    cell_w: Pixels,
    line_h: Pixels,
    font: &Font,
    default_fg: Hsla,
    default_bg: Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    let cell_wf = cell_w.as_f32();
    let line_hf = line_h.as_f32();

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
            let s: String = row.iter().map(|c| if c.spacer { ' ' } else { c.ch }).collect();
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

    for (r, row) in snapshot.rows.iter().enumerate() {
        // 把同行同 (fg,bg,attrs) 的 cell 聚成一段 run，减少 shape 次数。
        let mut text = String::with_capacity(row.len());
        let mut runs: Vec<TextRun> = Vec::new();
        let mut cur_fg: Option<Hsla> = None;
        let mut cur_bg: Option<Hsla> = None;
        let mut cur_bold = false;
        let mut cur_italic = false;
        let mut run_start_byte: usize = 0;

        let flush_run = |_text: &str,
                         runs: &mut Vec<TextRun>,
                         start: usize,
                         end: usize,
                         fg: Hsla,
                         bg: Option<Hsla>,
                         bold: bool,
                         italic: bool| {
            let len = end - start;
            if len == 0 {
                return;
            }
            runs.push(TextRun {
                len,
                font: Font {
                    weight: if bold { FontWeight::BOLD } else { FontWeight::NORMAL },
                    style: if italic { gpui::FontStyle::Italic } else { gpui::FontStyle::Normal },
                    family: font.family.clone(),
                    features: font.features.clone(),
                    fallbacks: font.fallbacks.clone(),
                },
                color: fg,
                background_color: bg,
                underline: None,
                strikethrough: None,
            });
        };

        for cell in row {
            if cell.spacer {
                continue;
            }
            if Some(cell.fg) != cur_fg || cell.bg != cur_bg || cell.bold != cur_bold
                || cell.italic != cur_italic
            {
                let _ = flush_run(
                    &text,
                    &mut runs,
                    run_start_byte,
                    text.len(),
                    cur_fg.unwrap_or(default_fg),
                    cur_bg,
                    cur_bold,
                    cur_italic,
                );
                run_start_byte = text.len();
                cur_fg = Some(cell.fg);
                cur_bg = cell.bg;
                cur_bold = cell.bold;
                cur_italic = cell.italic;
            }
            text.push(cell.ch);
        }
        let _ = flush_run(
            &text,
            &mut runs,
            run_start_byte,
            text.len(),
            cur_fg.unwrap_or(default_fg),
            cur_bg,
            cur_bold,
            cur_italic,
        );

        if text.is_empty() {
            continue;
        }
        let shaped = window.text_system().shape_line(
            SharedString::from(text),
            px(FONT_SIZE),
            &runs,
            None,
        );
        let origin = Point::new(bounds.origin.x, bounds.origin.y + px(r as f32 * line_hf));
        if let Err(e) = shaped.paint(origin, line_h, TextAlign::Left, None, window, cx) {
            log::warn!("paint row {r} failed: {e}");
        }
    }

    // 光标：实心块。
    if let Some((col, row)) = snapshot.cursor {
        if row < snapshot.rows.len() {
            let x = bounds.origin.x + px(col as f32 * cell_wf);
            let y = bounds.origin.y + px(row as f32 * line_hf);
            let cb = Bounds {
                origin: Point::new(x, y),
                size: gpui::size(cell_w, line_h),
            };
            window.paint_quad(quad(
                cb,
                Corners::default(),
                default_fg,
                Edges::default(),
                hsla(0., 0., 0., 0.),
                gpui::BorderStyle::default(),
            ));
        }
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

fn cell_fg(cell: &Cell, colors: &alacritty_terminal::term::color::Colors) -> Hsla {
    color_to_hsla(&cell.fg, colors)
        .map(|c| {
            if cell.flags.contains(CellFlags::DIM) {
                dimen(c)
            } else {
                c
            }
        })
        .unwrap_or_else(|| default_palette(&NamedColor::Foreground))
}

fn cell_bg(cell: &Cell, colors: &alacritty_terminal::term::color::Colors) -> Option<Hsla> {
    color_to_hsla(&cell.bg, colors)
}

/// 把 alacritty/vte 的 Color 解析为 Hsla。Named/Indexed 走终端调色板，Spec 直传。
fn color_to_hsla(
    color: &Color,
    colors: &alacritty_terminal::term::color::Colors,
) -> Option<Hsla> {
    match color {
        Color::Spec(rgb) => Some(rgb_to_hsla(*rgb)),
        Color::Named(n) => {
            let idx = *n as usize;
            colors[idx].map(rgb_to_hsla).or_else(|| Some(default_palette(n)))
        }
        Color::Indexed(i) => {
            let idx = *i as usize;
            if idx < 256 {
                colors[idx].map(rgb_to_hsla).or_else(|| Some(default_palette_indexed(idx)))
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
    use NamedColor::*;
    let rgb: [u8; 3] = match n {
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
        Foreground => [0xe6, 0xe6, 0xe6],
        Background => [0x12, 0x12, 0x14],
        Cursor => [0xe6, 0xe6, 0xe6],
        DimForeground => [0x9a, 0x9a, 0x9a],
    };
    Hsla::from(gpui::rgb(((rgb[0] as u32) << 16) | ((rgb[1] as u32) << 8) | rgb[2] as u32))
}

/// 256 色的回退（xterm 配色：16 色 + 6×6×6 立方 + 24 级灰度）。
fn default_palette_indexed(i: usize) -> Hsla {
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
        default_palette(&n)
    } else if i < 232 {
        let i = i - 16;
        let r = i / 36;
        let g = (i / 6) % 6;
        let b = i % 6;
        let v = |x: usize| if x == 0 { 0 } else { 0x37 + 0x28 * x };
        Hsla::from(gpui::rgb(
            ((v(r) as u32) << 16) | ((v(g) as u32) << 8) | v(b) as u32,
        ))
    } else {
        let v = 8 + (i - 232) * 10;
        Hsla::from(gpui::rgb(((v as u32) << 16) | ((v as u32) << 8) | v as u32))
    }
}

fn dimen(c: Hsla) -> Hsla {
    Hsla {
        a: c.a * 0.6,
        ..c
    }
}

// ─── 输入编码 ───────────────────────────────────────────────────────────────
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
pub fn encode_keystroke(ks: &gpui::Keystroke) -> Option<Vec<u8>> {
    let m = &ks.modifiers;
    let key = ks.key.as_str();

    // 控制键组合（不受 key_char 影响）。
    if m.control && !m.platform {
        if let Some(ch) = key.chars().next() {
            let lc = ch.to_ascii_lowercase();
            if ('a'..='z').contains(&lc) {
                return Some(vec![(lc as u8) - b'a' + 1]);
            }
            match lc {
                '[' => return Some(vec![0x1b]),
                '\\' => return Some(vec![0x1c]),
                ']' => return Some(vec![0x1d]),
                '^' => return Some(vec![0x1e]),
                '_' => return Some(vec![0x1f]),
                _ => {}
            }
        }
    }

    // 特殊键。
    let bytes: Vec<u8> = match key {
        "enter" | "return" => vec![b'\r'],
        "backspace" => vec![0x7f],
        "tab" => vec![b'\t'],
        "escape" => vec![0x1b],
        "left" => esc(if m.shift { "1D" } else { "D" }, m),
        "right" => esc(if m.shift { "1C" } else { "C" }, m),
        "up" => esc(if m.shift { "1A" } else { "A" }, m),
        "down" => esc(if m.shift { "1B" } else { "B" }, m),
        "home" => esc(if m.shift { "1H" } else { "H" }, m),
        "end" => esc(if m.shift { "1F" } else { "F" }, m),
        "delete" => b"\x1b[3~".to_vec(),
        "pageup" => b"\x1b[5~".to_vec(),
        "pagedown" => b"\x1b[6~".to_vec(),
        "space" => vec![b' '],
        _ => {
            // 可打印字符：优先用 key_char（已含 shift/option 组合结果）。
            if let Some(ch) = ks.key_char.as_ref().and_then(|s| s.chars().next()) {
                if !m.control && !m.platform {
                    let mut s = ch.to_string();
                    if m.alt {
                        s.insert(0, '\x1b');
                    }
                    return Some(s.into_bytes());
                }
            }
            return None;
        }
    };
    Some(bytes)
}

/// 计算带修饰键时的 CSI 修饰码（shift=1, alt=2, control=4, meta=8，再 +1）。
fn esc(suffix: &str, m: &Modifiers) -> Vec<u8> {
    if m.control && !m.alt && !m.shift {
        return format!("\x1bO{suffix}").into_bytes();
    }
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
    format!("\x1b[1;{code}{suffix}").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::index::{Column, Line};

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
        let mut term: Term<NoopListener> = Term::new(config, &size, NoopListener);
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
        let mut term: Term<NoopListener> =
            Term::new(config, &TermSize { cols: 80, rows: 10 }, NoopListener);
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
        term.resize(TermSize { cols: 100, rows: 30 });

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

        assert!(all.contains("backup"), "after resize, visible area missing 'backup'");
        assert!(all.contains("card"), "after resize, visible area missing 'card'");
    }

    /// 验证把字节流切成极小 chunk（模拟 drain 分批 advance）后，
    /// parser 仍能把 ls 结果正确写入 grid（跨 chunk 的 OSC/CSI 不断）。
    #[test]
    fn term_parses_chunked_output() {
        let config = Config {
            scrolling_history: SCROLLBACK,
            ..Default::default()
        };
        let mut term: Term<NoopListener> =
            Term::new(config, &TermSize { cols: 80, rows: 10 }, NoopListener);
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

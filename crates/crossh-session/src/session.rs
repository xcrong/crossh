//! 会话属主：回放环、输入所有权、尺寸仲裁、会话元数据。
//!
//! - 每会话 512 KiB 分段输出环（按尺寸封段、保留最近、先丢最旧），见 D9。
//! - 输入所有权：首个接入者为 controller；在接客户端发送一次输入即取得；
//!   controller 断开后角色为“无”。尺寸只接受 controller 上报，见 D8。
//! - 本模块零 IO，可单元测试；PTY 读写线程在 daemon 侧组装。

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::feed::{FeedState, FeedUpdate};
use crate::workspace::TermSize;

/// 回放上限：每会话最近 512 KiB 原始输出（写入规格 R3）。
pub const REPLAY_LIMIT_BYTES: usize = 512 * 1024;

/// 分段上限：与对照实现同值，超限合并最旧两段（只抄思路不抄代码）。
pub const MAX_SEGMENTS: usize = 64;

/// 回放段：同一尺寸下连续输出的字节，快照按段携带各自几何。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RingSegment {
    pub size: TermSize,
    pub bytes: Vec<u8>,
}

/// 脱离期攒下的终端查询（DA/配色），有订阅时由客户端正常应答，
/// 无订阅时由 daemon 按固定深色代答（3.3），此处只做识别与清理。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Query {
    /// `CSI c` / `CSI 0 c`：主设备属性。
    DeviceAttributes,
    /// `CSI ? 996 n` / `OSC 11 ; ?`：深浅色查询（同按深色代答）。
    ColorScheme,
}

impl Query {
    /// 脱离期固定深色应答字节（直接写回 PTY，不悬挂）。
    /// - DA 回 `VT102`（`ESC[?6c`，对照实现同值）。
    /// - 主题回深色（`ESC[?997;1n`；daemon 无窗口取主题，浅色由后续真实查询纠正）。
    pub fn reply_bytes(&self) -> &'static [u8] {
        match self {
            Query::DeviceAttributes => DA_REPLY,
            Query::ColorScheme => DARK_THEME_REPLY,
        }
    }
}

/// DA 固定应答：`VT102`。
pub const DA_REPLY: &[u8] = b"\x1b[?6c";
/// 深色主题固定应答：daemon 无窗口取主题，浅色偏差后续纠正。
pub const DARK_THEME_REPLY: &[u8] = b"\x1b[?997;1n";

/// 最小终端模式折叠：记住是否在 `ALT_SCREEN`，兼识别脱离期查询。
///
/// - 与环内字节用同一状态机推进，跨 `push` 分批携带（调用方可按 PTY 切块喂）。
/// - 驱逐字节同步 `feed` 进 `head_modes` 后立刻 `take_queries` 丢弃，
///   脱离期查询不堆积终身。
/// - 只跟踪 `ALT_SCREEN`（`?1049`）：2.2 判定首画去留够用，其余模式不记
///   （ponytail: 上限即所需，鼠标/括号粘贴等后续任务再加）。
#[derive(Clone, Debug, Default)]
pub struct TerminalModes {
    alt_screen: bool,
    queries: Vec<Query>,
    state: ModeState,
    params: Vec<u8>,
    /// 当前 OSC 负载（`ESC ]` 之后到终止符前，跨 `push` 续接，查 `OSC 11 ; ?` 用）。
    osc_buf: Vec<u8>,
}

/// 进入备用屏的 DEC 私有模式号（`CSI ? 1049 h/l`），对照实现同值。
pub const ALT_SCREEN: u16 = 1049;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ModeState {
    #[default]
    Text,
    Esc,
    Csi {
        marker: u8,
    },
    Osc,
    OscEsc,
}

impl TerminalModes {
    pub fn new() -> Self {
        Self::default()
    }

    /// 自上次调用以来攒下的查询（调用方必须取走，否则堆积；驱逐路径直接丢弃）。
    pub fn take_queries(&mut self) -> Vec<Query> {
        std::mem::take(&mut self.queries)
    }

    /// `mode` 是否开着（2.1 只跟踪 `ALT_SCREEN`，其余恒假）。
    pub fn is_on(&self, mode: u16) -> bool {
        mode == ALT_SCREEN && self.alt_screen
    }

    /// 缺失模式的前导字节：全量开着、但回放环内已无的才补（环自带者优先）。
    /// 现仅 `ALT_SCREEN` 一切换（`ESC[?1049h`），与 `FeedState::replay_prelude` 配套。
    pub fn restore_bytes_beyond(&self, replayed: &Self) -> Option<Vec<u8>> {
        if self.is_on(ALT_SCREEN) && !replayed.is_on(ALT_SCREEN) {
            return Some(b"\x1b[?1049h".to_vec());
        }
        None
    }

    /// 折叠一批 PTY 输出：记 `ALT_SCREEN` 开关，兼识别查询。
    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            match self.state {
                ModeState::Text => {
                    if b == 0x1b {
                        self.state = ModeState::Esc;
                    }
                }
                ModeState::Esc => match b {
                    b'[' => {
                        self.params.clear();
                        self.state = ModeState::Csi { marker: 0 };
                    }
                    b']' => {
                        self.params.clear();
                        self.osc_buf.clear();
                        self.state = ModeState::Osc;
                    }
                    // CAN/SUB 中止一切；RIS 回基态并清模式（与客户端模拟器一致）。
                    0x18 | 0x1a => self.state = ModeState::Text,
                    b'c' => {
                        self.alt_screen = false;
                        self.state = ModeState::Text;
                    }
                    0x1b => {}
                    _ => self.state = ModeState::Text,
                },
                ModeState::Csi { marker } => match b {
                    b'?' | b'>' | b'<' | b'=' if marker == 0 && self.params.is_empty() => {
                        self.state = ModeState::Csi { marker: b };
                    }
                    b'0'..=b'9' | b';' | b':' => {
                        self.params.push(b);
                        if self.params.len() > 64 {
                            self.state = ModeState::Text;
                        }
                    }
                    b'n' => {
                        if marker == b'?' && self.params.as_slice() == b"996" {
                            self.queries.push(Query::ColorScheme);
                        }
                        self.state = ModeState::Text;
                    }
                    b'h' | b'l' => {
                        if marker == b'?' {
                            self.apply_modes(b == b'h');
                        } else {
                            self.params.clear();
                        }
                        self.state = ModeState::Text;
                    }
                    b'c' => {
                        if marker == 0 && matches!(self.params.as_slice(), b"" | b"0") {
                            self.queries.push(Query::DeviceAttributes);
                        }
                        self.state = ModeState::Text;
                    }
                    // 其余终结字节（含 `u` 等）直接回基态；`$p` 查询走这里，不算模式开关。
                    _ => self.state = ModeState::Text,
                },
                // OSC 负载内的 `ESC [` 不得误判为查询，直接跳过整段；
                // 仅 `OSC 11 ; ?` 记为主题查询（与 `?996n` 同按深色代答）。
                ModeState::Osc => match b {
                    0x07 => {
                        self.check_osc_query();
                        self.state = ModeState::Text;
                    }
                    0x1b => self.state = ModeState::OscEsc,
                    _ => {
                        if self.osc_buf.len() < 64 {
                            self.osc_buf.push(b);
                        }
                    }
                },
                ModeState::OscEsc => match b {
                    b'\\' => {
                        self.check_osc_query();
                        self.state = ModeState::Text;
                    }
                    0x1b => {}
                    _ => self.state = ModeState::Osc,
                },
            }
        }
    }

    /// 检查当前 OSC 是否主题查询（`11;?`）：是则记一笔，同 `?996n` 代答深色。
    fn check_osc_query(&mut self) {
        if self.osc_buf.as_slice() == b"11;?" {
            self.queries.push(Query::ColorScheme);
        }
        self.osc_buf.clear();
    }

    /// 应用 `CSI ? ... h/l`：只认 `1049`，多参数共存（如 `?1000;1049h`）分别处理。
    fn apply_modes(&mut self, on: bool) {
        for param in self.params.split(|b| *b == b';') {
            let Ok(text) = std::str::from_utf8(param) else {
                continue;
            };
            let Ok(mode) = text.parse::<u16>() else {
                continue;
            };
            if mode == ALT_SCREEN {
                self.alt_screen = on;
            }
        }
        self.params.clear();
    }
}

/// 刚够判断“回放能否从这里开始”的 VT 文法子集（只抄思路不抄代码）。
///
/// - 字符串（OSC/DCS 等）止于 `ESC`（`ESC \` 即 ST），OSC 还止于 BEL；
/// - CAN/SUB 中止一切；C0 控制字符不结束序列（与 `vte` 一致）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum HeadCut {
    #[default]
    Ground,
    Esc,
    EscIntermediate,
    Csi,
    Str {
        osc: bool,
    },
}

impl HeadCut {
    fn step(&mut self, b: u8) {
        *self = match (*self, b) {
            (_, 0x18 | 0x1a) => Self::Ground,
            (_, 0x1b) => Self::Esc,
            (Self::Ground, _) => Self::Ground,
            (Self::Esc, b'[') => Self::Csi,
            (Self::Esc, b']') => Self::Str { osc: true },
            (Self::Esc, b'P' | b'X' | b'^' | b'_') => Self::Str { osc: false },
            (Self::Esc | Self::EscIntermediate, 0x20..=0x2f) => Self::EscIntermediate,
            (state @ (Self::Esc | Self::EscIntermediate | Self::Csi), 0x00..=0x1f | 0x7f) => state,
            (Self::Esc | Self::EscIntermediate, _) => Self::Ground,
            (Self::Csi, 0x40..=0x7e) => Self::Ground,
            (Self::Csi, _) => Self::Csi,
            (Self::Str { osc: true }, 0x07) => Self::Ground,
            (state @ Self::Str { .. }, _) => state,
        };
    }

    fn fold(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.step(b);
        }
    }

    /// 回放能否从 `next` 开始：不在序列内，且不在 UTF-8 续接字节上。
    fn at_boundary(self, next: u8) -> bool {
        self == Self::Ground && !(0x80..=0xbf).contains(&next)
    }
}

/// 环形缓冲：按尺寸分段，只保留最近 `REPLAY_LIMIT_BYTES` 字节。
///
/// - `push` 追加到尾段；`resize` 时封段（空尾直接改尺寸，非空尾推新段）。
/// - 段数超 `MAX_SEGMENTS` 时合并最旧两段；总预算仍是上限 + 段开销。
/// - `replay()` 拼旧单流（过渡期给接入回放用）；新接入按段用 `segments()`。
#[derive(Clone, Debug)]
pub struct ReplayRing {
    segments: VecDeque<RingSegment>,
    len: usize,
    /// 头部解析状态：被丢掉字节折叠至此，回放永不从 CSI/OSC/UTF-8 中间开始。
    head: HeadCut,
    /// 与头部同点的终端状态：被驱逐字节同步折叠，查询即时丢弃防堆积。
    head_modes: TerminalModes,
}

impl Default for ReplayRing {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplayRing {
    pub fn new() -> Self {
        Self {
            segments: VecDeque::from([RingSegment {
                size: TermSize::default(),
                bytes: Vec::new(),
            }]),
            len: 0,
            head: HeadCut::default(),
            head_modes: TerminalModes::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 当前尾段几何（新输出的记录尺寸）。
    pub fn size(&self) -> TermSize {
        self.segments.back().map(|s| s.size).unwrap_or_default()
    }

    /// 段数（含空尾；`segments()` 快照会跳过空段）。
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    /// 追加原始输出，超出上限时从头部裁剪（保留最近）。
    pub fn push(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if self.segments.is_empty() {
            self.segments.push_back(RingSegment {
                size: TermSize::default(),
                bytes: Vec::new(),
            });
        }
        // 单次写入超过上限：旧段全丢，只保留输入尾部，几何按当前尾段记。
        // 丢掉的前史仍过一遍 `head/head_modes`，再按边界对齐（与常规驱逐一致）。
        if bytes.len() > REPLAY_LIMIT_BYTES {
            let size = self.size();
            let old: Vec<RingSegment> = self.segments.drain(..).collect();
            for seg in &old {
                self.head.fold(&seg.bytes);
                self.head_modes.feed(&seg.bytes);
            }
            let kept = bytes.len() - REPLAY_LIMIT_BYTES;
            self.head.fold(&bytes[..kept]);
            self.head_modes.feed(&bytes[..kept]);
            // ponytail: 脱离期查询无人应答，攒着只会堆积终身，直接丢弃（3.3 代答）。
            self.head_modes.take_queries();
            self.segments.push_back(RingSegment {
                size,
                bytes: bytes[kept..].to_vec(),
            });
            self.len = REPLAY_LIMIT_BYTES;
            self.evict(0);
            return;
        }
        if let Some(tail) = self.segments.back_mut() {
            tail.bytes.extend_from_slice(bytes);
        }
        self.len += bytes.len();
        self.evict(self.len.saturating_sub(REPLAY_LIMIT_BYTES));
    }

    /// 尺寸变化时封段：空尾直接改尺寸，非空尾推新段。
    ///
    /// 超段数时先合并最旧两段再推新段；`nudge` 式的临时尺寸不得调这里。
    pub fn resize(&mut self, size: TermSize) {
        if self.segments.is_empty() {
            self.segments.push_back(RingSegment {
                size,
                bytes: Vec::new(),
            });
            return;
        }
        let tail = self.segments.back_mut().expect("环恒有尾段");
        if tail.size == size {
            return;
        }
        if tail.bytes.is_empty() {
            tail.size = size;
            return;
        }
        if self.segments.len() >= MAX_SEGMENTS {
            let mut old = self.segments.pop_front().expect("超限时必有头段");
            let head = self.segments.front_mut().expect("段上限至少为 2");
            old.bytes.extend_from_slice(&head.bytes);
            head.bytes = old.bytes;
        }
        self.segments.push_back(RingSegment {
            size,
            bytes: Vec::new(),
        });
    }

    /// 丢 `n` 字节后继续推进到解析边界（#857：`33;C` 碎片与半个 CJK 不回放）。
    ///
    /// - 上限切点落在 OSC/CSI/UTF-8 中间时，整段序列直接丢掉：客户端从基态解析，
    ///   序列尾部画成文本（`33;C`）或半个字符画成替换符反而更糟。
    /// - 每个被驱逐字节同步折叠进 `head_modes`，查询即时丢弃（旧查询不堆积）。
    /// - 空头段顺手摘掉，但至少留一段。
    fn evict(&mut self, mut n: usize) {
        loop {
            let (head, head_modes, segments, len) = (
                &mut self.head,
                &mut self.head_modes,
                &mut self.segments,
                &mut self.len,
            );
            let Some(front) = segments.front_mut() else {
                return;
            };
            let mut take = 0;
            for &b in front.bytes.iter() {
                if n == 0 && head.at_boundary(b) {
                    break;
                }
                head.step(b);
                n = n.saturating_sub(1);
                take += 1;
            }
            head_modes.feed(&front.bytes[..take]);
            // ponytail: 同超大写入路径，驱逐即丢弃，防脱离期堆积。
            head_modes.take_queries();
            front.bytes.drain(..take);
            *len -= take;
            if !front.bytes.is_empty() || segments.len() == 1 {
                return;
            }
            segments.pop_front();
        }
    }

    /// 取走头部折叠时攒下的查询（正常路径驱逐即清，此处恒空；供单测与 3.3 代答）。
    pub fn take_queries(&mut self) -> Vec<Query> {
        self.head_modes.take_queries()
    }

    /// 环内字节折出的模式：`attach` 判定“首画是否还在环内”用。
    ///
    /// - 当前全量开着、但这里已无 `?1049h`，即首画已随前史被驱逐（2.2 才发起 nudge）。
    /// - 跨段分批喂，`feed` 自带状态不断，段边界对折叠不可见。
    pub fn modes(&self) -> TerminalModes {
        let mut modes = TerminalModes::new();
        for seg in &self.segments {
            modes.feed(&seg.bytes);
        }
        // ponytail: 查询是问活程序的，重放不重播，只留模式位。
        modes.take_queries();
        modes
    }

    /// 分段快照：按段携带几何，空段跳过（不占回放）。
    pub fn segments(&self) -> Vec<RingSegment> {
        self.segments
            .iter()
            .filter(|s| !s.bytes.is_empty())
            .cloned()
            .collect()
    }

    /// 按写入顺序拼接（旧单流过渡用）。
    pub fn replay(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len);
        for s in &self.segments {
            out.extend_from_slice(&s.bytes);
        }
        out
    }
}

/// 输入权持有者：`None` 为暂无持有者。
pub type ControllerId = u64;

/// 重连重画冷却：上次 nudge 后这么久内不再发起（对照 `REDRAW_NUDGE_COOLDOWN`）。
///
/// - 突发重连只请一次：冷却内重连的请求已在环里或在路上（对照实现同值 2s）。
pub const REDRAW_NUDGE_COOLDOWN: Duration = Duration::from_secs(2);

/// 内核问 PTY 前台进程组（对照 `tcgetpgrp` 探针）。
///
/// - 成功返回前台进程组 id；失败/`<=0` 返回 `None`，调用方按无前台处理。
/// - Windows 无 `tcgetpgrp`：恒返回 `None`，即恒按无前台处理，只回放不 nudge。
#[cfg(unix)]
pub fn pty_foreground_pgid(fd: std::os::unix::io::RawFd) -> Option<i32> {
    match unsafe { libc::tcgetpgrp(fd) } {
        pg if pg > 0 => Some(pg),
        _ => None,
    }
}

/// Windows 无 `tcgetpgrp`：恒按无前台处理，只回放不 nudge。
#[cfg(not(unix))]
pub fn pty_foreground_pgid() -> Option<i32> {
    None
}

/// 前台是否有 shell 之外的程序：`Some(pg) != shell` 即有。
///
/// - `None`/非法值一律按无前台处理（`tcgetpgrp` 失败、shell pid 缺失、Windows）。
/// - 无前台时只回放不 nudge（诚实边界：shell 直接重放整屏即可）。
pub fn foreground_command_running(fg_pgid: Option<i32>, shell_pid: Option<u32>) -> bool {
    match (fg_pgid, shell_pid) {
        (Some(pg), Some(shell)) if pg > 0 => pg as u32 != shell,
        _ => false,
    }
}

/// `redraw_nudge` 的输入快照：调用方从 feed 与会话状态组装。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RedrawNudgeInput {
    /// 会话是否存活（死会话只回放，不请重画）。
    pub alive: bool,
    /// 前台是否有程序（Unix 问 `tcgetpgrp`，Windows 恒 false）。
    pub foreground_program: bool,
    /// 当前是否在备用屏（全量折叠，含已驱逐前史）。
    pub alt_now: bool,
    /// 环内是否还留着进备用屏的切换（只看现存字节）。
    pub alt_in_ring: bool,
    /// 当前尾段几何（临时尺寸以它为基准行位移）。
    pub size: TermSize,
    /// 是否 observer 重连：不影响判定（同样可触发 PTY 侧请求，只动 PTY 不改布局）。
    pub is_observer: bool,
}

/// 重连是否请前台全屏程序重画，命中返回 PTY 临时尺寸并记录时间（对照 `redraw_nudge`）。
///
/// - 全真才触发：存活 × 前台有程序 × 在备用屏 × 首画已丢（环内无切换）× 出冷却。
/// - 首画仍在/主屏/shell 时零打扰：回放即整屏，不请重画。
/// - 动作是行位移一行（`rows>1 ? rows-1 : rows+1`，同尺寸内核不发 `SIGWINCH`）。
/// - 返回的临时尺寸只给 PTY 用：不写环分段、不发客户端 `Size`（3.1 落地）。
/// - observer 与 controller 同权：`is_observer` 显式忽略，只动 PTY 不影响布局。
pub fn redraw_nudge(
    input: &RedrawNudgeInput,
    last_nudge: &mut Option<Instant>,
    now: Instant,
) -> Option<TermSize> {
    // ponytail: observer 不进判定，PT 票侧触发与布局静默正交，忽略即正确。
    let _ = input.is_observer;
    if !input.alive || !input.foreground_program || !input.alt_now || input.alt_in_ring {
        return None;
    }
    if last_nudge.is_some_and(|at| now.saturating_duration_since(at) < REDRAW_NUDGE_COOLDOWN) {
        return None;
    }
    *last_nudge = Some(now);
    let rows = if input.size.rows > 1 {
        input.size.rows - 1
    } else {
        input.size.rows + 1
    };
    Some(TermSize { rows, ..input.size })
}

/// nudge 暂留时长：背靠背两次改尺寸会被内核合并为一次 `SIGWINCH`，
/// 程序在信号处理时读尺寸若已复原就会跳过重画；暂留 50ms 让前台程序先看到
/// 临时尺寸（对照 `tty7` 的 `REDRAW_NUDGE_SETTLE`，只抄思路不抄代码）。
pub const REDRAW_NUDGE_SETTLE: Duration = Duration::from_millis(50);

/// nudge 两步：`transient` 立刻下发，`settle` 后读 `settled` 恢复最新值。
///
/// - 只动 PTY：不写环、不发客户端 `Size` 由调用方保证（恢复读环尾最新值，
///   真实 `resize` 已封段；临时尺寸从不进环）。
/// - `settled` 为 `None`（会话已死/锁不住）则放弃，不再下发第二次。
/// - 恢复跑在独立线程，主流程（`attach` 回放）不被 50ms 挡住。
/// - 连续 `attach` 各起一线程，都读最新值恢复，以后到者为准自然收敛。
pub fn run_redraw_nudge(
    transient: TermSize,
    settle: Duration,
    resize: impl Fn(TermSize) + Send + 'static,
    settled: impl FnOnce() -> Option<TermSize> + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    // 合并窗口：先让程序看到一次尺寸变化（即一次 `SIGWINCH`），再等它读到。
    resize(transient);
    std::thread::Builder::new()
        .name("crossh-redraw-nudge".into())
        .spawn(move || {
            std::thread::sleep(settle);
            // 睡眠窗内真实 `resize` 已更新环尾：恢复读最新值，不覆盖用户尺寸。
            if let Some(size) = settled() {
                resize(size);
            }
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnershipEvent {
    /// 某客户端取得输入权（含首次）。
    Takeover { new_controller: ControllerId },
    /// controller 断开，输入权空出。
    Released,
    /// observer 输入被丢弃（只观察）。
    ObserverInputDropped { from: ControllerId },
}

/// 输入所有权状态机（v1：发送一次输入即取得，无额外命令）。
#[derive(Clone, Debug, Default)]
pub struct Ownership {
    controller: Option<ControllerId>,
}

impl Ownership {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn controller(&self) -> Option<ControllerId> {
        self.controller
    }

    /// 客户端接入：首个接入者成为 controller，其余为 observer（返回 `None`）。
    pub fn on_attach(&mut self, client: ControllerId) -> Option<OwnershipEvent> {
        if self.controller.is_none() {
            self.controller = Some(client);
            Some(OwnershipEvent::Takeover {
                new_controller: client,
            })
        } else {
            None
        }
    }

    /// 客户端发送输入：在接者即取得输入权；非 controller 的输入应被丢弃。
    /// 返回 `Some(Takeover)` 表示易手，`Some(Dropped)` 表示应丢弃。
    pub fn on_input(&mut self, from: ControllerId) -> Option<OwnershipEvent> {
        if self.controller == Some(from) {
            None
        } else {
            // 发送一次输入即取得（规格 R2）。
            self.controller = Some(from);
            Some(OwnershipEvent::Takeover {
                new_controller: from,
            })
        }
    }

    /// 断开：是 controller 则释放，否则无变化。
    pub fn on_detach(&mut self, client: ControllerId) -> Option<OwnershipEvent> {
        if self.controller == Some(client) {
            self.controller = None;
            Some(OwnershipEvent::Released)
        } else {
            None
        }
    }

    /// 尺寸上报是否接受：仅 controller。
    pub fn accepts_resize(&self, from: ControllerId) -> bool {
        self.controller == Some(from)
    }
}

/// 会话元数据（列表与结束判定用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionMeta {
    pub id: String,
    pub cwd: Option<String>,
    /// 标题占位（服务端不解析标题语义）。
    pub title: Option<String>,
    /// 最近一次程序状态名（`idle`/`working`/…），`None` = 未知或已清除。
    pub program: Option<String>,
    pub controller: Option<ControllerId>,
    pub alive: bool,
    pub exit_code: Option<i32>,
}

/// 会话表：id →（元数据 + 字节馈送 + 所有权）。
#[derive(Debug, Default)]
pub struct SessionOwner {
    sessions: HashMap<String, (SessionMeta, FeedState, Ownership)>,
}

impl SessionOwner {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn insert(&mut self, meta: SessionMeta) {
        let id = meta.id.clone();
        self.sessions
            .entry(id)
            .or_insert((meta, FeedState::new(), Ownership::new()));
    }

    pub fn get(&self, id: &str) -> Option<&SessionMeta> {
        self.sessions.get(id).map(|(m, _, _)| m)
    }

    /// 推入输出字节，返回派生的会话表更新（cwd/程序状态变化时调用方同步元数据）。
    pub fn push_output(&mut self, id: &str, bytes: &[u8]) -> Option<FeedUpdate> {
        self.sessions
            .get_mut(id)
            .map(|(_, feed, _)| feed.push(bytes))
    }
    /// 子进程退出：标记结束并记录退出码，调用方随后广播并移除。
    pub fn mark_exited(&mut self, id: &str, code: i32) -> Option<SessionMeta> {
        if let Some((meta, _, _)) = self.sessions.get_mut(id) {
            meta.alive = false;
            meta.exit_code = Some(code);
            return Some(meta.clone());
        }
        None
    }

    pub fn remove(&mut self, id: &str) {
        self.sessions.remove(id);
    }

    pub fn list(&self) -> Vec<SessionMeta> {
        self.sessions.values().map(|(m, _, _)| m.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_tail_only() {
        let mut ring = ReplayRing::new();
        ring.push(b"hello ");
        ring.push(b"world");
        assert_eq!(ring.replay(), b"hello world");
        // 超出上限只保留尾部。
        let big = vec![b'x'; REPLAY_LIMIT_BYTES + 100];
        ring.push(&big);
        assert_eq!(ring.len(), REPLAY_LIMIT_BYTES);
        assert_eq!(ring.replay(), &big[big.len() - REPLAY_LIMIT_BYTES..]);
    }

    fn size_of(cols: u16, rows: u16) -> TermSize {
        TermSize { cols, rows }
    }

    #[test]
    fn resize_seals_segment_with_geometry() {
        let mut ring = ReplayRing::new();
        ring.push(b"a");
        ring.resize(size_of(100, 30));
        ring.push(b"b");
        let segs = ring.segments();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].size, TermSize::default());
        assert_eq!(segs[0].bytes, b"a");
        assert_eq!(segs[1].size, size_of(100, 30));
        assert_eq!(segs[1].bytes, b"b");
        // 拼单流与分段一致。
        assert_eq!(ring.replay(), b"ab");
    }

    #[test]
    fn resize_empty_tail_updates_size_in_place() {
        let mut ring = ReplayRing::new();
        // 空环封段不产生新段。
        ring.resize(size_of(100, 30));
        assert_eq!(ring.segment_count(), 1);
        assert_eq!(ring.size(), size_of(100, 30));
        assert!(ring.segments().is_empty());
        // 同尺寸重复调是空操作。
        ring.push(b"x");
        ring.resize(size_of(100, 30));
        assert_eq!(ring.segment_count(), 1);
        assert_eq!(ring.segments().len(), 1);
    }

    #[test]
    fn segments_merge_oldest_when_over_cap() {
        let mut ring = ReplayRing::new();
        // 70 次“写 1 字节 + 换尺寸”（尺寸全与默认值不同，保证每次都封段）：
        // 段数封顶 64，尾段为空。
        for i in 0..70u16 {
            ring.push(&[i as u8]);
            ring.resize(size_of(100 + i, 30));
        }
        assert_eq!(ring.segment_count(), MAX_SEGMENTS);
        // 7 次合并都卷进头段：初始段先吃下第 0 个字节，尺寸滞后一位，
        // 故头段是第 0~7 个字节，几何取第 7 次合并时的头段尺寸。
        let segs = ring.segments();
        assert_eq!(segs.len(), MAX_SEGMENTS - 1); // 空尾被快照跳过
        let expect: Vec<u8> = (0..8u16).map(|i| i as u8).collect();
        assert_eq!(segs[0].bytes, expect);
        assert_eq!(segs[0].size, size_of(106, 30));
        // 总字节与顺序不受合并影响。
        let flat = ring.replay();
        assert_eq!(flat.len(), 70);
        assert_eq!(flat, (0..70u16).map(|i| i as u8).collect::<Vec<_>>());
    }

    #[test]
    fn oversized_push_keeps_tail_and_size() {
        let mut ring = ReplayRing::new();
        ring.resize(size_of(100, 30));
        ring.push(b"old");
        let big = vec![b'y'; REPLAY_LIMIT_BYTES + 10];
        ring.push(&big);
        // 单次超限写入：旧段全丢，只留输入尾部，几何按当前尾段记。
        assert_eq!(ring.len(), REPLAY_LIMIT_BYTES);
        assert_eq!(ring.replay(), &big[big.len() - REPLAY_LIMIT_BYTES..]);
        let segs = ring.segments();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].size, size_of(100, 30));
    }

    #[test]
    fn evict_advances_past_split_csi() {
        let mut ring = ReplayRing::new();
        ring.push(b"\x1b[31m"); // 头部即 CSI（5 字节）
        ring.push(&vec![b'x'; REPLAY_LIMIT_BYTES - 5]);
        assert_eq!(ring.len(), REPLAY_LIMIT_BYTES);
        // 溢出 2 字节：切点落在 CSI 中间，必须推进到 `m` 之后。
        ring.push(b"YZ");
        // 对齐多丢的 3 字节不补：长度回落到上限之下。
        assert_eq!(ring.len(), REPLAY_LIMIT_BYTES - 3);
        let flat = ring.replay();
        assert!(flat.starts_with(b"x"), "CSI 整段消失，不留 `1m` 碎片");
        assert_eq!(&flat[flat.len() - 2..], b"YZ");
        assert!(ring.take_queries().is_empty());
    }

    #[test]
    fn evict_never_leaves_osc_tail() {
        let mut ring = ReplayRing::new();
        // `ESC ] 133;C BEL` 共 8 字节 + `OK`。
        ring.push(b"\x1b]133;C\x07OK");
        ring.push(&vec![b'.'; REPLAY_LIMIT_BYTES - 10]);
        ring.push(b"!");
        let flat = ring.replay();
        assert!(
            flat.starts_with(b"OK"),
            "OSC 整段丢掉，不留 `33;C` 碎片，头 = {:?}",
            &flat[..flat.len().min(8)]
        );
        assert_eq!(flat[flat.len() - 1], b'!');
    }

    #[test]
    fn evict_never_leaves_half_cjk() {
        let mut ring = ReplayRing::new();
        ring.push("下一步".as_bytes()); // 3 个 CJK 字符，共 9 字节
        ring.push(&vec![b'.'; REPLAY_LIMIT_BYTES - 9]);
        // 溢出 1 字节：切进首字中间，必须丢掉整个首字。
        ring.push(b"!");
        let flat = ring.replay();
        assert!(
            !matches!(flat[0], 0x80..=0xbf),
            "半个 CJK 不回放，首字节 = {:#x}",
            flat[0]
        );
        assert!(
            flat.starts_with("一步".as_bytes()),
            "首字整字丢掉，从字符边界开始"
        );
    }

    #[test]
    fn evict_discards_detached_queries() {
        let mut ring = ReplayRing::new();
        // 配色查询随前史一起被驱逐（`ESC [ ? 9 9 6 n` 共 7 字节）。
        ring.push(b"\x1b[?996n");
        ring.push(&vec![b'.'; REPLAY_LIMIT_BYTES - 7]);
        ring.push(b"!");
        // 驱逐已同步 feed 并即时取走，旧查询不堆积。
        assert!(ring.take_queries().is_empty());
        let flat = ring.replay();
        assert!(flat.starts_with(b"."), "查询序列整段丢掉，不留 `996n` 碎片");
    }

    #[test]
    fn alt_screen_set_and_cleared() {
        let mut modes = TerminalModes::new();
        assert!(!modes.is_on(ALT_SCREEN));
        assert!(!modes.is_on(47), "未跟踪的模式恒假");
        modes.feed(b"\x1b[?1049h");
        assert!(modes.is_on(ALT_SCREEN));
        modes.feed(b"\x1b[?1049l");
        assert!(!modes.is_on(ALT_SCREEN));
        // 多参数共存分别处理。
        modes.feed(b"\x1b[?1000;1049h");
        assert!(modes.is_on(ALT_SCREEN));
        // 非跟踪模式与 ANSI 开关不影响。
        modes.feed(b"\x1b[?25l\x1b[4h");
        assert!(modes.is_on(ALT_SCREEN));
        modes.feed(b"\x1b[?1049l");
        assert!(!modes.is_on(ALT_SCREEN));
    }

    #[test]
    fn alt_screen_survives_split_and_ignores_lookalikes() {
        // 跨 push 分批仍识别。
        let mut modes = TerminalModes::new();
        modes.feed(b"\x1b[?10");
        modes.feed(b"49h");
        assert!(modes.is_on(ALT_SCREEN));
        // DECRQM 查询不是开关。
        let mut q = TerminalModes::new();
        q.feed(b"\x1b[?1049$p");
        assert!(!q.is_on(ALT_SCREEN));
        // OSC 负载里的看似序列不是开关。
        let mut o = TerminalModes::new();
        o.feed(b"\x1b]0;\x1b[?1049h\x07");
        assert!(!o.is_on(ALT_SCREEN));
        // RIS 回默认清模式。
        modes.feed(b"\x1bc");
        assert!(!modes.is_on(ALT_SCREEN));
    }

    #[test]
    fn ring_modes_keeps_alt_while_carried() {
        let mut ring = ReplayRing::new();
        ring.push(b"\x1b[?1049h");
        ring.push(b"vim screen");
        assert!(ring.modes().is_on(ALT_SCREEN));
    }

    #[test]
    fn ring_modes_loses_alt_once_evicted() {
        let mut ring = ReplayRing::new();
        ring.push(b"\x1b[?1049h");
        ring.push(&vec![b'.'; REPLAY_LIMIT_BYTES - 8]);
        assert!(ring.modes().is_on(ALT_SCREEN));
        // 溢出致整段 `?1049h` 被驱逐：环内不再开着。
        ring.push(b"!");
        assert!(!ring.modes().is_on(ALT_SCREEN));
    }

    #[test]
    fn ownership_four_transitions() {
        let mut o = Ownership::new();
        // 1. 首个接入成为 controller。
        assert_eq!(
            o.on_attach(1),
            Some(OwnershipEvent::Takeover { new_controller: 1 })
        );
        // 2. 第二个接入只是观察。
        assert_eq!(o.on_attach(2), None);
        assert_eq!(o.controller(), Some(1));
        // 3. 在接者发送输入即接管。
        assert_eq!(
            o.on_input(2),
            Some(OwnershipEvent::Takeover { new_controller: 2 })
        );
        assert!(!o.accepts_resize(1));
        assert!(o.accepts_resize(2));
        // 4. controller 断开后角色为空。
        assert_eq!(o.on_detach(2), Some(OwnershipEvent::Released));
        assert_eq!(o.controller(), None);
        // 空位时发送输入即取得。
        assert_eq!(
            o.on_input(1),
            Some(OwnershipEvent::Takeover { new_controller: 1 })
        );
    }

    #[test]
    fn exit_marks_then_removes() {
        let mut owner = SessionOwner::new();
        owner.insert(SessionMeta {
            id: "s1".to_string(),
            cwd: None,
            title: None,
            program: None,
            controller: None,
            alive: true,
            exit_code: None,
        });
        let meta = owner.mark_exited("s1", 3).unwrap();
        assert!(!meta.alive);
        assert_eq!(meta.exit_code, Some(3));
        owner.remove("s1");
        assert!(owner.get("s1").is_none());
    }

    /// 真值表输入：默认“首画已丢的全屏前台活程序”，各用例只改一位。
    fn nudge_input() -> RedrawNudgeInput {
        RedrawNudgeInput {
            alive: true,
            foreground_program: true,
            alt_now: true,
            alt_in_ring: false,
            size: TermSize { cols: 80, rows: 24 },
            is_observer: false,
        }
    }

    #[test]
    fn nudge_triggers_when_first_paint_lost() {
        let mut last = None;
        let now = Instant::now();
        let out = redraw_nudge(&nudge_input(), &mut last, now);
        // 行位移一行：同尺寸内核不发 `SIGWINCH`，必须错开一行才有重画。
        assert_eq!(out, Some(TermSize { cols: 80, rows: 23 }));
        assert_eq!(last, Some(now), "命中即记录时间，突发重连只请一次");
    }

    #[test]
    fn nudge_dead_session_never() {
        let mut input = nudge_input();
        input.alive = false;
        let mut last = None;
        assert_eq!(redraw_nudge(&input, &mut last, Instant::now()), None);
        assert_eq!(last, None, "未命中不碰时间");
    }

    #[test]
    fn nudge_without_foreground_never() {
        // 前台缺失（`tcgetpgrp` 失败/`None`；Windows 探针恒 `None`）只回放，不请重画。
        assert!(!foreground_command_running(None, Some(1234)));
        assert!(
            !foreground_command_running(Some(100), Some(100)),
            "前台仍是 shell"
        );
        assert!(foreground_command_running(Some(200), Some(100)));
        let mut input = nudge_input();
        input.foreground_program = false;
        let mut last = None;
        assert_eq!(redraw_nudge(&input, &mut last, Instant::now()), None);
        assert_eq!(last, None);
    }

    #[test]
    fn nudge_primary_screen_never() {
        // 主屏 shell 的回放即整屏，零打扰。
        let mut input = nudge_input();
        input.alt_now = false;
        let mut last = None;
        assert_eq!(redraw_nudge(&input, &mut last, Instant::now()), None);
        assert_eq!(last, None);
    }

    #[test]
    fn nudge_first_paint_kept_never() {
        // 首画仍在环内：回放能整屏重建，不请重画。
        let mut input = nudge_input();
        input.alt_in_ring = true;
        let mut last = None;
        assert_eq!(redraw_nudge(&input, &mut last, Instant::now()), None);
        assert_eq!(last, None);
    }

    #[test]
    fn nudge_cooldown_suppresses_burst() {
        let mut last = None;
        let now = Instant::now();
        assert!(redraw_nudge(&nudge_input(), &mut last, now).is_some());
        assert_eq!(
            redraw_nudge(&nudge_input(), &mut last, now + Duration::from_millis(500)),
            None,
            "冷却内的重连不再请（重画已在路上）"
        );
        assert!(
            redraw_nudge(&nudge_input(), &mut last, now + REDRAW_NUDGE_COOLDOWN).is_some(),
            "冷却过后可再请"
        );
    }

    #[test]
    fn nudge_rows_one_goes_taller() {
        // 单行高无处可减：向上错一行，同样只动一行。
        let mut input = nudge_input();
        input.size = TermSize { cols: 80, rows: 1 };
        let mut last = None;
        assert_eq!(
            redraw_nudge(&input, &mut last, Instant::now()),
            Some(TermSize { cols: 80, rows: 2 })
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_probe_never_reports_foreground() {
        // Windows 无 `tcgetpgrp`：恒无前台，恒不 nudge 只回放。
        assert_eq!(pty_foreground_pgid(), None);
    }

    /// 记录每次下发尺寸的 `resize` 闭包（nudge 时序单测用）。
    fn recording_resizer(
        calls: &std::sync::Arc<std::sync::Mutex<Vec<TermSize>>>,
    ) -> impl Fn(TermSize) + Send + 'static {
        let calls = std::sync::Arc::clone(calls);
        move |size: TermSize| {
            calls.lock().expect("calls").push(size);
        }
    }

    #[test]
    fn nudge_applies_transient_then_restores_after_settle() {
        // 合并窗口：临时尺寸立刻下发（不等 50ms），恢复等暂留后。
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let handle = run_redraw_nudge(
            TermSize { cols: 80, rows: 23 },
            Duration::from_millis(1),
            recording_resizer(&calls),
            || Some(TermSize { cols: 80, rows: 24 }),
        )
        .expect("spawn nudge");
        assert_eq!(
            *calls.lock().expect("calls"),
            vec![TermSize { cols: 80, rows: 23 }],
            "返回前临时尺寸已下发，恢复还在暂留窗内"
        );
        handle.join().expect("nudge thread");
        assert_eq!(
            *calls.lock().expect("calls"),
            vec![
                TermSize { cols: 80, rows: 23 },
                TermSize { cols: 80, rows: 24 }
            ],
            "暂留后恢复最新值"
        );
    }

    #[test]
    fn nudge_restores_latest_size_seen_during_settle() {
        // 窗口内真实 `resize` 落盘：恢复读最新值，不覆盖用户尺寸。
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let handle = run_redraw_nudge(
            TermSize { cols: 80, rows: 23 },
            Duration::from_millis(1),
            recording_resizer(&calls),
            || {
                Some(TermSize {
                    cols: 100,
                    rows: 30,
                })
            },
        )
        .expect("spawn nudge");
        handle.join().expect("nudge thread");
        assert_eq!(
            *calls.lock().expect("calls"),
            vec![
                TermSize { cols: 80, rows: 23 },
                TermSize {
                    cols: 100,
                    rows: 30
                }
            ],
            "恢复的是暂留结束时的最新值"
        );
    }

    #[test]
    fn nudge_abandons_when_settled_gone() {
        // 睡眠窗内会话已死：`settled()->None` 则放弃，不再下发第二次。
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let handle = run_redraw_nudge(
            TermSize { cols: 80, rows: 23 },
            Duration::from_millis(1),
            recording_resizer(&calls),
            || None,
        )
        .expect("spawn nudge");
        handle.join().expect("nudge thread");
        assert_eq!(
            *calls.lock().expect("calls"),
            vec![TermSize { cols: 80, rows: 23 }],
            "放弃恢复，只留立刻下发的临时尺寸"
        );
    }

    #[test]
    fn nudge_burst_converges_to_latest() {
        // 连续两次 `attach`：两线程都读最新值恢复，以后到者为准收敛。
        let latest = std::sync::Arc::new(std::sync::Mutex::new(TermSize { cols: 80, rows: 24 }));
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let settled_of = |latest: &std::sync::Arc<std::sync::Mutex<TermSize>>| {
            let latest = std::sync::Arc::clone(latest);
            move || Some(*latest.lock().expect("latest"))
        };
        let first = run_redraw_nudge(
            TermSize { cols: 80, rows: 23 },
            Duration::from_millis(10),
            recording_resizer(&calls),
            settled_of(&latest),
        )
        .expect("spawn first");
        // 窗口内用户真实改尺寸 + 第二次 `attach`（以后到者为准）。
        *latest.lock().expect("latest") = TermSize {
            cols: 100,
            rows: 30,
        };
        let second = run_redraw_nudge(
            TermSize {
                cols: 100,
                rows: 29,
            },
            Duration::from_millis(10),
            recording_resizer(&calls),
            settled_of(&latest),
        )
        .expect("spawn second");
        first.join().expect("first nudge");
        second.join().expect("second nudge");
        let calls = calls.lock().expect("calls").clone();
        assert_eq!(calls.len(), 4, "两次临时 + 两次恢复，got {calls:?}");
        assert_eq!(calls[0], TermSize { cols: 80, rows: 23 });
        assert_eq!(
            calls[1],
            TermSize {
                cols: 100,
                rows: 29
            }
        );
        assert!(
            calls[2..].iter().all(|s| *s
                == TermSize {
                    cols: 100,
                    rows: 30
                }),
            "两次恢复都是最新值，最终停在后到者尺寸，got {calls:?}"
        );
    }

    #[test]
    fn detached_da_replies_vt102_without_hang() {
        // 脱离期 `DA` 代答固定 `VT102`，取走即空，不堆积不 hang。
        let mut modes = TerminalModes::new();
        modes.feed(b"\x1b[c");
        assert_eq!(modes.take_queries(), vec![Query::DeviceAttributes]);
        assert!(modes.take_queries().is_empty());
        assert_eq!(Query::DeviceAttributes.reply_bytes(), DA_REPLY);
        assert_eq!(DA_REPLY, b"\x1b[?6c");
        // 跨块仍识别，`CSI 0 c` 同义。
        let mut split = TerminalModes::new();
        split.feed(b"\x1b[0");
        split.feed(b"c");
        assert_eq!(split.take_queries(), vec![Query::DeviceAttributes]);
    }

    #[test]
    fn detached_osc11_maps_to_dark_theme() {
        // `OSC 11 ; ?` 与 `?996n` 同按深色代答，标题 OSC 不误判。
        let mut modes = TerminalModes::new();
        modes.feed(b"\x1b]11;?\x07");
        assert_eq!(modes.take_queries(), vec![Query::ColorScheme]);
        assert_eq!(Query::ColorScheme.reply_bytes(), DARK_THEME_REPLY);
        assert_eq!(DARK_THEME_REPLY, b"\x1b[?997;1n");
        // `ST` 结尾跨块同样识别。
        let mut st = TerminalModes::new();
        st.feed(b"\x1b]11;?");
        st.feed(b"\x1b\\");
        assert_eq!(st.take_queries(), vec![Query::ColorScheme]);
        // 标题不是查询。
        let mut title = TerminalModes::new();
        title.feed(b"\x1b]0;hi\x07");
        assert!(title.take_queries().is_empty());
    }

    #[test]
    fn detached_queries_drain_and_later_still_correctable() {
        // 攒下即取走，浅色偏差由后续真实查询纠正（新查询仍能识别）。
        let mut modes = TerminalModes::new();
        modes.feed(b"\x1b[c\x1b[?996n");
        let got = modes.take_queries();
        assert_eq!(got, vec![Query::DeviceAttributes, Query::ColorScheme]);
        assert!(modes.take_queries().is_empty());
        modes.feed(b"\x1b[?996n");
        assert_eq!(modes.take_queries(), vec![Query::ColorScheme]);
    }
}

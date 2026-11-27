//! 会话字节馈送：回放环 + OSC 7501 程序状态 + OSC 7 cwd，同批推进。
//!
//! - 三者都从同一批 PTY 原始字节派生，顺序一致，不存在"环里有、状态里无"的撕裂。
//! - 程序状态与 cwd 只影响会话表（`ls`、固定标签恢复），标题/提示符/链接仍由
//!   各客户端从字节流自己算（D3）。
//! - 回放不重复触发状态变化：重连客户端拿到的是字节回放，`FeedState` 不重放
//!   `StatusUpdate`（规格 R3 的"不重复触发"由"状态只进、字节只读"保证）。

use crossh_core::terminal::{Osc7501Scanner, ProgramState, ProgramStatus, StatusUpdate};

use crate::session::{
    ALT_SCREEN, Query, RedrawNudgeInput, ReplayRing, RingSegment, TerminalModes, redraw_nudge,
};
use crate::workspace::TermSize;

/// 一次馈送的派生结果（会话表更新用）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeedUpdate {
    /// 本批结束时是否有新的 cwd。
    pub cwd: Option<String>,
    /// 本批结束时的程序状态（`None` 表示清除/未知）。
    pub program: Option<ProgramStatus>,
    /// 本批是否改变了任一字段。
    pub changed: bool,
}

/// 会话输出的服务端侧全量：环 + 扫描器 + 同步序号。
///
/// `seq` 为已推入总字节数；attach 流程用它对齐回放与实时流：
/// 先订阅广播，再快照 `(seq, bytes)`，回放快照后只转发 `end_seq > seq` 的块，
/// 不丢不重（推送与序号在同一把锁内先后发生）。
#[derive(Debug, Default)]
pub struct FeedState {
    ring: ReplayRing,
    /// 全量折叠：与环内字节同一批推进（含已被驱逐的前史），`attach` 时对照环内判定首画去留。
    modes: TerminalModes,
    /// 上次发起重画请求的时间：冷却内重连不再发起（突发重连只请一次）。
    last_redraw_nudge: Option<std::time::Instant>,
    /// 提示符停留记忆：`133;A/B` 置位，`133;C/D` 清零（3.3 新增，存 `FeedState`）。
    prompt: PromptTracker,
    scanner: Osc7501Scanner,
    osc7: Osc7Scanner,
    cwd: Option<String>,
    program: Option<ProgramStatus>,
    seq: u64,
}

impl FeedState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 推入一批 PTY 输出，返回派生的会话表更新。
    pub fn push(&mut self, bytes: &[u8]) -> FeedUpdate {
        self.ring.push(bytes);
        // 与环内字节同一批折叠：全量记得前史，环只记得现存，对照即知首画去留。
        self.modes.feed(bytes);
        // 同批记提示符：`133` 残留后续以前台为准（`replayed_at_prompt`）。
        self.prompt.feed(bytes);
        self.seq += bytes.len() as u64;
        let mut changed = false;
        for update in self.scanner.feed(bytes) {
            match update {
                StatusUpdate::Set(status) => {
                    if self.program.as_ref() != Some(&status) {
                        self.program = Some(status);
                        changed = true;
                    }
                }
                StatusUpdate::Clear => {
                    if self.program.is_some() {
                        self.program = None;
                        changed = true;
                    }
                }
                // 特性探测由客户端 VT 回答（D2 出向通道），服务端忽略。
                StatusUpdate::Query => {}
            }
        }
        let mut update = FeedUpdate {
            cwd: None,
            program: self.program.clone(),
            changed,
        };
        if let Some(cwd) = self.osc7.feed(bytes)
            && self.cwd.as_ref() != Some(&cwd)
        {
            self.cwd = Some(cwd.clone());
            update.cwd = Some(cwd);
            update.changed = true;
        }
        update
    }

    pub fn snapshot(&self) -> (u64, Vec<u8>) {
        (self.seq, self.ring.replay())
    }

    /// 尺寸变化时封段（只记几何，不动序号与扫描器）。
    pub fn resize(&mut self, size: TermSize) {
        self.ring.resize(size);
    }

    /// 分段快照：序号语义与 `snapshot()` 一致，字节按段各带几何。
    pub fn snapshot_segments(&self) -> (u64, Vec<RingSegment>) {
        (self.seq, self.ring.segments())
    }

    /// 重连前导：全量开着、环内已无的模式才补（现仅进备用屏一切换）。
    /// 复用 `Output` 发出，环自带同样切换时返回 `None`（不重复）。
    pub fn replay_prelude(&self) -> Option<Vec<u8>> {
        self.modes.restore_bytes_beyond(&self.ring.modes())
    }

    /// 取走全量折叠攒下的查询（有订阅由客户端应答，无订阅 3.3 代答；驱逐路径已即时丢弃）。
    pub fn take_queries(&mut self) -> Vec<Query> {
        self.modes.take_queries()
    }

    /// 当前是否在 `alt-screen`（全量折叠，含已驱逐前史）。
    pub fn alt_screen_now(&self) -> bool {
        self.modes.is_on(ALT_SCREEN)
    }

    /// 环内是否还留着进入 `alt-screen` 的切换（只看现存字节）。
    /// `attach` 判定：开着但环内已无，即首画已丢（2.2 才据此发起 nudge）。
    pub fn alt_screen_in_ring(&self) -> bool {
        self.ring.modes().is_on(ALT_SCREEN)
    }

    /// 存量是否停在提示符（`133;A/B` 置位，`133;C/D` 清零，全屏多为 `C` 故不误判）。
    pub fn stored_at_prompt(&self) -> bool {
        self.prompt.is_at_prompt()
    }

    /// 重放是否声称停在提示符：存量在提示符且前台无程序才算（`tty7#711` 防护）。
    /// - 全屏跑起后前台非 shell：即使环里残留 `133;B` 也不采信，客户端保留重建的屏。
    /// - `foreground` 问内核（Unix `tcgetpgrp`，Windows 恒 false 只回放）。
    pub fn replayed_at_prompt(&self, foreground_program: bool) -> bool {
        self.stored_at_prompt() && !foreground_program
    }

    /// 重连重画判定包装：从本 feed 组装输入，命中返回 PTY 临时尺寸并记录时间。
    ///
    /// - `alive` 由会话存活给；`foreground_program` 由 `tcgetpgrp` 探针给，失败、`None`、
    ///   Windows 一律按无前台处理，只回放。
    /// - observer 与 controller 同权：`is_observer` 不影响判定，
    ///   只动 PTY 不改布局（与 observer 静默规则正交）。
    /// - 返回的临时尺寸只给 PTY 用：不写环分段、不发客户端 `Size`（3.1 落地）。
    pub fn poll_redraw_nudge(
        &mut self,
        alive: bool,
        foreground_program: bool,
        is_observer: bool,
        now: std::time::Instant,
    ) -> Option<TermSize> {
        let input = RedrawNudgeInput {
            alive,
            foreground_program,
            alt_now: self.alt_screen_now(),
            alt_in_ring: self.alt_screen_in_ring(),
            size: self.ring.size(),
            is_observer,
        };
        redraw_nudge(&input, &mut self.last_redraw_nudge, now)
    }

    /// 当前尾段几何（nudge 恢复读最新值用；临时尺寸从不进环）。
    pub fn current_size(&self) -> TermSize {
        self.ring.size()
    }

    /// 已推入总字节数（广播块的 `end_seq` 即推入后的该值）。
    pub fn seq(&self) -> u64 {
        self.seq
    }

    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }

    pub fn program(&self) -> Option<&ProgramStatus> {
        self.program.as_ref()
    }
}

/// 程序状态名（会话表 `ls` 显示用，不做结构化透出）。
pub fn program_state_name(state: ProgramState) -> &'static str {
    match state {
        ProgramState::Idle => "idle",
        ProgramState::Working => "working",
        ProgramState::Done => "done",
        ProgramState::Blocked => "blocked",
        ProgramState::Error => "error",
    }
}

/// 提示符停留记忆：`133;A/B` 置位、`133;C/D` 清零（3.3 新增，存 `FeedState`）。
///
/// - 与环内字节同批推进，跨 `push` 续接；`ST` 按 `ESC \` 两字节认（含跨块）。
/// - 只看 `133;` 首字母：`A/B` 在提示符，`C` 进命令、`D` 命令结束（仍不在提示符）。
/// - 全屏程序进入后多为 `C`：存量不会误报在提示符（`tty7#711`）。
#[derive(Debug, Default)]
struct PromptTracker {
    at_prompt: bool,
    state: PromptState,
    buf: Vec<u8>,
}

#[derive(Debug, Default, PartialEq, Eq)]
enum PromptState {
    #[default]
    Ground,
    Esc,
    Body,
    EscBody,
}

impl PromptTracker {
    fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            match self.state {
                PromptState::Ground => {
                    if b == 0x1b {
                        self.state = PromptState::Esc;
                    }
                }
                PromptState::Esc => {
                    if b == b']' {
                        self.buf.clear();
                        self.state = PromptState::Body;
                    } else {
                        self.state = PromptState::Ground;
                    }
                }
                PromptState::Body => match b {
                    0x07 => {
                        self.apply();
                        self.state = PromptState::Ground;
                    }
                    0x1b => self.state = PromptState::EscBody,
                    _ => {
                        if self.buf.len() < 64 {
                            self.buf.push(b);
                        }
                    }
                },
                PromptState::EscBody => match b {
                    b'\\' => {
                        self.apply();
                        self.state = PromptState::Ground;
                    }
                    0x1b => {}
                    _ => self.state = PromptState::Body,
                },
            }
        }
    }

    /// 终止符到达：`133;A/B` 置位，`133;C/D` 清零，其余忽略。
    fn apply(&mut self) {
        if self.buf.len() >= 5 && self.buf[..4] == *b"133;" {
            match self.buf[4] {
                b'A' | b'B' => self.at_prompt = true,
                b'C' | b'D' => self.at_prompt = false,
                _ => {}
            }
        }
        self.buf.clear();
    }

    fn is_at_prompt(&self) -> bool {
        self.at_prompt
    }
}

/// 最小 OSC 7 扫描器：`ESC ] 7 ; <url> (BEL | ESC \)` → 路径。
///
/// 只认 `file://<host><path>` 形态（集成脚本的发射格式）；其他形态原样返回
/// payload，调用方按需处理。
#[derive(Debug, Default)]
struct Osc7Scanner {
    state: Osc7State,
    /// `ESC ]` 之后收集的 OSC 编号（跨 chunk，最多 8 位）。
    num: Vec<u8>,
    body: Vec<u8>,
}

#[derive(Debug, Default, PartialEq, Eq)]
enum Osc7State {
    #[default]
    Ground,
    Esc,
    OscNum,
    Body,
    EscBody,
}

impl Osc7Scanner {
    /// 喂入一批字节，返回本批内最后一个完整的 cwd（无则 `None`）。
    fn feed(&mut self, bytes: &[u8]) -> Option<String> {
        let mut found = None;
        for &b in bytes {
            if let Some(cwd) = self.step(b) {
                found = Some(cwd);
            }
        }
        found
    }

    fn step(&mut self, b: u8) -> Option<String> {
        match self.state {
            Osc7State::Ground => {
                if b == 0x1b {
                    self.state = Osc7State::Esc;
                }
            }
            Osc7State::Esc => {
                if b == b']' {
                    self.num.clear();
                    self.state = Osc7State::OscNum;
                } else {
                    self.state = Osc7State::Ground;
                }
            }
            Osc7State::OscNum => match b {
                // 只认 `ESC ] 7 ;`；133/7501 等另有归属。
                b';' => {
                    if self.num.as_slice() == b"7" {
                        self.body.clear();
                        self.state = Osc7State::Body;
                    } else {
                        self.state = Osc7State::Ground;
                    }
                }
                _ if b.is_ascii_digit() && self.num.len() < 8 => {
                    self.num.push(b);
                    self.state = Osc7State::OscNum;
                }
                _ => self.state = Osc7State::Ground,
            },
            Osc7State::Body => match b {
                0x07 => {
                    self.state = Osc7State::Ground;
                    return Self::path_of(&self.body);
                }
                0x1b => self.state = Osc7State::EscBody,
                _ => {
                    if self.body.len() < 4096 {
                        self.body.push(b);
                    }
                }
            },
            Osc7State::EscBody => {
                self.state = Osc7State::Ground;
                if b == b'\\' {
                    return Self::path_of(&self.body);
                }
            }
        }
        None
    }

    fn path_of(body: &[u8]) -> Option<String> {
        let text = std::str::from_utf8(body).ok()?;
        // `file://<host><path>` → `<path>`；非 file 形态原样返回。
        if let Some(rest) = text.strip_prefix("file://") {
            let path = rest.find('/').map(|i| &rest[i..]).unwrap_or("");
            if path.is_empty() {
                return None;
            }
            return Some(path.to_string());
        }
        if text.is_empty() {
            None
        } else {
            Some(text.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_bytes_update_cwd_and_program() {
        let mut feed = FeedState::new();
        // 先一段普通输出：无更新。
        let update = feed.push(b"hello");
        assert!(!update.changed);
        assert_eq!(feed.cwd(), None);
        // OSC 7 上报 cwd（BEL 结尾）。
        let update = feed.push(b"\x1b]7;file://localhost/tmp/work\x07");
        assert!(update.changed);
        assert_eq!(update.cwd.as_deref(), Some("/tmp/work"));
        assert_eq!(feed.cwd(), Some("/tmp/work"));
        // OSC 7501 上报 working（ST 结尾，跨 chunk 切分也必须成立）。
        let update = feed.push(b"\x1b]7501;state=working:app=");
        assert!(!update.changed);
        let update = feed.push(b"cargo\x1b\\");
        assert!(update.changed);
        let program = feed.program().expect("program");
        assert_eq!(program.state, ProgramState::Working);
        assert_eq!(program.app.as_deref(), Some("cargo"));
        // Clear 清除状态。
        let update = feed.push(b"\x1b]7501;state=clear\x1b\\");
        assert!(update.changed);
        assert_eq!(feed.program(), None);
        // 回放环里三段都在。
        let (seq, replay) = feed.snapshot();
        assert_eq!(seq as usize, replay.len());
        assert!(replay.windows(5).any(|w| w == b"hello"));
    }

    #[test]
    fn osc7_esc_backslash_terminator() {
        let mut feed = FeedState::new();
        let update = feed.push(b"\x1b]7;file://host/home/u\x1b\\");
        assert_eq!(update.cwd.as_deref(), Some("/home/u"));
    }

    #[test]
    fn non_seven_osc_ignored() {
        let mut feed = FeedState::new();
        // 133 / 9 等编号另有归属，不得污染 cwd。
        let update = feed.push(b"\x1b]133;D;0\x07\x1b]9;/tmp/plain\x07");
        assert_eq!(update.cwd, None);
        assert_eq!(feed.cwd(), None);
    }

    #[test]
    fn feed_tracks_alt_screen_with_same_batch() {
        let mut feed = FeedState::new();
        feed.push(b"\x1b[?1049hhello");
        // 同批推进：当前开着且环内仍有切换，即在环判定。
        assert!(feed.alt_screen_now());
        assert!(feed.alt_screen_in_ring());
        // 查询照常攒下（全量折叠产出，3.3 脱离期代答用）。
        let update = feed.push(b"\x1b[?996n");
        assert!(!update.changed);
        assert_eq!(feed.take_queries(), vec![Query::ColorScheme]);
        // 退出备用屏：全量与环内一并关闭。
        feed.push(b"\x1b[?1049l");
        assert!(!feed.alt_screen_now());
        assert!(!feed.alt_screen_in_ring());
    }

    #[test]
    fn evicted_alt_entry_means_first_paint_lost() {
        use crate::session::REPLAY_LIMIT_BYTES;
        let mut feed = FeedState::new();
        feed.push(b"\x1b[?1049h");
        feed.push(&vec![b'.'; REPLAY_LIMIT_BYTES - 8]);
        assert!(feed.alt_screen_now());
        assert!(feed.alt_screen_in_ring());
        // 溢出致 `?1049h` 被驱逐：当前仍开着，环内已无，即丢首画判定。
        feed.push(b"!");
        assert!(feed.alt_screen_now());
        assert!(!feed.alt_screen_in_ring());
    }

    #[test]
    fn snapshot_segments_carry_geometry() {
        use crate::workspace::TermSize;
        let mut feed = FeedState::new();
        feed.push(b"a");
        feed.resize(TermSize {
            cols: 100,
            rows: 30,
        });
        feed.push(b"b");
        let (seq, segs) = feed.snapshot_segments();
        // 序号语义与单流快照一致：已推入总字节数。
        assert_eq!(seq, 2);
        assert_eq!(feed.snapshot(), (2, b"ab".to_vec()));
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].size, TermSize::default());
        assert_eq!(segs[0].bytes, b"a");
        assert_eq!(
            segs[1].size,
            TermSize {
                cols: 100,
                rows: 30
            }
        );
        assert_eq!(segs[1].bytes, b"b");
    }

    /// 首画已丢的 feed：当前在备用屏，环内已无切换（nudge 触发态）。
    fn feed_with_lost_first_paint() -> FeedState {
        use crate::session::REPLAY_LIMIT_BYTES;
        let mut feed = FeedState::new();
        feed.push(b"\x1b[?1049h");
        feed.push(&vec![b'.'; REPLAY_LIMIT_BYTES - 8]);
        feed.push(b"!");
        assert!(feed.alt_screen_now());
        assert!(!feed.alt_screen_in_ring(), "切换随前史被驱逐，即首画已丢");
        feed
    }

    #[test]
    fn observer_nudge_matches_controller_without_touching_layout() {
        use std::time::Instant;
        // observer 重连同样可触发 PTY 侧请求，且与 controller 结果一致。
        for is_observer in [false, true] {
            let mut feed = feed_with_lost_first_paint();
            let before = feed.snapshot_segments();
            let out = feed.poll_redraw_nudge(true, true, is_observer, Instant::now());
            assert_eq!(
                out,
                Some(TermSize { cols: 80, rows: 23 }),
                "is_observer={is_observer}"
            );
            // 只动 PTY：环分段与几何原样不动，布局无影响。
            assert_eq!(feed.snapshot_segments(), before);
        }
    }

    #[test]
    fn feed_nudge_rows_one_goes_taller() {
        use std::time::Instant;
        let mut feed = feed_with_lost_first_paint();
        feed.resize(TermSize { cols: 80, rows: 1 });
        let before = feed.snapshot_segments();
        let out = feed.poll_redraw_nudge(true, true, false, Instant::now());
        // 单行高向上错一行（`rows+1`），同样只动一行。
        assert_eq!(out, Some(TermSize { cols: 80, rows: 2 }));
        // 临时尺寸不进环：分段快照原样不动。
        assert_eq!(feed.snapshot_segments(), before);
    }

    #[test]
    fn feed_nudge_cooldown_and_quiet_states() {
        use crate::session::REDRAW_NUDGE_COOLDOWN;
        use std::time::{Duration, Instant};
        let mut feed = feed_with_lost_first_paint();
        let now = Instant::now();
        assert!(feed.poll_redraw_nudge(true, true, false, now).is_some());
        assert_eq!(
            feed.poll_redraw_nudge(true, true, false, now + Duration::from_millis(500)),
            None,
            "冷却内只请一次"
        );
        assert!(
            feed.poll_redraw_nudge(true, true, false, now + REDRAW_NUDGE_COOLDOWN)
                .is_some(),
            "冷却过后可再请"
        );
        // 主屏 / 无前台 / 死会话：只回放，不请重画。
        let mut quiet = FeedState::new();
        quiet.push(b"shell output");
        assert_eq!(
            quiet.poll_redraw_nudge(true, false, false, Instant::now()),
            None,
            "无前台（Windows 恒走此分支）不触发"
        );
        assert_eq!(
            quiet.poll_redraw_nudge(false, true, false, Instant::now()),
            None,
            "死会话不触发"
        );
    }

    #[test]
    fn replay_prelude_only_when_first_paint_lost() {
        // 主屏：无前导。
        let mut plain = FeedState::new();
        plain.push(b"hi");
        assert_eq!(plain.replay_prelude(), None);
        // 首画仍在环内：环自带切换，不重复补。
        let mut kept = FeedState::new();
        kept.push(b"\x1b[?1049hvim");
        assert_eq!(kept.replay_prelude(), None);
        // 首画已丢（全量开着、环内已无）：补进屏一切换，复用 `Output` 发出。
        assert_eq!(
            feed_with_lost_first_paint().replay_prelude(),
            Some(b"\x1b[?1049h".to_vec())
        );
    }

    #[test]
    fn detached_queries_drain_without_hang() {
        // 脱离期查询取走即空，不堆积不 hang；有订阅留给客户端，此处只验不堆。
        let mut feed = FeedState::new();
        feed.push(b"\x1b[c\x1b]11;?\x07");
        let got = feed.take_queries();
        assert_eq!(got.len(), 2, "DA + OSC11 各一");
        assert!(feed.take_queries().is_empty());
    }

    #[test]
    fn fullscreen_not_mistaken_for_prompt() {
        // 存量 `133;B` 残留 + 前台有程序：不采信在提示符，保留重建的屏。
        let mut feed = FeedState::new();
        feed.push(b"\x1b]133;B\x07");
        assert!(feed.stored_at_prompt());
        assert!(feed.replayed_at_prompt(false));
        assert!(!feed.replayed_at_prompt(true));
        // 全屏进入（`C` + 备用屏）：存量即不在提示符，前后台都不采信。
        let mut full = FeedState::new();
        full.push(b"\x1b]133;B\x07\x1b]133;C\x07\x1b[?1049h");
        assert!(!full.stored_at_prompt());
        assert!(!full.replayed_at_prompt(false));
        assert!(!full.replayed_at_prompt(true));
        // 跨块 `ST` 结尾同样识别。
        let mut split = FeedState::new();
        split.push(b"\x1b]133;A");
        split.push(b"\x1b\\");
        assert!(split.stored_at_prompt());
    }

    #[test]
    fn shallow_bias_correctable_by_later_query() {
        // 固定深色代答后，后续真实查询仍能识别纠正（浅色不永久跑偏）。
        let mut feed = FeedState::new();
        feed.push(b"\x1b[?996n");
        assert_eq!(feed.take_queries().len(), 1);
        assert!(feed.take_queries().is_empty());
        feed.push(b"\x1b[?996n");
        assert_eq!(feed.take_queries().len(), 1, "新查询仍可被客户端纠正");
    }
}

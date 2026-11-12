//! 会话字节馈送：回放环 + OSC 7501 程序状态 + OSC 7 cwd，同批推进。
//!
//! - 三者都从同一批 PTY 原始字节派生，顺序一致，不存在"环里有、状态里无"的撕裂。
//! - 程序状态与 cwd 只影响会话表（`ls`、固定标签恢复），标题/提示符/链接仍由
//!   各客户端从字节流自己算（D3）。
//! - 回放不重复触发状态变化：重连客户端拿到的是字节回放，`FeedState` 不重放
//!   `StatusUpdate`（规格 R3 的"不重复触发"由"状态只进、字节只读"保证）。

use crossh_core::terminal::{Osc7501Scanner, ProgramState, ProgramStatus, StatusUpdate};

use crate::session::ReplayRing;

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
}

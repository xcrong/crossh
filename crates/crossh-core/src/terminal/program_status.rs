//! OSC 7501「Program Status Protocol」解析。
//!
//! 规范：<https://www.superlogical.com/rex/docs/build/program-status>（v0.4，2026-10-10）。
//! 序列格式：`ESC ] 7501 ; <k=v> (":" k=v)* (BEL | ESC \)`，body 为 `:` 分隔的
//! `key=value` 对。MVP 裁剪：只维护根记录（带 `id` 的子记录整体忽略）、不做
//! OSC 9;4 映射与通知限频；特性探测仅回复 `?`（等价于五个状态 + 三种 kind）。

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

/// 规范上限：整条序列（OSC 到 ST）4096 字节，超限整条丢弃。
const MAX_SEQUENCE_BYTES: usize = 4096;
/// 规范上限：`msg` 解码后 2048 字节，超限整条丢弃（编码长度已被序列上限覆盖）。
const MAX_MSG_DECODED_BYTES: usize = 2048;

/// 特性探测（`OSC 7501 ; ? ST`）的回复。按规范，不带 `states`/`kinds` 即表示
/// 恰好支持 `idle/working/done/blocked/error` 与 `permission/question/auth`。
pub const FEATURE_DETECTION_REPLY: &str = "\x1b]7501;?\x1b\\";

/// 程序状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramState {
    /// 静止，等待用户下一步指示。
    Idle,
    /// 运行中，可能带 `progress`。
    Working,
    /// 无法继续，等待用户操作。
    Blocked,
    /// 完成，结果尚未被用户查看；随进程退出与提示符存活。
    Done,
    /// 失败并停止；随进程退出与提示符存活。
    Error,
}

/// 一次根记录状态报告。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramStatus {
    pub state: ProgramState,
    /// 稳定的程序标识（如 `cargo`、`claude-code`）。
    pub app: Option<String>,
    /// base64 解码后的人类可读单行消息。
    pub msg: Option<String>,
    /// 0-100，仅 `working`/`blocked` 有效；缺省表示不定进度。
    pub progress: Option<u8>,
}

/// 一条 OSC 7501 报告解析后的动作。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatusUpdate {
    /// 根记录被设置为该状态。
    Set(ProgramStatus),
    /// `state=clear`：清除状态。
    Clear,
    /// `OSC 7501 ; ? ST` 特性探测，终端必须回复 [`FEATURE_DETECTION_REPLY`]。
    Query,
}

/// value 字符集：`[A-Za-z0-9_.,+/=-]`（规范保证无需转义）。
fn is_value_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b',' | b'+' | b'/' | b'=' | b'-')
}

/// `app` 字符集：`[A-Za-z0-9_.+-]{1,32}`，不合法视为缺失。
fn is_app_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'+' | b'-')
}

/// 解析一条 OSC 7501 的 body（`7501;` 与终止符之间的部分）。
///
/// 规则：`state` 缺失或未识别 → 整条忽略；畸形 pair（无 `=`、空 key、非法字节）
/// 跳过且不影响其余部分；重复 key 后者覆盖；base64 解码失败或解码后含控制字符
/// → 整条丢弃；MVP 下带 `id` 的报告整体忽略。
pub fn parse_program_status(body: &str) -> Option<StatusUpdate> {
    if body.len() > MAX_SEQUENCE_BYTES {
        return None;
    }
    // body 整体为 `?` 是特性探测
    if body.trim() == "?" {
        return Some(StatusUpdate::Query);
    }

    #[derive(Clone, Copy, PartialEq)]
    enum ParsedState {
        Idle,
        Working,
        Done,
        Blocked,
        Error,
        Clear,
    }

    let mut state: Option<ParsedState> = None;
    let mut app: Option<String> = None;
    let mut msg_raw: Option<&str> = None;
    let mut progress_raw: Option<&str> = None;
    let mut has_id = false;

    for pair in body.split(':') {
        let pair = pair.trim();
        let Some((key, value)) = pair.split_once('=') else {
            continue; // 畸形 pair：跳过，其余照常处理
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_lowercase()) {
            continue;
        }
        if !value.bytes().all(is_value_byte) {
            continue; // 值字符集外：pair 跳过
        }
        match key {
            "state" => {
                state = Some(match value {
                    "idle" => ParsedState::Idle,
                    "working" => ParsedState::Working,
                    "done" => ParsedState::Done,
                    "blocked" => ParsedState::Blocked,
                    "error" => ParsedState::Error,
                    "clear" => ParsedState::Clear,
                    // 未识别的 state：整条报告忽略，避免降级成 idle
                    _ => return None,
                });
            }
            "id" => has_id = true,
            "app" => {
                app = if !value.is_empty() && value.len() <= 32 && value.bytes().all(is_app_byte) {
                    Some(value.to_string())
                } else {
                    None // 不合法视为缺失
                };
            }
            "msg" => msg_raw = Some(value),
            "progress" => progress_raw = Some(value),
            // 未知 key 一律忽略（协议扩展点）
            _ => {}
        }
    }

    if has_id {
        // ponytail: 层级子记录需要 HashMap<id, ProgramStatus> 与父级 app 继承，
        // 待出现真实多记录场景再升级；根记录展示不受子记录影响，故整体忽略。
        return None;
    }
    let state = state?;

    let msg = match msg_raw {
        Some(encoded) => {
            let Ok(bytes) = BASE64.decode(encoded) else {
                return None; // base64 解码失败：整条丢弃
            };
            let Ok(text) = String::from_utf8(bytes) else {
                return None;
            };
            if text.len() > MAX_MSG_DECODED_BYTES || text.chars().any(char::is_control) {
                return None; // 超限或含控制字符：整条丢弃
            }
            Some(text)
        }
        None => None,
    };

    if state == ParsedState::Clear {
        return Some(StatusUpdate::Clear);
    }

    let state = match state {
        ParsedState::Idle => ProgramState::Idle,
        ParsedState::Working => ProgramState::Working,
        ParsedState::Done => ProgramState::Done,
        ParsedState::Blocked => ProgramState::Blocked,
        ParsedState::Error => ProgramState::Error,
        ParsedState::Clear => unreachable!(),
    };
    let progress = progress_raw
        .and_then(|v| v.parse::<u8>().ok())
        .filter(|p| *p <= 100)
        // progress 仅对 working/blocked 有意义，其余视为缺失
        .filter(|_| matches!(state, ProgramState::Working | ProgramState::Blocked));

    Some(StatusUpdate::Set(ProgramStatus {
        state,
        app,
        msg,
        progress,
    }))
}

/// 流式扫描 PTY 输出中的 OSC 7501 序列。
///
/// 扫描器只旁路观察：非 7501 字节不复制不缓冲，[`Osc7501Scanner::feed`]
/// 返回本批字节中完成的报告；字节流本身由调用方原样转发给终端解析器。
#[derive(Debug, Default)]
pub struct Osc7501Scanner {
    state: ScanState,
    /// `ESC ]` 之后收集的 OSC 编号（跨 chunk）。
    num: Vec<u8>,
    /// 7501 的 body（跨 chunk）。
    body: Vec<u8>,
}

#[derive(Debug, Default, PartialEq)]
enum ScanState {
    /// 普通直通。
    #[default]
    Ground,
    /// Ground 中刚看到 ESC。
    Esc,
    /// 看到 `ESC ]`，正在收集 OSC 编号。
    OscNum,
    /// 编号为 7501 且已见 `;`，收集 body。
    Body,
    /// 其他 OSC，跳过到终止符（不缓冲）。
    Bypass,
    /// Body 中看到 ESC，等待 `\`（ST）。
    EscBody,
    /// Bypass 中看到 ESC，等待 `\`（ST）。
    EscBypass,
}

/// OSC 编号缓冲上限：超过 4 位必然不是 7501，8 位封顶防无限缓冲。
const MAX_OSC_NUM_BYTES: usize = 8;

impl Osc7501Scanner {
    /// 喂入一批 PTY 字节，返回其中完成的报告（可能为空，也可能多条）。
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<StatusUpdate> {
        let mut updates = Vec::new();
        for &b in bytes {
            self.step(b, &mut updates);
        }
        updates
    }

    fn step(&mut self, b: u8, updates: &mut Vec<StatusUpdate>) {
        self.state = match self.state {
            ScanState::Ground => {
                if b == 0x1b {
                    ScanState::Esc
                } else {
                    ScanState::Ground
                }
            }
            ScanState::Esc => {
                if b == b']' {
                    self.num.clear();
                    ScanState::OscNum
                } else {
                    ScanState::Ground
                }
            }
            ScanState::OscNum => match b {
                b';' => {
                    if self.num.as_slice() == b"7501" {
                        self.body.clear();
                        ScanState::Body
                    } else {
                        ScanState::Bypass
                    }
                }
                0x07 | 0x1b => ScanState::Ground, // 编号后直接终止/中断，无 body
                _ if b.is_ascii_digit() && self.num.len() < MAX_OSC_NUM_BYTES => {
                    self.num.push(b);
                    ScanState::OscNum
                }
                _ => ScanState::Ground,
            },
            ScanState::Body => match b {
                0x07 => {
                    self.finish(updates);
                    ScanState::Ground
                }
                0x1b => ScanState::EscBody,
                _ => {
                    // 超限：停止缓冲，继续消费到终止符后整条丢弃
                    if self.body.len() <= MAX_SEQUENCE_BYTES {
                        self.body.push(b);
                    }
                    ScanState::Body
                }
            },
            ScanState::Bypass => match b {
                0x07 => ScanState::Ground,
                0x1b => ScanState::EscBypass,
                _ => ScanState::Bypass,
            },
            ScanState::EscBody => {
                if b == b'\\' {
                    self.finish(updates);
                    ScanState::Ground
                } else if b == 0x1b {
                    ScanState::EscBody // ESC ESC \ 仍是合法 ST
                } else {
                    // ESC 后跟其他字节：本 OSC 被中断，该 ESC 开启新序列
                    if b == b']' {
                        self.num.clear();
                        ScanState::OscNum
                    } else {
                        ScanState::Ground
                    }
                }
            }
            ScanState::EscBypass => {
                if b == b'\\' {
                    ScanState::Ground
                } else if b == 0x1b {
                    ScanState::EscBypass
                } else if b == b']' {
                    self.num.clear();
                    ScanState::OscNum
                } else {
                    ScanState::Ground
                }
            }
        };
    }

    /// 终止符到达：解析缓冲的 body（超限丢弃）。
    fn finish(&mut self, updates: &mut Vec<StatusUpdate>) {
        if self.body.len() > MAX_SEQUENCE_BYTES {
            return;
        }
        if let Ok(body) = std::str::from_utf8(&self.body)
            && let Some(update) = parse_program_status(body)
        {
            updates.push(update);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(state: ProgramState) -> StatusUpdate {
        StatusUpdate::Set(ProgramStatus {
            state,
            app: None,
            msg: None,
            progress: None,
        })
    }

    /// 将多段字节依次喂入同一个扫描器，收集全部更新。
    fn scan(chunks: &[&[u8]]) -> Vec<StatusUpdate> {
        let mut scanner = Osc7501Scanner::default();
        let mut updates = Vec::new();
        for chunk in chunks {
            updates.extend(scanner.feed(chunk));
        }
        updates
    }

    #[test]
    fn full_sequence_with_st() {
        let updates = scan(&[b"\x1b]7501;state=working:app=cargo\x1b\\"]);
        assert_eq!(
            updates,
            vec![StatusUpdate::Set(ProgramStatus {
                state: ProgramState::Working,
                app: Some("cargo".into()),
                msg: None,
                progress: None,
            })]
        );
    }

    #[test]
    fn full_sequence_with_bel() {
        assert_eq!(
            scan(&[b"\x1b]7501;state=idle\x07"]),
            vec![set(ProgramState::Idle)]
        );
    }

    #[test]
    fn sequence_before_and_after_text_is_detected() {
        let updates = scan(&[b"hello \x1b]7501;state=done\x1b\\ world"]);
        assert_eq!(updates, vec![set(ProgramState::Done)]);
    }

    #[test]
    fn split_across_chunks() {
        // 切在编号中间 / body 中间 / 终止符中间
        let cases: [&[&[u8]]; 3] = [
            &[b"\x1b]75", b"01;state=working\x1b\\"],
            &[b"\x1b]7501;state=wor", b"king\x1b\\"],
            &[b"\x1b]7501;state=working\x1b", b"\\"],
        ];
        for chunks in cases {
            assert_eq!(scan(chunks), vec![set(ProgramState::Working)], "{chunks:?}");
        }
    }

    #[test]
    fn query_detection() {
        assert_eq!(scan(&[b"\x1b]7501;?\x1b\\"]), vec![StatusUpdate::Query]);
        assert_eq!(
            scan(&[b"\x1b]7501;state=working:?\x1b\\"]),
            vec![set(ProgramState::Working)]
        );
    }

    #[test]
    fn clear_removes_status() {
        assert_eq!(
            scan(&[b"\x1b]7501;state=clear\x1b\\"]),
            vec![StatusUpdate::Clear]
        );
    }

    #[test]
    fn unknown_state_ignored() {
        assert!(scan(&[b"\x1b]7501;state=building\x1b\\"]).is_empty());
    }

    #[test]
    fn missing_state_ignored() {
        assert!(scan(&[b"\x1b]7501;app=cargo\x1b\\"]).is_empty());
        assert!(scan(&[b"\x1b]7501;\x1b\\"]).is_empty());
    }

    #[test]
    fn progress_only_counts_for_working_and_blocked() {
        let updates = scan(&[b"\x1b]7501;state=working:progress=40\x1b\\"]);
        assert_eq!(
            updates,
            vec![StatusUpdate::Set(ProgramStatus {
                state: ProgramState::Working,
                app: None,
                msg: None,
                progress: Some(40),
            })]
        );
        // 非 working/blocked：progress 视为缺失，报告本身仍生效
        let updates = scan(&[b"\x1b]7501;state=done:progress=40\x1b\\"]);
        assert_eq!(updates, vec![set(ProgramState::Done)]);
        // 越界/非数字：视为缺失
        for body in ["state=blocked:progress=150", "state=blocked:progress=abc"] {
            let updates = scan(&[format!("\x1b]7501;{body}\x1b\\").as_bytes()]);
            assert_eq!(updates, vec![set(ProgramState::Blocked)], "{body}");
        }
    }

    #[test]
    fn unknown_keys_and_malformed_pairs_are_skipped() {
        let updates = scan(&[b"\x1b]7501;future=x:nogood=:state=error\x1b\\"]);
        assert_eq!(updates, vec![set(ProgramState::Error)]);
    }

    #[test]
    fn repeated_key_last_wins() {
        let updates = scan(&[b"\x1b]7501;state=working:state=done\x1b\\"]);
        assert_eq!(updates, vec![set(ProgramState::Done)]);
    }

    #[test]
    fn msg_base64_decode() {
        // "Installing updates"
        let updates = scan(&[b"\x1b]7501;state=working:msg=SW5zdGFsbGluZyB1cGRhdGVz\x1b\\"]);
        assert_eq!(
            updates,
            vec![StatusUpdate::Set(ProgramStatus {
                state: ProgramState::Working,
                app: None,
                msg: Some("Installing updates".into()),
                progress: None,
            })]
        );
    }

    #[test]
    fn msg_invalid_base64_discards_report() {
        // 字符集合法但无法解码（长度非 4 的倍数）：整条丢弃
        assert!(scan(&[b"\x1b]7501;state=working:msg=abcde\x1b\\"]).is_empty());
        // 字符集外的 msg 属畸形 pair：跳过该 pair，报告其余部分仍生效
        let updates = scan(&[b"\x1b]7501;state=working:msg=!!!\x1b\\"]);
        assert_eq!(updates, vec![set(ProgramState::Working)]);
    }

    #[test]
    fn msg_control_characters_discard_report() {
        // base64("\n") = "Cg=="
        assert!(scan(&[b"\x1b]7501;state=working:msg=Cg==\x1b\\"]).is_empty());
    }

    #[test]
    fn id_reports_ignored_for_root_record_mvp() {
        assert!(scan(&[b"\x1b]7501;state=working:id=build/test\x1b\\"]).is_empty());
        assert!(scan(&[b"\x1b]7501;state=clear:id=us-east\x1b\\"]).is_empty());
    }

    #[test]
    fn non_7501_osc_bypassed() {
        assert!(scan(&[b"\x1b]0;window title\x07"]).is_empty());
        assert!(scan(&[b"\x1b]133;A\x1b\\"]).is_empty());
        assert!(scan(&[b"\x1b]9;4;3;50\x1b\\"]).is_empty());
        assert!(scan(&[b"\x1b]75011;state=working\x1b\\"]).is_empty());
    }

    #[test]
    fn oversize_sequence_discarded() {
        let mut seq = b"\x1b]7501;state=working:msg=".to_vec();
        seq.extend(std::iter::repeat_n(b'A', MAX_SEQUENCE_BYTES + 1));
        seq.extend_from_slice(b"\x1b\\");
        assert!(scan(&[&seq]).is_empty());
        // 解析器防御：直接喂超限 body 同样拒绝
        let body = "A".repeat(MAX_SEQUENCE_BYTES + 1);
        assert_eq!(parse_program_status(&body), None);
    }

    #[test]
    fn aborted_sequence_by_esc_does_not_leak_into_next() {
        // OSC 被中断后，紧随的 ESC ] 7501 仍应正常识别
        let updates = scan(&[b"\x1b]7501;state=working\x1b[2J\x1b]7501;state=done\x1b\\"]);
        assert_eq!(updates, vec![set(ProgramState::Done)]);
    }

    #[test]
    fn body_over_limit_stops_buffering_but_stream_stays_aligned() {
        // 超限报告被丢弃，但其后的合法报告仍能识别（流不丢位）
        let mut seq = b"\x1b]7501;".to_vec();
        seq.extend(std::iter::repeat_n(b'x', MAX_SEQUENCE_BYTES * 2));
        seq.extend_from_slice(b"\x1b\\\x1b]7501;state=idle\x1b\\");
        assert_eq!(scan(&[&seq]), vec![set(ProgramState::Idle)]);
    }

    #[test]
    fn app_charset_and_length() {
        let updates = scan(&[b"\x1b]7501;state=working:app=claude-code\x1b\\"]);
        assert!(
            matches!(&updates[0], StatusUpdate::Set(s) if s.app.as_deref() == Some("claude-code"))
        );
        // 非法字符 / 超长：视为缺失
        let updates = scan(&[b"\x1b]7501;state=working:app=my.app,1\x1b\\"]);
        assert!(matches!(&updates[0], StatusUpdate::Set(s) if s.app.is_none()));
        let long_app = "a".repeat(33);
        let updates =
            scan(&[&format!("\x1b]7501;state=working:app={long_app}\x1b\\").into_bytes()]);
        assert!(matches!(&updates[0], StatusUpdate::Set(s) if s.app.is_none()));
    }

    #[test]
    fn whitespace_around_pairs_is_trimmed() {
        let updates = scan(&[b"\x1b]7501; state = working : app = cargo \x1b\\"]);
        assert_eq!(
            updates,
            vec![StatusUpdate::Set(ProgramStatus {
                state: ProgramState::Working,
                app: Some("cargo".into()),
                msg: None,
                progress: None,
            })]
        );
    }
}

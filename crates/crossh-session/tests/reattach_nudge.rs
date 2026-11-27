//! 重连三态集成断言（4.2）：`attach` 分段回放 + nudge 时序。
//!
//! - 首画丢：触发一次 nudge 且 `Size` 不占 `seq`（`Attached.end_seq` 仍是总流偏移）。
//! - 首画仍在：只回放，不触发。
//! - 主屏：只回放，不触发，不抖。
//! - Windows：恒不 nudge，只回放（此处用 `cfg` 分流断言）。

use std::time::{Duration, Instant};

use crossh_session::feed::FeedState;
use crossh_session::protocol::{MessageKind, decode_frame, encode_frame};
use crossh_session::session::{
    REDRAW_NUDGE_COOLDOWN, REPLAY_LIMIT_BYTES, foreground_command_running,
};
use crossh_session::workspace::TermSize;

/// 首画已丢且带分段的 feed：默认尺寸段被挤掉首画后，又封一段新几何。
/// 回放剩两段（旧几何大段 + 新几何 `!`），`alt` 全量开着、环内已无。
fn feed_lost_segmented() -> FeedState {
    let mut feed = FeedState::new();
    feed.push(b"\x1b[?1049h");
    feed.push(&vec![b'.'; REPLAY_LIMIT_BYTES - 8]);
    // 尾段非空时封段：新几何独立成段，nudge 恢复读它。
    feed.resize(TermSize {
        cols: 100,
        rows: 30,
    });
    feed.push(b"!");
    assert!(feed.alt_screen_now(), "全量仍在备用屏");
    assert!(!feed.alt_screen_in_ring(), "切换随前史被驱逐，即首画已丢");
    feed
}

/// 首画仍在环内的全屏 feed：小量输出，无驱逐。
fn feed_kept() -> FeedState {
    let mut feed = FeedState::new();
    feed.push(b"\x1b[?1049hvim-screen");
    assert!(feed.alt_screen_now());
    assert!(feed.alt_screen_in_ring(), "切换仍在环内");
    feed
}

/// 主屏 shell feed：无备用屏。
fn feed_main() -> FeedState {
    let mut feed = FeedState::new();
    feed.push(b"shell output");
    assert!(!feed.alt_screen_now());
    assert!(!feed.alt_screen_in_ring());
    feed
}

/// 模拟 `attach` 快照口径：`end_seq` 为总流偏移，各段字节和为回放总量。
fn assert_replay_seq_invariant(feed: &FeedState) -> (u64, usize) {
    let (end_seq, segs) = feed.snapshot_segments();
    let (seq, flat) = feed.snapshot();
    assert_eq!(end_seq, seq, "`end_seq` 即总流偏移");
    assert_eq!(end_seq, feed.seq());
    let total: usize = segs.iter().map(|s| s.bytes.len()).sum();
    assert_eq!(total, flat.len(), "分段拼合与单流一致");
    // 每段 `Size` 帧可编解码，且不占 `seq`（载荷只有几何）。
    for seg in &segs {
        let body = serde_json::to_vec(&seg.size).unwrap();
        let frame = encode_frame(MessageKind::Size, &body).unwrap();
        let (kind, back) = decode_frame(&frame).unwrap();
        assert_eq!(kind, MessageKind::Size);
        assert_eq!(back, body);
    }
    (end_seq, total)
}

#[test]
fn lost_first_paint_nudges_once_and_size_skips_seq() {
    let mut feed = feed_lost_segmented();
    // 分段回放不变量：`Size` 只做几何标记，`seq` 只随字节递增。
    let (end_seq, total) = assert_replay_seq_invariant(&feed);
    assert_eq!(total, feed.snapshot().1.len());
    let start = end_seq.saturating_sub(total as u64);
    // 首段几何是默认尺寸，尾段是封段后的新几何。
    let (_, segs) = feed.snapshot_segments();
    assert!(segs.len() >= 2, "新旧几何分段都在，got {}", segs.len());
    assert_eq!(segs.last().unwrap().size.cols, 100);
    // 前导复用 `Output` 发出进屏切换（环自带者已丢才补）。
    assert_eq!(
        feed.replay_prelude(),
        Some(b"\x1b[?1049h".to_vec()),
        "首画已丢才补前导"
    );
    // 时序：首次命中给临时尺寸（行位移一行），`Size` 不进环。
    let before = feed.snapshot_segments();
    let now = Instant::now();
    let transient = feed.poll_redraw_nudge(true, true, false, now);
    assert_eq!(
        transient,
        Some(TermSize {
            cols: 100,
            rows: 29
        }),
        "同尺寸不发信号，必须错开一行"
    );
    assert_eq!(feed.snapshot_segments(), before, "临时尺寸不进环分段");
    // 冷却内只请一次：500ms 内不再请，冷却过后可再请。
    assert_eq!(
        feed.poll_redraw_nudge(true, true, false, now + Duration::from_millis(500)),
        None,
        "冷却内重连只回放"
    );
    assert!(
        feed.poll_redraw_nudge(true, true, false, now + REDRAW_NUDGE_COOLDOWN)
            .is_some(),
        "冷却过后可再请"
    );
    // observer 与 controller 同权：判定忽略该位，只动 PTY 不改布局。
    for is_observer in [false, true] {
        let mut f = feed_lost_segmented();
        let out = f.poll_redraw_nudge(true, true, is_observer, Instant::now());
        assert!(
            out.is_some(),
            "is_observer={is_observer} 同样触发 PTY 侧请求"
        );
    }
    // `start` 口径与 daemon 一致：`Output` 首包序号即快照起点。
    assert!(start < end_seq || total == 0);
}

#[test]
fn kept_first_paint_only_replays() {
    let mut feed = feed_kept();
    assert_replay_seq_invariant(&feed);
    // 环自带切换，不重复补前导。
    assert_eq!(feed.replay_prelude(), None, "首画仍在不补前导");
    // 首画仍在：回放即整屏，零打扰。
    assert_eq!(
        feed.poll_redraw_nudge(true, true, false, Instant::now()),
        None,
        "首画仍在不触发"
    );
    // 前台缺失同样不触发（`tcgetpgrp` 失败/`None` 只回放）。
    assert_eq!(
        feed.poll_redraw_nudge(true, false, false, Instant::now()),
        None
    );
}

#[test]
fn main_screen_only_replays_without_jitter() {
    let mut feed = feed_main();
    assert_replay_seq_invariant(&feed);
    assert_eq!(feed.replay_prelude(), None, "主屏无前导");
    // 主屏 / 死会话：只回放，不请重画（前端也不抖，见 4.1）。
    assert_eq!(
        feed.poll_redraw_nudge(true, false, false, Instant::now()),
        None,
        "主屏 shell 不触发"
    );
    assert_eq!(
        feed.poll_redraw_nudge(false, true, false, Instant::now()),
        None,
        "死会话不触发"
    );
}

#[test]
fn rows_one_goes_taller_without_touching_ring() {
    // 单行高无处可减：向上错一行，同样只动一行。
    let mut feed = feed_lost_segmented();
    feed.resize(TermSize { cols: 80, rows: 1 });
    let before = feed.snapshot_segments();
    let out = feed.poll_redraw_nudge(true, true, false, Instant::now());
    assert_eq!(out, Some(TermSize { cols: 80, rows: 2 }));
    assert_eq!(feed.snapshot_segments(), before, "临时尺寸不进环");
}

/// Windows 恒不 nudge：无 `tcgetpgrp`，恒按无前台处理，只回放。
#[cfg(windows)]
#[test]
fn windows_never_nudges_only_replays() {
    // 探针恒 `None`：签名即无参，与 Unix 的 `fd` 版本区分。
    assert_eq!(crossh_session::session::pty_foreground_pgid(), None);
    for mut feed in [feed_lost_segmented(), feed_kept(), feed_main()] {
        assert_eq!(
            feed.poll_redraw_nudge(true, false, false, Instant::now()),
            None,
            "Windows 恒走无前台分支"
        );
        assert_replay_seq_invariant(&feed);
    }
}

/// 非 Windows 用无前台分支模拟 Windows 语义：同样只回放。
#[cfg(not(windows))]
#[test]
fn non_windows_without_foreground_matches_windows_quiet() {
    // `None`/非法值一律按无前台处理（Windows 探针恒 `None` 走同一分支）。
    assert!(!foreground_command_running(None, Some(1234)));
    assert!(!foreground_command_running(Some(100), Some(100)));
    assert!(foreground_command_running(Some(200), Some(100)));
    for mut feed in [feed_lost_segmented(), feed_kept(), feed_main()] {
        assert_eq!(
            feed.poll_redraw_nudge(true, false, false, Instant::now()),
            None,
            "无前台（Windows 恒走此分支）只回放"
        );
    }
}

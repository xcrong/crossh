//! main_screen — 1:1 移植 pi-tui 的 tui-main-screen.js
//!
//! regular 模式：在主屏 + scrollback 中渲染，**不进入备选缓冲、不捕获鼠标**，
//! 因此选区与右键菜单完全是终端原生行为。
//!
//! 渲染模型（与 pi 的 inline 模式一致）：
//! - `transcript`（对话行）只追加进 scrollback，已打印的行不再重写；
//! - `dock`（编辑器 + footer）固定在屏幕底部，每帧原位重绘；
//! - 追加/尾部改写需要新行时，先在底行显式滚动（`\r\n`×N）腾出空间再写入，
//!   避免新行落在 dock 行上随后被 dock 重绘覆盖；
//! - 只有变化越过可视区（scrollback 不可改写）、首帧或尺寸变化时
//!   才整屏重绘（`\x1b[2J` + 重印尾部），不向 scrollback 复制重复内容；
//! - dock 高度变化（浮层开/关）走增量路径：变高时 dock 覆写 transcript 底部行
//!   （等效滚动），变矮时先清除残行再增量写入（对齐 pi 的 TuiMainScreen，
//!   见 spec 20260822-main-screen-dock-incremental）。

use crate::ansi::{CURSOR_MARKER, SEGMENT_RESET, normalize_terminal_output, visible_width};

/// 主屏渲染器：transcript 追加进 scrollback，dock 原位重绘
#[derive(Default)]
pub struct MainScreenRenderer {
    /// 已提交进 scrollback 的 transcript 行
    printed: Vec<String>,
    prev_dock_len: usize,
    previous_width: usize,
    previous_height: usize,
}

/// 写入路径的几何计算结果
struct WritePlan {
    /// 需要先在底行执行的滚动行数
    scroll_n: usize,
    /// 新内容写入的起始屏幕行（1-based）
    start_row: usize,
    /// 跳过的新内容行数（超出屏幕容量的头部）
    skip: usize,
}

impl MainScreenRenderer {
    pub fn reset(&mut self) {
        self.printed.clear();
        self.prev_dock_len = 0;
        self.previous_width = 0;
        self.previous_height = 0;
    }

    /// 最后一帧的可见屏幕行（供 afterTerminalStop 重放到主屏 scrollback）
    pub fn last_document(&self) -> &[String] {
        &self.printed
    }

    /// 计算追加 M 行时的滚动量、起始行与跳过数。
    ///
    /// `visible_old`：当前屏幕上 transcript 区已占用的行数；
    /// `first_row`：本次写入对应旧内容所在的屏幕行（纯追加时为 visible_old+1）。
    fn plan_rows(&self, first_row: usize, m: usize, viewport: usize, height: usize) -> WritePlan {
        let _ = height;
        // first_row 可能是「已提交尾部的下一行」（超出视口一行），此时容量为 0
        let cap = if first_row > viewport {
            0
        } else {
            viewport - first_row + 1
        };
        let scroll_n = m.saturating_sub(cap);
        let start_row = first_row.saturating_sub(scroll_n).max(1);
        // start_row≥1 故 start_row-1 不会下溢；当 start_row 越过视口底
        // （first_row=viewport+1 且 scroll_n=0 的空追加）时容量为 0，
        // 不能用裸减法（旧代码这里 viewport - start_row 无符号下溢 panic）。
        let cap2 = viewport.saturating_sub(start_row - 1);
        let skip = m.saturating_sub(cap2);
        WritePlan {
            scroll_n,
            start_row,
            skip,
        }
    }

    /// 在底行滚动 `n` 行腾出空间（autowrap 开启时底行的 `\r\n` 恰好各滚动一行）
    fn emit_scroll(buffer: &mut String, height: usize, n: usize) {
        if n > 0 {
            buffer.push_str(&format!("\x1b[{height};1H"));
            for _ in 0..n {
                buffer.push_str("\r\n");
            }
        }
    }

    fn write_lines(buffer: &mut String, lines: &[String], start_row: usize) {
        if lines.is_empty() {
            return;
        }
        buffer.push_str(&format!("\x1b[{start_row};1H"));
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                buffer.push_str("\r\n");
            }
            buffer.push_str("\x1b[2K");
            buffer.push_str(line);
        }
    }

    fn normalize_lines(lines: Vec<String>, width: usize) -> (Vec<String>, Option<(usize, usize)>) {
        let mut cursor_pos = None;
        let mut normalized = Vec::with_capacity(lines.len());
        for line in lines {
            let line = if cursor_pos.is_none() {
                match line.find(CURSOR_MARKER) {
                    Some(idx) => {
                        let col = visible_width(&line[..idx]);
                        cursor_pos = Some((normalized.len(), col));
                        format!("{}{}", &line[..idx], &line[idx + CURSOR_MARKER.len()..])
                    }
                    None => line,
                }
            } else {
                line
            };
            let mut line = format!("{}{}", normalize_terminal_output(&line), SEGMENT_RESET);
            if visible_width(&line) > width {
                line = crate::ansi::slice_by_column(&line, 0, width, true);
            }
            normalized.push(line);
        }
        (normalized, cursor_pos)
    }

    /// 渲染一帧（regular 模式核心）：
    /// - `transcript`：对话内容行（追加语义，写入 scrollback）
    /// - `dock`：底部固定区行（编辑器 + footer，每帧原位重绘）
    /// - 返回写终端的字节串
    pub fn render_frame_regular(
        &mut self,
        transcript: Vec<String>,
        dock: Vec<String>,
        width: usize,
        height: usize,
    ) -> String {
        if width == 0 || height == 0 {
            return String::new();
        }
        // dock 至少保留 1 行给光标所在编辑器；其余全给 transcript 视口
        let dock = if dock.is_empty() {
            vec![String::new()]
        } else {
            dock
        };
        let dock_len = dock.len().min(height.max(1));
        let (dock, cursor_in_dock) = Self::normalize_lines(dock, width);
        let (transcript, _) = Self::normalize_lines(transcript, width);

        let first_frame = self.previous_width == 0;
        let size_changed = self.previous_width != width || self.previous_height != height;

        // transcript 与已提交前缀的共同长度
        let common = self
            .printed
            .iter()
            .zip(transcript.iter())
            .take_while(|(old, new)| old == new)
            .count();
        let old_len = self.printed.len();
        let new_len = transcript.len();
        let viewport = height - dock_len;

        // 当前 transcript 区已占用的行数，以及旧 tail 的起始索引
        let visible_old = old_len.min(viewport);
        let tail_start_old = old_len - visible_old;

        let mut buffer = crate::terminal::BEGIN_SYNCHRONIZED_OUTPUT.to_string();

        // 对齐 pi TuiMainScreen（spec_20260822_main_screen_dock_incremental）：
        // dock 高度变化（浮层开/关）不再强制整屏重绘；变化行在可视区内时一律增量。
        // dock 变矮（浮层关闭）时旧 dock 多出的行需主动清除（残行清除先于
        // transcript 增量写入，避免新写入行被随后的清除擦掉）。
        let incremental = !first_frame && !size_changed;
        if incremental && self.prev_dock_len > dock_len {
            Self::clear_residual(&mut buffer, height, self.prev_dock_len, dock_len);
        }

        if incremental && common == old_len && new_len >= old_len {
            // ── 纯追加：N 行新内容接在已提交尾部之后 ──
            let n = new_len - old_len;
            let plan = self.plan_rows(visible_old + 1, n, viewport, height);
            Self::emit_scroll(&mut buffer, height, plan.scroll_n);
            let lines = &transcript[old_len + plan.skip..];
            Self::write_lines(&mut buffer, lines, plan.start_row);
            self.printed.extend_from_slice(&transcript[old_len..]);
        } else if incremental && old_len.saturating_sub(common) <= viewport {
            // ── 尾部原位改写（流式输出主路径）：变化的后缀全部还在屏幕上，
            //    仅擦写受影响行。流式增量通常只改动最后一条消息的末尾一两行。──
            let first_row = common - tail_start_old + 1;
            let m = new_len - common;
            let plan = self.plan_rows(first_row, m, viewport, height);
            Self::emit_scroll(&mut buffer, height, plan.scroll_n);
            let lines = &transcript[common + plan.skip..];
            Self::write_lines(&mut buffer, lines, plan.start_row);
            self.printed.truncate(common);
            self.printed.extend_from_slice(&transcript[common..]);
        } else {
            // ── 整屏重绘：首帧 / 尺寸变化 / 变化越过可视区 ──
            // 只重印能放下的 transcript 尾部，避免把头部重复灌入 scrollback
            buffer.push_str("\x1b[2J\x1b[H");
            let skip = transcript.len().saturating_sub(viewport);
            for (i, line) in transcript[skip..].iter().enumerate() {
                if i > 0 {
                    buffer.push_str("\r\n");
                }
                buffer.push_str(line);
            }
            self.printed = transcript;
        }
        self.prev_dock_len = dock_len;
        self.previous_width = width;
        self.previous_height = height;
        buffer.push_str(crate::terminal::END_SYNCHRONIZED_OUTPUT);

        // dock 原位重绘 + 硬件光标定位到编辑器位置（恒在界内）
        self.write_dock(&mut buffer, &dock, height);
        match cursor_in_dock {
            Some((row_in_dock, col)) => {
                let row = height - dock_len + 1 + row_in_dock;
                buffer.push_str(&format!(
                    "\x1b[{};{}H\x1b[?25h",
                    row.min(height),
                    col.min(width.saturating_sub(1)) + 1
                ));
            }
            None => buffer.push_str(crate::terminal::HIDE_CURSOR),
        }
        buffer
    }

    /// 清除旧 dock 的全部行（浮层关闭等 dock 变矮场景）。
    ///
    /// 清除范围是整个旧 dock 区域（`prev_dock_len` 行）而非只清"残行区"：
    /// dock 变矮同时 transcript 追加触发滚动（`scroll_n > 0`）时，`\r\n` 滚动
    /// 会把残行区之外的旧 dock 底部行上移到新 dock 顶附近，只清残行区会留下
    /// 残留（review P2 组合）；整体清除后无论是否滚动，旧内容都不可能残留。
    /// 必须在 transcript 增量写入之前调用：新写入行可能落在清除区，
    /// 先清再写才能保证写入内容不被随后的清除擦掉。
    fn clear_residual(buffer: &mut String, height: usize, prev_dock_len: usize, dock_len: usize) {
        if prev_dock_len <= dock_len {
            return;
        }
        debug_assert!(
            prev_dock_len <= height,
            "prev_dock_len 应已在上一帧 min(height) 过"
        );
        let start = height - prev_dock_len + 1;
        for i in 0..prev_dock_len {
            buffer.push_str(&format!("\x1b[{};1H\x1b[2K", start + i));
        }
    }

    /// 把 dock 行写到屏幕底部（调用方保证在 BEGIN_SYNC 区间内）
    fn write_dock(&self, buffer: &mut String, dock: &[String], height: usize) {
        let dock_len = dock.len().min(height);
        if dock_len == 0 {
            return;
        }
        buffer.push_str(&format!("\x1b[{};1H", height - dock_len + 1));
        for (i, line) in dock[..dock_len].iter().enumerate() {
            if i > 0 {
                buffer.push_str("\r\n");
            }
            buffer.push_str("\x1b[2K");
            buffer.push_str(line);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    use super::*;
    use crate::ansi::strip_terminal_sequences;

    #[test]
    fn spec_main_screen__first_frame_prints_transcript_and_dock() {
        let mut r = MainScreenRenderer::default();
        let out = r.render_frame_regular(
            vec!["a".into(), "b".into()],
            vec!["edit".into(), "footer".into()],
            20,
            10,
        );
        assert!(out.contains("a"));
        assert!(out.contains("edit"));
        assert!(out.contains("footer"));
        assert!(out.contains("\x1b[2J")); // 首帧整屏重绘
    }

    #[test]
    fn spec_main_screen__append_diff_only_writes_new_lines() {
        let mut r = MainScreenRenderer::default();
        let _ = r.render_frame_regular(vec!["a".into(), "b".into()], vec!["d".into()], 20, 10);
        let out = r.render_frame_regular(
            vec!["a".into(), "b".into(), "c".into()],
            vec!["d".into()],
            20,
            10,
        );
        assert!(out.contains("c"));
        // 纯追加路径不应有 \x1b[2J 全屏清空，也不应重写旧行 a/b
        assert!(!out.contains("\x1b[2J"));
        assert!(!out.contains("a\r\n"));
    }

    #[test]
    fn spec_main_screen__dock_edit_rewrites_in_place_without_repaint() {
        let mut r = MainScreenRenderer::default();
        let _ = r.render_frame_regular(vec!["a".into()], vec!["d1".into(), "f".into()], 20, 10);
        let out = r.render_frame_regular(vec!["a".into()], vec!["d2".into(), "f".into()], 20, 10);
        assert!(out.contains("d2"));
        // 编辑内容变化不触发整屏重绘
        assert!(!out.contains("\x1b[2J"));
    }

    #[test]
    fn spec_main_screen__streaming_delta_rewrites_tail_in_place_without_repaint() {
        let mut r = MainScreenRenderer::default();
        let _ = r.render_frame_regular(
            vec!["a".into(), "hello wor".into()],
            vec!["d".into()],
            20,
            10,
        );
        // 流式增量：最后一行内容变化 → 原位擦写，不整屏清空
        let out = r.render_frame_regular(
            vec!["a".into(), "hello world!".into()],
            vec!["d".into()],
            20,
            10,
        );
        assert!(!out.contains("\x1b[2J"), "流式不应全屏重绘: {out:?}");
        assert!(out.contains("world!"));
        assert!(out.contains("\x1b[2K"), "应有行内擦写");
        // wrap 使行数增加也仍走原位重写
        let out = r.render_frame_regular(
            vec!["a".into(), "hello world!".into(), "more".into()],
            vec!["d".into()],
            20,
            10,
        );
        assert!(!out.contains("\x1b[2J"), "追加换行也不应全屏重绘: {out:?}");
        assert!(out.contains("more"));
    }

    #[test]
    fn spec_main_screen__rewritten_prefix_triggers_full_repaint_not_duplication() {
        let mut r = MainScreenRenderer::default();
        let long: Vec<String> = (0..30).map(|i| format!("line{i}")).collect();
        let _ = r.render_frame_regular(long.clone(), vec!["d".into()], 20, 10);
        // 变化点越过可视区（视口 9 行，改写 index 15 → 距末尾 15 行）：
        // 必须整屏重绘；变化行本身已在 scrollback 之上、不可达，故不重印
        let mut changed = long.clone();
        changed[15] = "CHANGED".into();
        let out = r.render_frame_regular(changed, vec!["d".into()], 20, 10);
        assert!(out.contains("\x1b[2J"), "expected full repaint: {out:?}");
        // 重印只含尾部视口（height-dock=9 行），含末行、不含头部
        assert!(out.contains("line29"), "tail should be reprinted: {out:?}");
        assert!(
            !out.contains("line0\r\n"),
            "head should not be reprinted: {out:?}"
        );
    }

    #[test]
    fn spec_main_screen__append_scrolls_explicitly_instead_of_clobbering_dock() {
        // 回归：视口已满时追加 N 行必须先显式滚动 N 行，
        // 否则新行写在 dock 行上、随即被 dock 重绘覆盖（用户消息"消失"）。
        let mut r = MainScreenRenderer::default();
        let full: Vec<String> = (0..9).map(|i| format!("t{i}")).collect(); // 视口 9 行占满
        let _ = r.render_frame_regular(full, vec!["d".into()], 20, 10);
        let out = r.render_frame_regular(
            (0..11).map(|i| format!("t{i}")).collect::<Vec<_>>(),
            vec!["d".into()],
            20,
            10,
        );
        // 显式滚动 2 行：定位到底行后连发两个 \r\n
        assert!(out.contains("\x1b[10;1H\r\n\r\n"), "应先滚动: {out:?}");
        // 写入起点 = 视口语量顶(9-2+1=8)
        assert!(out.contains("\x1b[8;1H"), "应从第 8 行写入: {out:?}");
        assert!(out.contains("t10"), "新末行必须出现: {out:?}");
        assert!(!out.contains("\x1b[2J"));
    }

    #[test]
    fn spec_main_screen__suffix_growth_scrolls_within_viewport() {
        // 尾部改写需要更多行时同样先滚动：视口 9 行满，common=7 起 +4 行 → 需滚 2 行
        let mut r = MainScreenRenderer::default();
        let base: Vec<String> = (0..9).map(|i| format!("L{i}")).collect();
        let _ = r.render_frame_regular(base, vec!["d".into()], 20, 10);
        let mut next: Vec<String> = (0..9).map(|i| format!("L{i}")).collect();
        next.truncate(7);
        next.push("X0".into());
        next.push("X1".into());
        next.push("X2".into());
        next.push("X3".into());
        let out = r.render_frame_regular(next, vec!["d".into()], 20, 10);
        // common=7, first_row=8, M=4, cap=2 → scroll 2, start_row=6
        assert!(out.contains("\x1b[10;1H\r\n\r\n"), "应先滚动 2 行: {out:?}");
        assert!(out.contains("\x1b[6;1H"), "应从第 6 行写入: {out:?}");
        assert!(out.contains("X3"));
        assert!(!out.contains("\x1b[2J"));
    }

    #[test]
    fn spec_main_screen__dock_resize_is_incremental_and_clears_residue() {
        // 原 spec_main_screen__dock_resize_triggers_full_repaint 语义被
        // spec_20260822_main_screen_dock_incremental 替换：dock 高度变化不再整屏重绘。
        let mut r = MainScreenRenderer::default();
        let _ = r.render_frame_regular(vec!["a".into()], vec!["d".into(), "f".into()], 20, 10);
        // dock 变高（浮层出现）：不 2J，新行写入
        let out = r.render_frame_regular(
            vec!["a".into()],
            vec!["d".into(), "x".into(), "f".into()],
            20,
            10,
        );
        assert!(!out.contains("\x1b[2J"), "dock 变高不应整屏重绘: {out:?}");
        assert!(out.contains("x"));
        // dock 变矮（浮层消失）：不 2J，旧浮层行被清除（2K 总数 > dock 行数）
        let out2 = r.render_frame_regular(vec!["a".into()], vec!["d".into(), "f".into()], 20, 10);
        assert!(!out2.contains("\x1b[2J"), "dock 变矮不应整屏重绘: {out2:?}");
        assert!(
            out2.matches("\x1b[2K").count() >= 3,
            "残行应被清除（1 清除 + 2 dock 行）: {out2:?}"
        );
    }

    #[test]
    fn spec_20260822_main_screen_dock_incremental__popup_open_covers_transcript_tail_incrementally()
    {
        // 契约 1：浮层出现（dock 变高）且 transcript 无新增 → 无 2J，
        // 浮层行从新 dock 起始行覆写（原 transcript 底部行被等效滚动出视口）
        let mut r = MainScreenRenderer::default();
        // 首帧视口 8 行全满（height=10, dock=2）
        let transcript: Vec<String> = (0..8).map(|i| format!("t{i}")).collect();
        let _ = r.render_frame_regular(
            transcript.clone(),
            vec!["edit".into(), "footer".into()],
            20,
            10,
        );
        // 浮层出现：dock 2→4 行，transcript 不变
        let dock = vec!["pop1".into(), "pop2".into(), "edit".into(), "footer".into()];
        let out = r.render_frame_regular(transcript, dock, 20, 10);
        assert!(!out.contains("\x1b[2J"), "浮层出现帧不应整屏重绘: {out:?}");
        assert!(
            out.contains("\x1b[7;1H"),
            "dock 应从新起始行第 7 行写入: {out:?}"
        );
        assert!(out.contains("pop1"), "浮层首行应显示: {out:?}");
        assert!(out.contains("pop2"), "浮层次行应显示: {out:?}");
        // transcript 底部行不被重写（无 t6/t7 的 2K 重写痕迹，仅 dock 行被清写）
        assert!(
            !out.contains("\x1b[6;1H\x1b[2K"),
            "transcript 区不应被清写: {out:?}"
        );
    }

    #[test]
    fn spec_20260822_main_screen_dock_incremental__popup_close_clears_residue_without_repaint() {
        // 契约 2：浮层消失（dock 变矮）且 transcript 无新增 → 无 2J，残行被清除
        let mut r = MainScreenRenderer::default();
        let transcript: Vec<String> = (0..8).map(|i| format!("t{i}")).collect();
        // 浮层在：dock 4 行（7-10 行），视口 6 行，transcript 8 行溢出 2 行
        let _ = r.render_frame_regular(
            transcript.clone(),
            vec!["pop1".into(), "pop2".into(), "edit".into(), "footer".into()],
            20,
            10,
        );
        // 浮层消失：dock 4→2，transcript 不变
        let out = r.render_frame_regular(transcript, vec!["edit".into(), "footer".into()], 20, 10);
        assert!(!out.contains("\x1b[2J"), "浮层关闭帧不应整屏重绘: {out:?}");
        // 残行（旧 popup 占的屏幕行）被清除：2K 总数 > dock 行数 2
        assert!(out.matches("\x1b[2K").count() > 2, "残行应被清除: {out:?}");
    }

    #[test]
    fn spec_20260822_main_screen_dock_incremental__popup_change_with_streaming_growth_stays_incremental()
     {
        // 契约 3：浮层开/关同时 transcript 有新增/改写 → 无 2J，新行与浮层互不干扰
        let mut r = MainScreenRenderer::default();
        let t0: Vec<String> = (0..8).map(|i| format!("t{i}")).collect();
        let _ = r.render_frame_regular(t0.clone(), vec!["edit".into(), "footer".into()], 20, 10);
        // 浮层出现同时 transcript 追加 1 行
        let mut t1 = t0.clone();
        t1.push("t8".into());
        let out = r.render_frame_regular(
            t1,
            vec!["pop".into(), "edit".into(), "footer".into()],
            20,
            10,
        );
        assert!(
            !out.contains("\x1b[2J"),
            "浮层+流式增长不应整屏重绘: {out:?}"
        );
        assert!(out.contains("t8"), "新 transcript 行应写入: {out:?}");
        assert!(out.contains("pop"), "浮层应显示: {out:?}");
        // 反向：浮层消失同时尾部改写
        let mut t2: Vec<String> = (0..8).map(|i| format!("t{i}")).collect();
        t2[7] = "changed".into();
        let out2 = r.render_frame_regular(t2, vec!["edit".into(), "footer".into()], 20, 10);
        assert!(
            !out2.contains("\x1b[2J"),
            "浮层关闭+改写不应整屏重绘: {out2:?}"
        );
        assert!(out2.contains("changed"), "改写行应写入: {out2:?}");
    }

    #[test]
    fn spec_20260822_main_screen_dock_incremental__dock_shrink_with_scroll_clears_whole_old_dock() {
        // P2 review 组合：dock 变矮 + transcript 追加触发 scroll_n>0 时，
        // \r\n 滚动会把"残行区之外的旧 dock 底部行"上移到新 dock 顶附近，
        // 只清残行区会在滚动后留下残留——旧 dock 区域必须整体清除。
        let mut r = MainScreenRenderer::default();
        let t0: Vec<String> = (0..8).map(|i| format!("t{i}")).collect();
        // 首帧：dock=4（浮层在），视口 6，重印 t2..t7 @ 1..6，dock @ 7..10
        let _ = r.render_frame_regular(
            t0.clone(),
            vec!["pop1".into(), "pop2".into(), "edit".into(), "footer".into()],
            20,
            10,
        );
        // 浮层消失 + transcript 追加 1 行：dock 4→2，视口 8，追加需滚动 1 行
        let mut t1 = t0.clone();
        t1.push("t8".into());
        let out = r.render_frame_regular(t1, vec!["edit".into(), "footer".into()], 20, 10);
        assert!(!out.contains("\x1b[2J"), "不应整屏重绘: {out:?}");
        // 旧 dock 区域 7..10 四行必须在 dock 重写前整体清除，滚动代入的旧行才不会残留
        let clear_all = "\x1b[7;1H\x1b[2K\x1b[8;1H\x1b[2K\x1b[9;1H\x1b[2K\x1b[10;1H\x1b[2K";
        assert!(out.contains(clear_all), "旧 dock 四行应整体清除: {out:?}");
        assert!(out.contains("t8"), "新行应写入: {out:?}");
        assert!(!out.contains("pop1"), "旧浮层内容不应残留: {out:?}");
    }

    #[test]
    fn spec_20260822_main_screen_dock_incremental__size_change_still_full_repaints() {
        // 契约 6：终端尺寸变化仍整屏重绘（行为保持）
        let mut r = MainScreenRenderer::default();
        let _ = r.render_frame_regular(vec!["a".into()], vec!["d".into()], 20, 10);
        let out = r.render_frame_regular(vec!["a".into()], vec!["d".into()], 30, 12);
        assert!(out.contains("\x1b[2J"), "尺寸变化应整屏重绘: {out:?}");
    }

    #[test]
    fn spec_20260822_main_screen_dock_incremental__cursor_stays_inside_dock_after_popup_toggle() {
        // 契约 8：浮层开/关帧后硬件光标仍定位在编辑器行内
        let mut r = MainScreenRenderer::default();
        let t: Vec<String> = (0..5).map(|i| format!("t{i}")).collect();
        let _ = r.render_frame_regular(t.clone(), vec!["edit".into(), "footer".into()], 20, 10);
        // 浮层出现：dock 3 行，编辑器行 = 第 8 行（10-3+1），光标列 = "edit"+1
        let dock = vec![
            format!("edit{}", CURSOR_MARKER),
            "x".into(),
            "footer".into(),
        ];
        let out = r.render_frame_regular(t, dock, 20, 10);
        assert!(!out.contains("\x1b[2J"));
        assert!(out.contains("\x1b[8;5H"), "光标应在第 8 行第 5 列: {out:?}");
    }

    #[test]
    fn spec_main_screen__hardware_cursor_positioned_inside_viewport() {
        let mut r = MainScreenRenderer::default();
        let long: Vec<String> = (0..50).map(|i| format!("line{i}")).collect();
        let dock = vec![format!("input{}", CURSOR_MARKER), "footer".into()];
        let out = r.render_frame_regular(long, dock, 40, 10);
        // 光标必须在 height-2+1=9 行内（而不是按内容索引的 51 行）
        assert!(out.contains("\x1b[9;6H"), "cursor CUP: {out:?}");
    }

    #[test]
    fn spec_main_screen__last_document_returns_tail_for_replay() {
        let mut r = MainScreenRenderer::default();
        let long: Vec<String> = (0..50).map(|i| format!("line{i}")).collect();
        let _ = r.render_frame_regular(long.clone(), vec!["d".into()], 40, 10);
        let doc = r.last_document();
        // last_document 是完整提交记录（供重放），包含全部已提交 transcript
        assert_eq!(doc.len(), 50);
        assert_eq!(strip_terminal_sequences(&doc[49]), "line49");
    }

    #[test]
    fn spec_main_screen__empty_append_when_viewport_full_does_not_panic() {
        // 回归：视口已满（old_len >= viewport）且 transcript 无变化的增量帧
        // （status spinner、popup 候选内容变化等只改 dock 内容、不改高度）
        // 走纯追加分支（n=0）：first_row=viewport+1、scroll_n=0 时
        // start_row=viewport+1，旧代码 cap2 = viewport - start_row + 1 无符号下溢 panic。
        let mut r = MainScreenRenderer::default();
        let full: Vec<String> = (0..11).map(|i| format!("t{i}")).collect(); // 视口 9 行，超出 2 行
        let _ = r.render_frame_regular(full.clone(), vec!["d1".into()], 20, 10);
        // transcript 不变、dock 内容变化（高度不变）→ n=0 的增量帧，不得 panic
        let out = r.render_frame_regular(full, vec!["d2".into()], 20, 10);
        assert!(out.contains("d2"));
        assert!(!out.contains("\x1b[2J"), "空追加不应整屏重绘: {out:?}");
    }
}

//! main_screen — 1:1 移植 pi-tui 的 tui-main-screen.js
//!
//! regular 模式：在主屏 + scrollback 中渲染，**不进入备选缓冲、不捕获鼠标**，
//! 因此选区与右键菜单完全是终端原生行为。
//!
//! 渲染模型（与 pi 的 inline 模式一致）：
//! - `transcript`（对话行）只追加进 scrollback，已打印的行不再重写；
//! - `dock`（编辑器 + footer）固定在屏幕底部，每帧原位重绘；
//! - 仅当 transcript 前缀被改写（/resume、Ctrl-T 折叠切换等）或 dock 高度变化时
//!   才做整屏重绘（`\x1b[2J` + 重印尾部），避免向 scrollback 复制重复内容。

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
        let dock_resized = self.prev_dock_len != dock_len;

        // transcript 与已提交前缀的共同长度
        let common = self
            .printed
            .iter()
            .zip(transcript.iter())
            .take_while(|(old, new)| old == new)
            .count();
        let pure_append =
            !first_frame && !size_changed && !dock_resized && common == self.printed.len();

        let mut buffer = crate::terminal::BEGIN_SYNCHRONIZED_OUTPUT.to_string();
        if pure_append {
            // ── 增量路径：先追加新 transcript 行，再原位重绘 dock ──
            let new_lines = &transcript[common..];
            if !new_lines.is_empty() {
                // 从上一帧 dock 顶行开始覆盖写（该行之上是最后一条已提交的 transcript 行）
                let start_row = height - self.prev_dock_len + 1;
                buffer.push_str(&format!("\x1b[{start_row};1H"));
                for (i, line) in new_lines.iter().enumerate() {
                    if i > 0 {
                        buffer.push_str("\r\n");
                    }
                    buffer.push_str("\x1b[2K");
                    buffer.push_str(line);
                }
            }
            // dock 原位重绘
            self.write_dock(&mut buffer, &dock, height);
            self.printed.extend_from_slice(new_lines);
        } else {
            // ── 整屏重绘：首帧 / 尺寸变化 / dock 高度变化 / 前缀被改写 ──
            // 只重印能放下的 transcript 尾部，避免把头部重复灌入 scrollback
            buffer.push_str("\x1b[2J\x1b[H");
            let viewport = height.saturating_sub(dock_len);
            let skip = transcript.len().saturating_sub(viewport);
            for (i, line) in transcript[skip..].iter().enumerate() {
                if i > 0 {
                    buffer.push_str("\r\n");
                }
                buffer.push_str(line);
            }
            self.write_dock(&mut buffer, &dock, height);
            self.printed = transcript;
        }
        self.prev_dock_len = dock_len;
        self.previous_width = width;
        self.previous_height = height;
        buffer.push_str(crate::terminal::END_SYNCHRONIZED_OUTPUT);

        // 硬件光标定位到 dock 内的编辑器位置（行号是屏幕绝对行，恒在界内）
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
    fn spec_main_screen__rewritten_prefix_triggers_full_repaint_not_duplication() {
        let mut r = MainScreenRenderer::default();
        let long: Vec<String> = (0..30).map(|i| format!("line{i}")).collect();
        let _ = r.render_frame_regular(long.clone(), vec!["d".into()], 20, 10);
        // 前缀被改写（如折叠切换）：必须整屏重绘；改写点在尾部视口内应可见
        let mut changed = long.clone();
        changed[25] = "CHANGED".into();
        let out = r.render_frame_regular(changed, vec!["d".into()], 20, 10);
        assert!(out.contains("\x1b[2J"), "expected full repaint: {out:?}");
        assert!(out.contains("CHANGED"));
        // 重印只含尾部视口（height-dock=9 行），不含头部
        assert!(
            !out.contains("line0\r\n"),
            "head should not be reprinted: {out:?}"
        );
    }

    #[test]
    fn spec_main_screen__dock_resize_triggers_full_repaint() {
        let mut r = MainScreenRenderer::default();
        let _ = r.render_frame_regular(vec!["a".into()], vec!["d".into(), "f".into()], 20, 10);
        let out = r.render_frame_regular(
            vec!["a".into()],
            vec!["d".into(), "x".into(), "f".into()],
            20,
            10,
        );
        assert!(out.contains("\x1b[2J"));
        assert!(out.contains("x"));
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
}

//! screen — pi 的 TuiBase/TuiAltScreen doRender 1:1 移植
//!
//! 管线：layout lines → 去 OSC133 → applySelection(\x1b[7m) → compositeFlashes
//! → extractCursorPosition(CURSOR_MARKER) → applyLineResets → diff(previousScreen)
//! → BEGIN/END_SYNCHRONIZED_OUTPUT 包裹的按行重绘

use crate::ansi::{CURSOR_MARKER, SEGMENT_RESET, normalize_terminal_output, visible_width};
use crate::selection::SelectionState;
use crate::terminal::{BEGIN_SYNCHRONIZED_OUTPUT, END_SYNCHRONIZED_OUTPUT, HIDE_CURSOR};

/// flash 消息容器（pi 的 AltScreenFlashContainer）
#[derive(Default)]
pub struct FlashContainer {
    pub entries: Vec<FlashEntry>,
    next_id: u64,
}

pub struct FlashEntry {
    pub id: u64,
    pub message: String,
    /// 到期时间（毫秒时间戳）
    pub expires_at: u64,
}

impl FlashContainer {
    pub fn flash(&mut self, message: impl Into<String>, duration_ms: u64) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.entries.push(FlashEntry {
            id: self.next_id,
            message: message.into(),
            expires_at: now + duration_ms.max(1),
        });
        self.next_id += 1;
    }
    pub fn expire(&mut self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let before = self.entries.len();
        self.entries.retain(|e| e.expires_at > now);
        before != self.entries.len()
    }
    pub fn render(&self, width: usize) -> Vec<String> {
        self.entries
            .iter()
            .map(|entry| {
                let msg = crate::ansi::truncate_to_width(
                    &format!(" {} ", entry.message),
                    width,
                    "",
                    false,
                );
                format!("\x1b[7m{}\x1b[27m", msg)
            })
            .collect()
    }
}

/// 屏幕渲染器：持有上一帧用于 diff
#[derive(Default)]
pub struct ScreenRenderer {
    previous_screen: Vec<String>,
    previous_width: usize,
    previous_height: usize,
}

impl ScreenRenderer {
    /// 最后一帧的屏幕行（供 afterTerminalStop 重放到主屏 scrollback）
    pub fn last_document(&self) -> &[String] {
        &self.previous_screen
    }
}

impl ScreenRenderer {
    pub fn reset(&mut self) {
        self.previous_screen.clear();
        self.previous_width = 0;
        self.previous_height = 0;
    }

    /// 应用选区高亮：把 [start,end) 行区间包 \x1b[7m…\x1b[27m（pi 的 applySelection）
    pub fn apply_selection(
        screen: &mut [String],
        selection: Option<(usize, usize)>,
        viewport_top: usize,
    ) {
        let Some((start_row, end_row)) = selection else {
            return;
        };
        for row in start_row..=end_row.min(end_row) {
            let screen_row = row.wrapping_sub(viewport_top);
            if screen_row < screen.len() && row >= start_row && !screen[screen_row].is_empty() {
                // 只在未高亮时包裹，避免嵌套
                if !screen[screen_row].contains("\x1b[7m") {
                    screen[screen_row] = format!("\x1b[7m{}\x1b[27m", screen[screen_row]);
                }
            }
        }
        let _ = start_row; // silence unused in some cfgs
    }

    /// 从可见区找 CURSOR_MARKER，返回绝对 (row, col)，并从行中剥离标记
    pub fn extract_cursor_position(lines: &mut [String], height: usize) -> Option<(usize, usize)> {
        let viewport_top = lines.len().saturating_sub(height);
        for row in (viewport_top..lines.len()).rev() {
            if let Some(idx) = lines[row].find(CURSOR_MARKER) {
                let col = visible_width(&lines[row][..idx]);
                lines[row] = format!(
                    "{}{}",
                    &lines[row][..idx],
                    &lines[row][idx + CURSOR_MARKER.len()..]
                );
                return Some((row, col));
            }
        }
        None
    }

    /// 渲染一帧并输出写终端的字节串（pi 的 doRender 输出部分）
    ///
    /// `content_lines`：布局产出的全屏行（可能多于 height，取底部 height 行）
    /// `selection`：选区的内容行区间（相对 content_lines）
    /// `flashes`：flash 消息
    #[allow(clippy::too_many_arguments)]
    pub fn render_frame(
        &mut self,
        mut content_lines: Vec<String>,
        width: usize,
        height: usize,
        selection_rows: Option<(usize, usize)>,
        flashes: &FlashContainer,
    ) -> String {
        if width == 0 || height == 0 {
            return String::new();
        }
        // 取底部 height 行作为视口
        if content_lines.len() > height {
            content_lines.drain(..content_lines.len() - height);
        }
        // 选区行号换算到屏幕坐标
        ScreenRenderer::apply_selection(&mut content_lines, selection_rows, 0);

        // compositeFlashes：左上角反显叠加
        let flash_lines = flashes.render(width);
        if !flash_lines.is_empty() {
            while content_lines.len() < height {
                content_lines.push(String::new());
            }
            for (row, line) in flash_lines.iter().enumerate() {
                if row < content_lines.len() && visible_width(line) > 0 {
                    // pi 的 compositeTuiLine：前段原样 + 反显 overlay 截断到宽度 + 后段继承样式
                    let fw = visible_width(line).min(width);
                    content_lines[row] =
                        crate::ansi::composite_tui_line(&content_lines[row], line, 0, fw, width);
                }
            }
        }

        // 光标提取（在 resets 之前，标记是零宽 APC）
        let cursor_pos = Self::extract_cursor_position(&mut content_lines, height);

        // applyLineResets：normalize + SEGMENT_RESET
        for line in &mut content_lines {
            *line = format!("{}{}", normalize_terminal_output(line), SEGMENT_RESET);
        }
        // 超宽截断
        for line in &mut content_lines {
            if visible_width(line) > width {
                *line = crate::ansi::slice_by_column(line, 0, width, true);
            }
        }

        // diff 渲染
        let full_redraw = self.previous_screen.is_empty()
            || self.previous_width != width
            || self.previous_height != height;
        let mut buffer = String::from(BEGIN_SYNCHRONIZED_OUTPUT);
        if full_redraw {
            buffer.push_str("\x1b[2J");
        }
        for row in 0..height {
            let line = content_lines.get(row).cloned().unwrap_or_default();
            if !full_redraw && self.previous_screen.get(row) == Some(&line) {
                continue;
            }
            buffer.push_str(&format!("\x1b[{};1H\x1b[2K{}", row + 1, line));
        }
        match cursor_pos {
            Some((row, col)) => {
                buffer.push_str(&format!(
                    "\x1b[{};{}H",
                    row + 1,
                    col.min(width.saturating_sub(1)) + 1
                ));
                // agent 场景显示硬件光标（IME 需要），对齐 pi 的 showHardwareCursor 可配置；
                // 这里保持显示以支持输入法
                buffer.push_str("\x1b[?25h");
            }
            None => buffer.push_str(HIDE_CURSOR),
        }
        buffer.push_str(END_SYNCHRONIZED_OUTPUT);

        self.previous_screen = content_lines;
        self.previous_width = width;
        self.previous_height = height;
        buffer
    }
}

/// SelectionState 的便捷转换：内容行区间
pub fn selection_content_rows(selection: &SelectionState) -> Option<(usize, usize)> {
    let (start, end) = selection.bounds()?;
    Some((start.row, end.row))
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]

    use super::*;

    #[test]
    fn spec_20260822_agent_tui_pi_parity__cursor_marker_extracted_and_stripped() {
        let mut lines = vec![String::new(), "abc\x1b_pi:c\x07d".into()];
        let pos = ScreenRenderer::extract_cursor_position(&mut lines, 3);
        assert_eq!(pos, Some((1, 3)));
        assert_eq!(lines[1], "abcd");
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__flash_renders_inverse_video() {
        let mut f = FlashContainer::default();
        f.flash("Copied!", 1000);
        let rendered = f.render(40);
        assert_eq!(rendered.len(), 1);
        assert!(rendered[0].starts_with("\x1b[7m"));
        assert!(rendered[0].ends_with("\x1b[27m"));
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__diff_only_rewrites_changed_rows() {
        let mut r = ScreenRenderer::default();
        let out1 = r.render_frame(
            vec!["aaa".into(), "bbb".into()],
            10,
            3,
            None,
            &FlashContainer::default(),
        );
        assert!(out1.contains("aaa"));
        assert!(out1.contains("\x1b[2J")); // 全量
        // 第二帧只改第一行
        let out2 = r.render_frame(
            vec!["xxx".into(), "bbb".into()],
            10,
            3,
            None,
            &FlashContainer::default(),
        );
        assert!(out2.contains("xxx"));
        assert!(!out2.contains("\x1b[2J"));
        // bbb 行未重写（无 \x1b[2;1H）
        assert!(!out2.contains("\x1b[2;1H"));
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__viewport_takes_bottom_lines() {
        let mut r = ScreenRenderer::default();
        let lines: Vec<String> = (0..10).map(|i| format!("line{}", i)).collect();
        let out = r.render_frame(lines, 20, 4, None, &FlashContainer::default());
        assert!(out.contains("line9"));
        assert!(out.contains("line6"));
        assert!(!out.contains("line5"));
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__line_resets_appended() {
        let mut r = ScreenRenderer::default();
        let out = r.render_frame(vec!["hi".into()], 10, 2, None, &FlashContainer::default());
        assert!(out.contains(&format!("hi{}", SEGMENT_RESET)));
    }
}

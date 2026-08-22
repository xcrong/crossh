//! editor — pi-tui 的 components/editor.js 移植
//!
//! 多行编辑器：上下边框（含 ↑N more/↓N more 滚动指示）、
//! \x1b[7m 假光标 + CURSOR_MARKER 硬件光标标记、垂直滚动（30% 终端高度，最少 5 行）

use crate::ansi::{CURSOR_MARKER, visible_width};
use crate::component::Component;

/// 生成滚动指示边框：`─── ↑ N more ───…`（pi 的 createScrollBorder）
fn create_scroll_border(direction: &str, hidden_line_count: usize, width: usize) -> String {
    let indicator = format!("─── {} {} more ", direction, hidden_line_count);
    let remaining = width as isize - visible_width(&indicator) as isize;
    if remaining >= 0 {
        return format!("{}{}", indicator, "─".repeat(remaining as usize));
    }
    let ellipsis: String = "...".chars().take(width).collect();
    let indicator_width = width.saturating_sub(visible_width(&ellipsis));
    let truncated: String = indicator.chars().take(indicator_width).collect();
    format!("{}{}", truncated, ellipsis)
}

pub struct EditorState {
    pub lines: Vec<String>,
    /// 光标：字符索引（按 char 计）
    pub cursor_line: usize,
    pub cursor_col: usize,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
        }
    }
}

pub struct Editor {
    pub state: EditorState,
    pub padding_x: usize,
    /// 最大可见文本行数（pi 的 maxVisibleLines = max(5, rows*30%)）；
    /// 由渲染管线按当前终端高度设置
    pub max_visible_lines: usize,
    scroll_offset: usize,
    last_width: usize,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new(0)
    }
}

impl Editor {
    pub fn new(padding_x: usize) -> Self {
        Self {
            state: EditorState::default(),
            padding_x,
            max_visible_lines: 5,
            scroll_offset: 0,
            last_width: 80,
        }
    }

    pub fn set_text(&mut self, text: &str) {
        self.state.lines = text.split('\n').map(|s| s.to_string()).collect();
        if self.state.lines.is_empty() {
            self.state.lines.push(String::new());
        }
        self.state.cursor_line = self.state.lines.len() - 1;
        self.state.cursor_col = self
            .state
            .lines
            .last()
            .map(|l| l.chars().count())
            .unwrap_or(0);
    }

    pub fn get_text(&self) -> String {
        self.state.lines.join("\n")
    }

    pub fn clear(&mut self) {
        self.state = EditorState::default();
        self.scroll_offset = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.state.lines.iter().all(|l| l.is_empty())
    }

    /// 插入文本到光标处
    pub fn insert_text(&mut self, text: &str) {
        for ch in text.chars() {
            self.insert_char(ch);
        }
    }

    pub fn insert_char(&mut self, ch: char) {
        let line_idx = self.state.cursor_line;
        let col = self.state.cursor_col;
        let line = &mut self.state.lines[line_idx];
        let byte = line
            .char_indices()
            .nth(col)
            .map(|(b, _)| b)
            .unwrap_or(line.len());
        line.insert(byte, ch);
        self.state.cursor_col += 1;
    }

    pub fn backspace(&mut self) {
        let line_idx = self.state.cursor_line;
        if self.state.cursor_col > 0 {
            let col = self.state.cursor_col;
            let line = &mut self.state.lines[line_idx];
            let byte = line
                .char_indices()
                .nth(col)
                .map(|(b, _)| b)
                .unwrap_or(line.len());
            let prev_byte = line[..byte]
                .char_indices()
                .next_back()
                .map(|(b, _)| b)
                .unwrap_or(0);
            line.replace_range(prev_byte..byte, "");
            self.state.cursor_col -= 1;
        } else if line_idx > 0 {
            let current = self.state.lines.remove(line_idx);
            self.state.cursor_line -= 1;
            self.state.cursor_col = self.state.lines[self.state.cursor_line].chars().count();
            self.state.lines[self.state.cursor_line].push_str(&current);
        }
    }

    pub fn new_line(&mut self) {
        let line_idx = self.state.cursor_line;
        let col = self.state.cursor_col;
        let line = &mut self.state.lines[line_idx];
        let byte = line
            .char_indices()
            .nth(col)
            .map(|(b, _)| b)
            .unwrap_or(line.len());
        let rest = line.split_off(byte);
        self.state.lines.insert(line_idx + 1, rest);
        self.state.cursor_line += 1;
        self.state.cursor_col = 0;
    }

    pub fn move_left(&mut self) {
        if self.state.cursor_col > 0 {
            self.state.cursor_col -= 1;
        } else if self.state.cursor_line > 0 {
            self.state.cursor_line -= 1;
            self.state.cursor_col = self.state.lines[self.state.cursor_line].chars().count();
        }
    }

    pub fn move_right(&mut self) {
        let len = self.state.lines[self.state.cursor_line].chars().count();
        if self.state.cursor_col < len {
            self.state.cursor_col += 1;
        } else if self.state.cursor_line + 1 < self.state.lines.len() {
            self.state.cursor_line += 1;
            self.state.cursor_col = 0;
        }
    }

    pub fn move_up(&mut self) {
        if self.state.cursor_line > 0 {
            self.state.cursor_line -= 1;
            self.state.cursor_col = self
                .state
                .cursor_col
                .min(self.state.lines[self.state.cursor_line].chars().count());
        }
    }

    pub fn move_down(&mut self) {
        if self.state.cursor_line + 1 < self.state.lines.len() {
            self.state.cursor_line += 1;
            self.state.cursor_col = self
                .state
                .cursor_col
                .min(self.state.lines[self.state.cursor_line].chars().count());
        }
    }

    pub fn move_home(&mut self) {
        self.state.cursor_col = 0;
    }

    pub fn move_end(&mut self) {
        self.state.cursor_col = self.state.lines[self.state.cursor_line].chars().count();
    }

    /// 删除到行尾（pi 的 deleteToLineEnd / Ctrl+K）
    pub fn delete_to_line_end(&mut self) {
        let line_idx = self.state.cursor_line;
        let col = self.state.cursor_col;
        let line = &mut self.state.lines[line_idx];
        let byte = line
            .char_indices()
            .nth(col)
            .map(|(b, _)| b)
            .unwrap_or(line.len());
        line.truncate(byte);
    }

    /// 删除到行首（Ctrl+U）
    pub fn delete_to_line_start(&mut self) {
        let line_idx = self.state.cursor_line;
        let col = self.state.cursor_col;
        let line = &mut self.state.lines[line_idx];
        let byte = line
            .char_indices()
            .nth(col)
            .map(|(b, _)| b)
            .unwrap_or(line.len());
        line.replace_range(0..byte, "");
        self.state.cursor_col = 0;
    }

    /// 渲染后的可见行数（与 render 一致），用于外部计算布局高度
    pub fn visual_lines(&self) -> usize {
        // 与 layoutText 相同的换行逻辑估算
        self.state.lines.len()
    }

    /// 光标所在渲染行的行内偏移（列）
    pub fn cursor_visual_col(&self) -> usize {
        visible_width(
            &self.state.lines[self.state.cursor_line]
                .chars()
                .take(self.state.cursor_col)
                .collect::<String>(),
        )
    }
}

impl Component for Editor {
    fn render(&mut self, width: usize) -> Vec<String> {
        let max_padding = (width.saturating_sub(1)) / 2;
        let padding_x = self.padding_x.min(max_padding);
        let content_width = width.saturating_sub(padding_x * 2).max(1);
        let layout_width = content_width
            .max(1)
            .saturating_sub(if padding_x == 0 { 1 } else { 0 })
            .max(1);
        self.last_width = layout_width;

        // layoutText：按宽度硬换（pi 的 layoutText 用 grapheme + 宽度换行）。
        // 每个物理分段记录其在逻辑行内的起始可见宽度，用于把光标定位到
        // 正确的分段与段内列（wrap 后光标不在最后一段时依然准确）。
        struct Seg {
            text: String,
            start_w: usize,
        }
        let mut layout_lines: Vec<(Seg, bool)> = Vec::new();
        for (li, line) in self.state.lines.iter().enumerate() {
            let has_cursor_line = li == self.state.cursor_line;
            let before_w = if has_cursor_line {
                visible_width(&line.chars().take(self.state.cursor_col).collect::<String>())
            } else {
                usize::MAX
            };
            // 硬换分段
            let mut segs: Vec<Seg> = Vec::new();
            let mut cur = String::new();
            let mut cur_w = 0usize;
            let mut cur_start_w = 0usize;
            for ch in line.chars() {
                let cw = visible_width(&ch.to_string());
                if !cur.is_empty() && cur_w + cw > layout_width {
                    segs.push(Seg {
                        text: std::mem::take(&mut cur),
                        start_w: cur_start_w,
                    });
                    cur_start_w += cur_w;
                    cur_w = 0;
                }
                cur.push(ch);
                cur_w += cw;
            }
            segs.push(Seg {
                text: cur,
                start_w: cur_start_w,
            });
            // 光标归属第一个满足 [start, end] 的分段（段末边界属于前一段）
            let mut cursor_placed = false;
            for seg in segs {
                let seg_end_w = seg.start_w + visible_width(&seg.text);
                let cursor_here = has_cursor_line
                    && !cursor_placed
                    && before_w >= seg.start_w
                    && before_w <= seg_end_w;
                if cursor_here {
                    cursor_placed = true;
                    CURSOR_IN_LINE.with(|c| c.set(before_w - seg.start_w));
                }
                layout_lines.push((seg, cursor_here));
            }
        }

        // 最大可见行：由调用方设置（pi 在组件内读 tui.terminal.rows）
        let max_visible_lines = self.max_visible_lines.max(1);

        let cursor_line_index = layout_lines
            .iter()
            .position(|(_, hc)| *hc)
            .unwrap_or(layout_lines.len().saturating_sub(1));

        // 调整滚动偏移保持光标可见
        if cursor_line_index < self.scroll_offset {
            self.scroll_offset = cursor_line_index;
        } else if cursor_line_index >= self.scroll_offset + max_visible_lines {
            self.scroll_offset = cursor_line_index - max_visible_lines + 1;
        }
        let max_scroll_offset = layout_lines.len().saturating_sub(max_visible_lines);
        self.scroll_offset = self.scroll_offset.min(max_scroll_offset);

        let visible_start = self.scroll_offset.min(layout_lines.len());
        let visible_end = (self.scroll_offset + max_visible_lines).min(layout_lines.len());
        let visible_lines = &layout_lines[visible_start..visible_end];

        let mut result = Vec::new();
        let left_padding = " ".repeat(padding_x);
        let right_padding = left_padding.clone();

        // 上边框
        if self.scroll_offset > 0 {
            result.push(create_scroll_border("↑", self.scroll_offset, width));
        } else {
            result.push("─".repeat(width));
        }

        let cursor_col_in_line = CURSOR_IN_LINE.with(|c| c.get());
        for (seg, has_cursor) in visible_lines {
            let mut display = seg.text.clone();
            let mut line_visible = visible_width(&seg.text);
            let mut cursor_in_padding = false;
            if *has_cursor {
                // 把段内可见列换算为字符切分点（宽字符不会跨界：before_w 始终落在字符边界）
                let mut w = 0usize;
                let mut split_chars = seg.text.chars().count();
                for (i, ch) in seg.text.chars().enumerate() {
                    if w >= cursor_col_in_line {
                        split_chars = i;
                        break;
                    }
                    w += visible_width(&ch.to_string());
                }
                let before: String = seg.text.chars().take(split_chars).collect();
                let after: String = seg.text.chars().skip(split_chars).collect();
                let mut after_chars = after.chars();
                let first_grapheme: String = after_chars
                    .next()
                    .map(|c| c.to_string())
                    .unwrap_or_default();
                let rest_after: String = after_chars.collect();
                if !first_grapheme.is_empty() {
                    let cursor = format!("\x1b[7m{}\x1b[0m", first_grapheme);
                    display = format!("{}{}{}{}", before, CURSOR_MARKER, cursor, rest_after);
                    // 宽度不变
                } else {
                    let cursor = "\x1b[7m \x1b[0m";
                    display = format!("{}{}{}", before, CURSOR_MARKER, cursor);
                    line_visible += 1;
                    if line_visible > content_width && padding_x > 0 {
                        cursor_in_padding = true;
                    }
                }
            }
            let padding = " ".repeat(content_width.saturating_sub(line_visible));
            let right = if cursor_in_padding {
                right_padding.chars().skip(1).collect::<String>()
            } else {
                right_padding.clone()
            };
            result.push(format!("{}{}{}{}", left_padding, display, padding, right));
        }

        // 下边框
        let lines_below = layout_lines
            .len()
            .saturating_sub(self.scroll_offset + visible_lines.len());
        if lines_below > 0 {
            result.push(create_scroll_border("↓", lines_below, width));
        } else {
            result.push("─".repeat(width));
        }

        result
    }
}

thread_local! {
    static CURSOR_IN_LINE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]

    use super::*;
    use crate::ansi::strip_terminal_sequences;

    #[test]
    fn spec_20260822_agent_tui_pi_parity__editor_borders_and_fake_cursor() {
        let mut e = Editor::default();
        e.set_text("hello");
        // set_text 光标落在末尾：反显末尾空格
        assert_eq!(e.state.cursor_col, 5);
        let lines = e.render(40);
        assert_eq!(lines.len(), 3); // 上边框 + 内容 + 下边框
        assert!(lines[0].starts_with("─"));
        assert!(lines[2].starts_with("─"));
        assert!(lines[1].contains("\x1b[7m \x1b[0m"));
        assert!(lines[1].contains(CURSOR_MARKER));
        // 光标移到行首后应反显 h
        for _ in 0..5 {
            e.move_left();
        }
        let lines2 = e.render(40);
        assert!(lines2[1].contains("\x1b[7mh\x1b[0m"));
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__editor_cursor_position_across_wrapped_segments() {
        let mut e = Editor::default();
        // 25 个 a + X + 4 个 b，layout_width=20 → 折两段；光标在 col 25（X）
        let text = format!("{}X{}", "a".repeat(25), "b".repeat(4));
        e.set_text(&text);
        e.state.cursor_col = 25;
        let lines = e.render(24); // 内容宽约 22
        let cursor_lines: Vec<&String> = lines.iter().filter(|l| l.contains("\x1b[7m")).collect();
        assert_eq!(cursor_lines.len(), 1, "one cursor line, got {lines:?}");
        let plain = strip_terminal_sequences(cursor_lines[0]);
        // 反显的是 X 而不是别的
        assert!(plain.contains('X'));
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__editor_scroll_indicator_when_content_overflows() {
        let mut e = Editor::default();
        // maxVisibleLines 最少 5：给 10 行内容，光标移到底部
        let text: String = (0..10)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        e.set_text(&text);
        for _ in 0..9 {
            e.move_down();
        }
        let lines = e.render(60);
        assert!(
            lines[0].contains('↑'),
            "top scroll indicator: {:?}",
            lines[0]
        );
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__editor_multiline_insert_and_backspace() {
        let mut e = Editor::default();
        e.insert_text("ab");
        e.new_line();
        e.insert_text("cd");
        assert_eq!(e.get_text(), "ab\ncd");
        e.backspace(); // 删 d
        assert_eq!(e.get_text(), "ab\nc");
        e.move_left(); // col=0
        e.backspace(); // 行首退格：c 并入上一行
        assert_eq!(e.get_text(), "abc");
        assert_eq!(e.state.cursor_line, 0);
        assert_eq!(e.state.cursor_col, 2);
    }
}

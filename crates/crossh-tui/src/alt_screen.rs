//! AltScreen — Rust 1:1 移植 pi-tui 的 tui-alt-screen.js
//!
//! 覆盖 spec 的 14 条契约，首版实现核心子集（文本 + OSC133 + scrollbar + selection + search）

use crate::scroll_view::{PAGE_SCROLL_OVERLAP, ScrollView};
use crate::selection::{Granularity, SelectionPoint, SelectionState};
use crate::terminal::{
    BEGIN_SYNCHRONIZED_OUTPUT, CLEAR_SCREEN, DISABLE_AUTOWRAP, DISABLE_MOUSE, ENABLE_AUTOWRAP,
    END_SYNCHRONIZED_OUTPUT, ENTER_ALT_SCREEN, EXIT_ALT_SCREEN, HIDE_CURSOR, HOME_CURSOR,
    SHOW_CURSOR, mouse_sequence_for_env,
};
use std::collections::HashMap;

const DOUBLE_CLICK_INTERVAL_MS: u64 = 500;
const WHEEL_SCROLL_LINES_DEFAULT: i32 = 1;

#[derive(Debug, Clone)]
pub struct AltScreenOptions {
    pub wheel_scroll_lines: i32,
    pub mouse_enabled: bool,
}

impl Default for AltScreenOptions {
    fn default() -> Self {
        Self {
            wheel_scroll_lines: WHEEL_SCROLL_LINES_DEFAULT,
            mouse_enabled: true,
        }
    }
}

#[derive(Debug)]
pub struct AltScreen {
    pub primary_scroll_view: ScrollView,
    pub wheel_scroll_lines: i32,
    pub mouse_enabled: bool,
    pub alt_active: bool,
    // selection
    pub selection: SelectionState,
    pub last_click: Option<ClickRecord>,
    pub pressed_url: Option<String>,
    // scrollbar drag
    pub scrollbar_drag: Option<ScrollbarDrag>,
    // search
    pub search_query: Option<String>,
    pub search_matches: Vec<SearchMatch>,
    pub search_selected: i32,
    // flash
    pub flashes: Vec<String>,
    // terminal size cache
    pub term_cols: usize,
    pub term_rows: usize,
}

#[derive(Debug, Clone)]
pub struct ClickRecord {
    pub row: usize,
    pub col: usize,
    pub timestamp_ms: u64,
    pub count: usize,
}

#[derive(Debug, Clone)]
pub struct ScrollbarDrag {
    pub scroll_view_id: usize,
    pub grab_offset: i32,
}

#[derive(Debug, Clone)]
pub struct SearchMatch {
    pub row: usize,
    pub start_col: usize,
    pub end_col: usize,
}

impl AltScreen {
    pub fn new(cols: usize, rows: usize, options: AltScreenOptions) -> Self {
        Self {
            primary_scroll_view: ScrollView::with_viewport(rows.saturating_sub(4).max(1)),
            wheel_scroll_lines: options.wheel_scroll_lines.max(1),
            mouse_enabled: options.mouse_enabled,
            alt_active: false,
            selection: SelectionState::default(),
            last_click: None,
            pressed_url: None,
            scrollbar_drag: None,
            search_query: None,
            search_matches: Vec::new(),
            search_selected: -1,
            flashes: Vec::new(),
            term_cols: cols.max(1),
            term_rows: rows.max(1),
        }
    }

    // ── 契约 1: beforeTerminalStart ──
    pub fn before_terminal_start(&mut self, env: &HashMap<String, String>) -> String {
        self.alt_active = true;
        let mouse = if self.mouse_enabled {
            mouse_sequence_for_env(env)
        } else {
            ""
        };
        format!(
            "{}{}{}{}{}{}",
            ENTER_ALT_SCREEN, DISABLE_AUTOWRAP, mouse, CLEAR_SCREEN, HOME_CURSOR, HIDE_CURSOR
        )
    }

    // ── 契约 2: beforeTerminalStop / afterTerminalStop ──
    pub fn before_terminal_stop(&mut self) -> String {
        if !self.alt_active {
            return String::new();
        }
        let mouse_off = if self.mouse_enabled {
            DISABLE_MOUSE
        } else {
            ""
        };
        format!(
            "{}{}{}{}",
            BEGIN_SYNCHRONIZED_OUTPUT, mouse_off, ENABLE_AUTOWRAP, END_SYNCHRONIZED_OUTPUT
        )
    }

    /// pi 的 afterTerminalStop：preserveScreen=false 时把最后一帧重放到主屏幕，
    /// 使会话内容留在 scrollback（pi 的 lastDocument 重放）
    pub fn after_terminal_stop(
        &mut self,
        preserve_screen: bool,
        last_document: &[String],
        width: usize,
    ) -> String {
        if !self.alt_active {
            return String::new();
        }
        self.alt_active = false;
        if preserve_screen {
            format!(
                "{}{}{}{}",
                BEGIN_SYNCHRONIZED_OUTPUT, EXIT_ALT_SCREEN, SHOW_CURSOR, END_SYNCHRONIZED_OUTPUT
            )
        } else {
            // 重放文档到主屏（pi: BEGIN_SYNC + EXIT_ALT + DISABLE_AUTOWRAP + 每行 \r\x1b[2K + reset + ENABLE_AUTOWRAP）
            let mut buffer = format!(
                "{}{}{}",
                BEGIN_SYNCHRONIZED_OUTPUT, EXIT_ALT_SCREEN, DISABLE_AUTOWRAP
            );
            for (row, line) in last_document.iter().enumerate() {
                if row > 0 {
                    buffer.push_str("\r\n");
                }
                buffer.push_str(&format!("\r\x1b[2K{}", line));
            }
            buffer.push_str(&format!(
                "\x1b[0m{}\r\n{}{}",
                ENABLE_AUTOWRAP, SHOW_CURSOR, END_SYNCHRONIZED_OUTPUT
            ));
            let _ = width;
            buffer
        }
    }

    // ── 契约 3: wheel ──
    pub fn handle_wheel(&mut self, direction: i32) {
        let delta = direction * self.wheel_scroll_lines;
        let remaining = self.primary_scroll_view.scroll_by(delta);
        // pi 的 routeWheel 会冒泡，此处仅 primary，故 remaining 丢弃（或可用于父容器）
        let _ = remaining;
    }

    // ── 契约 4-7: 键盘滚动 ──
    pub fn scroll_by(&mut self, delta: i32) {
        self.primary_scroll_view.scroll_by(delta);
    }
    pub fn scroll_page_up(&mut self) {
        let h = self.primary_scroll_view.viewport_height as i32;
        self.scroll_by(-(h - PAGE_SCROLL_OVERLAP as i32).max(1));
    }
    pub fn scroll_page_down(&mut self) {
        let h = self.primary_scroll_view.viewport_height as i32;
        self.scroll_by((h - PAGE_SCROLL_OVERLAP as i32).max(1));
    }
    pub fn scroll_half_page_up(&mut self) {
        let h = self.primary_scroll_view.viewport_height as i32;
        self.scroll_by(-(h / 2).max(1));
    }
    pub fn scroll_half_page_down(&mut self) {
        let h = self.primary_scroll_view.viewport_height as i32;
        self.scroll_by((h / 2).max(1));
    }
    pub fn scroll_line_up(&mut self) {
        self.scroll_by(-1);
    }
    pub fn scroll_line_down(&mut self) {
        self.scroll_by(1);
    }
    pub fn scroll_to_top(&mut self) {
        self.primary_scroll_view.scroll_to_start();
    }
    pub fn scroll_to_bottom(&mut self) {
        self.primary_scroll_view.scroll_to_end();
    }

    // ── 契约 9-11: 选择 ──
    pub fn handle_mouse_down(&mut self, x: usize, y: usize, timestamp_ms: u64, line_text: &str) {
        let point = SelectionPoint {
            row: y,
            col: x,
            scroll_view_id: Some(0),
        };
        // 双击/三击检测
        let word = crate::selection::word_segment_at(line_text, x);
        let count = self.click_count(point, word, timestamp_ms);
        let granularity = match count {
            2 => Granularity::Word,
            3 => Granularity::Line,
            _ => Granularity::Character,
        };
        // word/line 时扩展锚点
        let (anchor, focus) = match granularity {
            Granularity::Word => {
                if let Some((s, e)) = word {
                    (
                        SelectionPoint {
                            row: y,
                            col: s,
                            scroll_view_id: Some(0),
                        },
                        SelectionPoint {
                            row: y,
                            col: e,
                            scroll_view_id: Some(0),
                        },
                    )
                } else {
                    (point, point)
                }
            }
            Granularity::Line => (
                SelectionPoint {
                    row: y,
                    col: 0,
                    scroll_view_id: Some(0),
                },
                SelectionPoint {
                    row: y,
                    col: line_text.chars().count(),
                    scroll_view_id: Some(0),
                },
            ),
            _ => (point, point),
        };
        self.selection.anchor = Some(anchor);
        self.selection.focus = Some(focus);
        self.selection.granularity = granularity;
        self.selection.press_active = true;
        self.selection.dragged = false;
    }

    pub fn handle_mouse_drag(&mut self, x: usize, y: usize) {
        if !self.selection.press_active {
            return;
        }
        let point = SelectionPoint {
            row: y,
            col: x,
            scroll_view_id: Some(0),
        };
        self.selection.update_focus(point);
    }

    /// 结束选区按压。返回是否存在非空选区（拷贝与否由调用方结合 selected_text 决定）
    pub fn handle_mouse_up(&mut self) -> bool {
        if !self.selection.press_active {
            return false;
        }
        self.selection.press_active = false;
        self.selection.bounds().is_some()
    }

    /// 提取选中文本（按可见列语义切片，ANSI 感知）
    pub fn selected_text(&self, lines: &[String]) -> Option<String> {
        use crate::ansi::{slice_by_column, strip_terminal_sequences, visible_width};
        let (start, end) = self.selection.bounds()?;
        if lines.is_empty() {
            return None;
        }
        let start_row = start.row.min(lines.len() - 1);
        let end_row = end.row.min(lines.len() - 1);
        if start_row > end_row {
            return None;
        }
        let mut out = Vec::new();
        for (row, line) in lines.iter().enumerate().take(end_row + 1).skip(start_row) {
            let len = visible_width(line);
            let s = if row == start_row { start.col } else { 0 }.min(len);
            let e = if row == end_row { end.col } else { len }.min(len);
            if s < e {
                out.push(strip_terminal_sequences(&slice_by_column(
                    line,
                    s,
                    e - s,
                    true,
                )));
            } else if start_row != end_row {
                out.push(String::new());
            }
        }
        if out.is_empty() {
            None
        } else {
            Some(out.join("\n"))
        }
    }

    pub fn osc52_sequence(text: &str) -> String {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        format!("\x1b]52;c;{}\x07", STANDARD.encode(text.as_bytes()))
    }

    fn click_count(
        &mut self,
        point: SelectionPoint,
        word: Option<(usize, usize)>,
        now_ms: u64,
    ) -> usize {
        let prev = self.last_click.clone();
        let count = if let (Some(w), Some(p)) = (word, prev) {
            if now_ms.saturating_sub(p.timestamp_ms) <= DOUBLE_CLICK_INTERVAL_MS
                && p.row == point.row
                && p.col == w.0
            {
                (p.count % 3) + 1
            } else {
                1
            }
        } else {
            1
        };
        if let Some((s, e)) = word {
            self.last_click = Some(ClickRecord {
                row: point.row,
                col: s,
                timestamp_ms: now_ms,
                count,
            });
            let _ = e;
        } else {
            self.last_click = None;
        }
        count
    }

    // ── 契约 13: 搜索 ──
    pub fn open_search(&mut self, query: String) {
        self.search_query = Some(query);
        self.search_selected = -1;
        self.search_matches.clear();
    }
    pub fn close_search(&mut self) {
        self.search_query = None;
        self.search_matches.clear();
        self.search_selected = -1;
    }
    pub fn refresh_search(&mut self, lines: &[String]) {
        let Some(q) = self.search_query.clone() else {
            return;
        };
        if q.trim().is_empty() {
            self.search_matches.clear();
            self.search_selected = -1;
            return;
        }
        let mut matches = Vec::new();
        for (row, line) in lines.iter().enumerate() {
            let mut start = 0;
            while let Some(pos) = line[start..].find(&q) {
                let abs = start + pos;
                matches.push(SearchMatch {
                    row,
                    start_col: abs,
                    end_col: abs + q.len(),
                });
                start = abs + 1;
            }
        }
        self.search_matches = matches;
        if !self.search_matches.is_empty() && self.search_selected < 0 {
            self.search_selected = 0;
            // 滚动到首个命中居中 viewportHeight/3
            let first = self.search_matches[0].row;
            let target = first.saturating_sub(self.primary_scroll_view.viewport_height / 3);
            self.primary_scroll_view.scroll_to(target);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]

    use super::*;
    use std::collections::HashMap;

    #[test]
    fn spec_20260822_agent_tui_pi_parity__before_terminal_start_writes_alt_and_mouse() {
        let mut alt = AltScreen::new(80, 24, AltScreenOptions::default());
        let env = HashMap::new();
        let seq = alt.before_terminal_start(&env);
        assert!(seq.contains(ENTER_ALT_SCREEN));
        assert!(seq.contains(DISABLE_AUTOWRAP));
        assert!(seq.contains(HIDE_CURSOR));
        // 默认非 tmux -> ALL_MOTION
        assert!(seq.contains(crate::terminal::ENABLE_ALL_MOTION_MOUSE));
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__selection_drag_extracts_visible_text() {
        let mut alt = AltScreen::new(80, 24, AltScreenOptions::default());
        // 带样式的行：可见文本 "hello world"，列坐标按可见宽计算
        let styled = "\x1b[31mhello\x1b[0m world".to_string();
        alt.handle_mouse_down(2, 1, 1000, "hello world");
        alt.handle_mouse_drag(9, 1);
        assert!(alt.handle_mouse_up(), "drag 后应存在非空选区");
        let text = alt
            .selected_text(&[String::new(), styled])
            .expect("selected text");
        assert_eq!(text, "llo wor");
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__mouse_up_without_selection_is_noop() {
        let mut alt = AltScreen::new(80, 24, AltScreenOptions::default());
        assert!(!alt.handle_mouse_up());
        // 单击（无拖动）= 空选区
        alt.handle_mouse_down(2, 1, 1000, "hello world");
        assert!(!alt.handle_mouse_up());
    }
}

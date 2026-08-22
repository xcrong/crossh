//! 渲染 — 按 pi 的 interactive-mode.js 布局树组织：
//!
//! ```text
//! VStack [
//!   transcriptScrollView {follow: end, primary}   ← grow: 1（对话内容）
//!   dock VStack [                                  ← shrink: 1
//!     editorContainer   （边框编辑器，minSize 3）
//!     footerContainer   （pwd 行 + 统计行）
//!   ]
//! ]
//! ```
//!
//! 输出走 `crossh_tui::screen::ScreenRenderer`（diff + 同步输出 + 光标标记）

use super::*;
use crossh_tui::component::Component;
use crossterm::terminal;
use std::io::{self, Write};
#[cfg(test)]
use unicode_width::UnicodeWidthStr;

/// 编辑器最大可见行：pi 的 maxVisibleLines = max(5, rows*30%)
fn editor_max_visible_lines(rows: usize) -> usize {
    (rows * 3 / 10).max(5)
}

/// 主渲染入口（regular=主屏不捕获鼠标/原生选区，fullscreen=AltScreen）
pub fn render(app: &mut App) -> io::Result<()> {
    let Ok((cols, rows)) = terminal::size() else {
        return Ok(());
    };
    let width = cols as usize;
    let height = rows as usize;
    if width == 0 || height == 0 {
        return Ok(());
    }
    app.flashes.expire();
    if app.fullscreen {
        render_fullscreen(app, width, height)
    } else {
        render_regular(app, width, height)
    }
}

fn render_regular(app: &mut App, width: usize, height: usize) -> io::Result<()> {
    // regular：transcript 追加进 scrollback（终端原生滚动/框选/右键菜单），
    // dock（编辑器 + footer）固定屏幕底部原位重绘
    let content_width = width.max(1);
    let transcript_lines = build_transcript(app, content_width);

    // dock = 编辑器 + footer 两行，顶部可叠加 / 候选浮层
    let mut editor = build_editor(app);
    editor.max_visible_lines = editor_max_visible_lines(height).min(height.saturating_sub(4));
    let editor_lines = editor.render(width);
    let footer_lines = build_footer(app, width);
    let mut dock_lines: Vec<String> = Vec::new();
    if let Some(popup) = build_slash_popup(app, width) {
        dock_lines.extend(popup);
    }
    dock_lines.extend(editor_lines);
    dock_lines.extend(footer_lines);

    let frame = app
        .main_renderer
        .render_frame_regular(transcript_lines, dock_lines, width, height);
    let mut stdout = io::stdout();
    stdout.write_all(frame.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

pub(super) fn build_slash_popup(app: &App, width: usize) -> Option<Vec<String>> {
    let cands = slash::slash_candidates(app);
    if cands.is_empty() {
        return None;
    }
    // 仅在单行以 / 或 、 开头的命令输入时展示（与 handle_key 的 gating 一致）
    if app.input.contains('\n') {
        return None;
    }
    let trimmed = app.input.trim_start();
    if !(trimmed.starts_with('/') || trimmed.starts_with('、')) {
        return None;
    }
    let selected = app.slash_selected.min(cands.len().saturating_sub(1));
    let popup_width = (width.saturating_sub(6))
        .clamp(32, 64)
        .max(20)
        .min(width.saturating_sub(2).max(20));
    let inner_w = popup_width.saturating_sub(2);
    let hint = if cands.len() == 1 {
        " Tab/Enter 补全 "
    } else {
        " ↑↓ 选择 Tab/Enter 补全 "
    };
    let hint_w = visible_width_safe(hint);
    let mut lines: Vec<String> = Vec::new();
    // 顶部边框（嵌入 hint）
    if hint_w + 4 <= popup_width {
        let remaining = popup_width.saturating_sub(2 + hint_w + 2);
        let left = 1usize;
        let right = remaining.saturating_sub(left);
        lines.push(format!(
            "┌{}┤{}├{}┐",
            "─".repeat(left),
            hint,
            "─".repeat(right)
        ));
    } else {
        lines.push(format!("┌{}┐", "─".repeat(popup_width.saturating_sub(2))));
    }
    // 候选视口固定行数：候选数量变化只改行内容、不改 popup 高度。
    // 否则（旧实现）每敲一个字符候选 8→1→5… 都触发 dock 高度变化，
    // 进而整屏 \x1b[2J 重绘（打字时页面闪烁）。
    const POPUP_CANDIDATE_ROWS: usize = 8;
    for idx in 0..POPUP_CANDIDATE_ROWS {
        let Some(cand) = cands.get(idx) else {
            lines.push(format!("│{}│", " ".repeat(inner_w)));
            continue;
        };
        let is_sel = idx == selected;
        // 构造内部文本： display 左对齐 14 列 + desc
        let mut inner = format!(" {:<14} {}", cand.display, cand.desc);
        // 截断并补齐到 inner_w
        let vw = visible_width_safe(&inner);
        if vw > inner_w {
            inner = truncate_ansi(&inner, inner_w);
        }
        let pad = inner_w.saturating_sub(visible_width_safe(&inner));
        inner.push_str(&" ".repeat(pad));
        let content = if is_sel {
            // 选中行：反显 + 加粗，模拟旧版 bg/accent
            format!("\x1b[7m\x1b[1m{}\x1b[22m\x1b[27m", inner)
        } else {
            // 未选中：display 高亮
            // 对 display 部分单独着色，简化为整体着色
            inner
        };
        lines.push(format!("│{}│", content));
    }
    lines.push(format!("└{}┘", "─".repeat(popup_width.saturating_sub(2))));
    // 左对齐浮层：直接返回 popup_width 宽的行，写入时会以 \x1b[2K 清行后左对齐显示
    // 为了与旧版 input.x 对齐，可在每行左侧加少量空格（已包含在 dock 布局中）
    Some(lines)
}

/// footer 两行（regular 与 fullscreen 共用）：
/// 第一行 = status（为空时回退 pwd）；第二行 = 统计 + 右对齐 model·thinking
pub(super) fn build_footer(app: &App, width: usize) -> Vec<String> {
    let first_raw = if app.status.is_empty() {
        format!(" {}", app.workspace.display())
    } else {
        format!(" {}", app.status)
    };
    let stats = format!(
        " {} msgs  ~{} tokens  {} ctx",
        app.session.messages.len(),
        estimate_tokens(&app.session.messages),
        app.context_files.len()
    );
    let right = format!("{} · {}", active_model_label(app), app.thinking.label());
    let left_w = visible_width_safe(&stats);
    let right_w = visible_width_safe(&right);
    // 优先保证右侧 model·thinking 可见（用户刚用 /model 切换后需要立即反馈）。
    // 旧逻辑在 left+right>=width 时直接丢弃 right，导致长模型名（如 muse-spark-1.2）切换后底部不显示。
    let stats_line = if left_w + right_w + 1 < width {
        format!(
            "{}{}{}",
            stats,
            " ".repeat(width - left_w - right_w - 1),
            right
        )
    } else if right_w + 1 >= width {
        truncate_ansi(&right, width)
    } else {
        let avail_left = width.saturating_sub(right_w + 1);
        let left_trunc = truncate_ansi(&stats, avail_left);
        let left_trunc_w = visible_width_safe(&left_trunc);
        let pad = width.saturating_sub(left_trunc_w + right_w + 1);
        format!("{}{} {}", left_trunc, " ".repeat(pad), right)
    };
    vec![truncate_ansi(&first_raw, width), stats_line]
}

fn render_fullscreen(app: &mut App, width: usize, height: usize) -> io::Result<()> {
    // ── 1. transcript 内容行（带 ANSI，与 pi 的隐式文档一致）──
    let content_width = width.max(1);
    let content_lines = build_transcript(app, content_width);

    // ── 2. ScrollView 布局同步（follow: end）──
    app.alt_screen.primary_scroll_view.viewport_height = height;
    app.alt_screen
        .primary_scroll_view
        .update_layout(content_lines.len(), height);

    // ── 3. dock 高度分配：VStack[transcript(fill), dock(editor+footer+popup, shrink)] ──
    let popup_lines = build_slash_popup(app, width);
    let popup_len = popup_lines.as_ref().map(|v| v.len()).unwrap_or(0);
    let mut editor = build_editor(app);
    // pi 的 editor.js：maxVisibleLines = max(5, floor(rows*30%))，组件内部自行裁剪
    let max_editor = editor_max_visible_lines(height);
    editor.max_visible_lines = max_editor;
    let editor_all = editor.render(width);
    // 编辑器实际高度 = 边框2 + 可见文本行数（由组件自身裁剪；这里按总行数截断）
    let editor_lines_n = editor_all.len().min(max_editor + 2);
    let footer_lines_n = 2.min(height.saturating_sub(3));
    let dock_basis = popup_len + editor_lines_n + footer_lines_n;
    let entries = vec![
        crossh_tui::layout::StackEntry::fill(),
        crossh_tui::layout::StackEntry {
            basis: Some(dock_basis),
            grow: 0,
            shrink: 1,
            min_size: 4,
            max_size: usize::MAX,
        },
    ];
    let intrinsic = [0usize, dock_basis];
    let sizes = crossh_tui::layout::allocate_stack_sizes(&entries, &intrinsic, Some(height));
    let transcript_h = sizes[0].max(1);
    let dock_h = sizes[1];
    // popup 固定高度，剩余分配给 editor
    let editor_h = dock_h
        .saturating_sub(popup_len)
        .saturating_sub(footer_lines_n)
        .max(3);

    // ── 4. 组装全屏行 ──
    let scroll_top = app.alt_screen.primary_scroll_view.scroll_top;
    let mut screen: Vec<String> = Vec::with_capacity(height);

    // transcript 视口窗口
    app.conversation_rect = TuiRect {
        x: 0,
        y: 0,
        width: width as i32,
        height: transcript_h as i32,
    };
    app.conversation_lines = content_lines.clone();
    // 选区边界（内容行坐标 + 列），逐行做列级反显（pi 的 applySelection 列级语义）
    let sel_bounds = app.alt_screen.selection.bounds();
    for row in 0..transcript_h {
        let idx = scroll_top + row;
        let mut line = content_lines.get(idx).cloned().unwrap_or_default();
        if let Some((s, e)) = sel_bounds
            && let Some((sc, ec)) = selection_cols_for_row(idx, s.row, s.col, e.row, e.col)
        {
            line = crossh_tui::ansi::style_visible_range(&line, sc, ec);
        }
        screen.push(line);
    }

    // 候选浮层（位于编辑器上方，随 dock 一起出现/消失）
    if let Some(popup) = popup_lines {
        for line in popup {
            screen.push(line);
        }
    }
    // 编辑器（截断到分配高度）
    let editor_lines = editor.render(width);
    for line in editor_lines.into_iter().take(editor_h) {
        screen.push(line);
    }
    while screen.len() < height.saturating_sub(footer_lines_n) {
        screen.push(String::new());
    }

    // footer 两行：status/pwd + 统计（与 regular 共用）
    let footer_lines = build_footer(app, width);
    screen.truncate(height.saturating_sub(footer_lines.len()));
    for line in footer_lines {
        screen.push(line);
    }

    // ── 5. diff 渲染输出（选区已在视口组装时列级应用）──
    let frame = app
        .screen_renderer
        .render_frame(screen, width, height, None, &app.flashes);
    let mut stdout = io::stdout();
    stdout.write_all(frame.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

/// 计算某内容行的选中列区间（pi 的 getSelectionColumns）
fn selection_cols_for_row(
    row: usize,
    start_row: usize,
    start_col: usize,
    end_row: usize,
    end_col: usize,
) -> Option<(usize, usize)> {
    if start_row == end_row {
        return if row == start_row {
            Some((start_col.min(end_col), end_col.max(start_col)))
        } else {
            None
        };
    }
    if row == start_row {
        // 首行：从起始列到行尾
        Some((start_col, usize::MAX))
    } else if row == end_row {
        // 末行：行首到结束列
        Some((0, end_col))
    } else if row > start_row && row < end_row {
        // 中间整行
        Some((0, usize::MAX))
    } else {
        None
    }
}

/// 对话内容行构建（角色标签着色 + Markdown/折叠工具输出）
pub(super) fn build_transcript(app: &mut App, width: usize) -> Vec<String> {
    let mut container = crossh_tui::component::Container::new();
    for (role, content) in &app.messages {
        match role {
            Role::User => {
                // 用户消息：全宽背景 + Markdown（pi 的 UserMessageComponent）；
                // 与 agent 消息左侧对齐（背景从第 0 列开始，无前导空格）
                let label = styled_label("you", theme_color_user());
                let mut md = crossh_tui::markdown::Markdown::new(content.clone(), 0, 0);
                let body = md.render(width.saturating_sub(2));
                let mut box_lines = vec![label];
                box_lines.extend(body);
                let padded: Vec<String> = box_lines
                    .into_iter()
                    .map(|l| {
                        let v = visible_width_safe(&l);
                        // pi 的 userMessageBg #343541（dark.json）
                        format!(
                            "\x1b[48;2;52;53;65m{}{}\x1b[49m",
                            l,
                            " ".repeat(width.saturating_sub(v))
                        )
                    })
                    .collect();
                container.add_child(Box::new(RawLines(padded)));
            }
            Role::Agent => {
                let md = crossh_tui::markdown::Markdown::new(content.clone(), 0, 1);
                container.add_child(Box::new(md));
            }
            Role::Reasoning => {
                if app.show_reasoning {
                    let mut md = crossh_tui::markdown::Markdown::new(content.clone(), 0, 1);
                    // thinkingText = gray italic（pi 的 assistant-thinking 样式）
                    let lines = md
                        .render(width)
                        .into_iter()
                        .map(|l| format!("\x1b[90m\x1b[3m{}\x1b[23m\x1b[39m", l))
                        .collect();
                    container.add_child(Box::new(RawLines(lines)));
                } else {
                    let label = format!(
                        "\x1b[3m\x1b[90m  Thinking… ({} chars, Ctrl-T to show)\x1b[23m\x1b[39m",
                        content.len()
                    );
                    container.add_child(Box::new(RawLines(vec![label, String::new()])));
                }
            }
            Role::Tool => {
                if app.show_tool_details {
                    let title = styled_label("tool", "\x1b[33m");
                    let mut md = crossh_tui::markdown::Markdown::new(content.clone(), 0, 1);
                    let mut lines = vec![title];
                    lines.extend(md.render(width));
                    container.add_child(Box::new(RawLines(lines)));
                } else {
                    let compact = one_line(content);
                    let summary = collapse_tool(&compact, width.saturating_sub(6));
                    let line = format!("\x1b[33m▸ {}\x1b[39m", summary);
                    container.add_child(Box::new(RawLines(vec![line])));
                }
            }
            Role::Approval => {
                let label = styled_label("approval", "\x1b[36m");
                let mut md = crossh_tui::markdown::Markdown::new(content.clone(), 0, 1);
                let mut lines = vec![label];
                lines.extend(md.render(width));
                container.add_child(Box::new(RawLines(lines)));
            }
            Role::Error => {
                let label = styled_label("error", "\x1b[31m");
                let mut md = crossh_tui::markdown::Markdown::new(content.clone(), 0, 1);
                let mut lines = vec![label];
                lines.extend(md.render(width));
                container.add_child(Box::new(RawLines(lines)));
            }
            Role::Notice => {
                let label = styled_label("note", "\x1b[34m");
                let mut md = crossh_tui::markdown::Markdown::new(content.clone(), 0, 1);
                let mut lines = vec![label];
                lines.extend(md.render(width));
                container.add_child(Box::new(RawLines(lines)));
            }
            Role::Queued => {
                let label = styled_label("queued", "\x1b[36m");
                container.add_child(Box::new(RawLines(vec![label, String::new()])));
            }
        }
    }
    container.render(width)
}

/// 编辑器视图（从 App.input 构造，保持既有 input/input_cursor 为唯一真源）
pub(super) fn build_editor(app: &mut App) -> crossh_tui::editor::Editor {
    let mut editor = crossh_tui::editor::Editor::default();
    let text = if app.input.is_empty() {
        String::new()
    } else {
        app.input.clone()
    };
    editor.set_text(&text);
    // 把字节光标换算为编辑器的（行，字符列），使 Home/←→ 等移动在画面上生效
    let cursor_byte = app.input_cursor.min(text.len());
    let mut byte_seen = 0usize;
    for (li, line) in text.split('\n').enumerate() {
        if cursor_byte <= byte_seen + line.len() {
            editor.state.cursor_line = li;
            editor.state.cursor_col = line[..cursor_byte - byte_seen].chars().count();
            break;
        }
        byte_seen += line.len() + 1; // +1 为 '\n'
    }
    editor.padding_x = 0;
    editor
}

/// 已带 ANSI 的预渲染行（适配 Container）
struct RawLines(Vec<String>);

impl crossh_tui::component::Component for RawLines {
    fn render(&mut self, _width: usize) -> Vec<String> {
        std::mem::take(&mut self.0)
    }
}

fn styled_label(label: &str, color: &str) -> String {
    // pi 的消息标签：着色 + 加粗，后跟一个空格
    format!("{}\x1b[1m[{}]\x1b[22m ", color, label)
}

fn theme_color_user() -> &'static str {
    "\x1b[36m"
}

/// 按可见宽度截断（ANSI 安全，走 crossh-tui 的 truncateToWidth）
pub(crate) fn truncate_ansi(s: &str, width: usize) -> String {
    crossh_tui::ansi::truncate_to_width(s, width, "…", false)
}

pub(crate) fn visible_width_safe(s: &str) -> usize {
    crossh_tui::ansi::visible_width(s)
}

fn collapse_tool(content: &str, limit: usize) -> String {
    let limit = limit.max(8);
    if visible_width_safe(content) <= limit {
        return content.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in content.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cw + 1 > limit.saturating_sub(3) {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out.push('…');
    out
}

// ── 兼容保留的纯函数（仅测试引用）──
#[cfg(test)]
const HEADER_HEIGHT: u16 = 3;
#[cfg(test)]
const FOOTER_HEIGHT: u16 = 1;
#[cfg(test)]
const MIN_CONVERSATION_HEIGHT: u16 = 5;
#[cfg(test)]
const MAX_VISIBLE_INPUT_LINES: usize = 6;

#[cfg(test)]
pub fn agent_layout(area: TuiRect, input_height: u16) -> [TuiRect; 4] {
    let h = area.height as usize;
    let ih = input_height as usize;
    let header = TuiRect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: HEADER_HEIGHT as i32,
    };
    let footer = TuiRect {
        x: area.x,
        y: area.y + area.height - FOOTER_HEIGHT as i32,
        width: area.width,
        height: FOOTER_HEIGHT as i32,
    };
    let conv_h = (h as i32 - HEADER_HEIGHT as i32 - FOOTER_HEIGHT as i32 - ih as i32)
        .max(MIN_CONVERSATION_HEIGHT as i32);
    let conversation = TuiRect {
        x: area.x,
        y: area.y + HEADER_HEIGHT as i32,
        width: area.width,
        height: conv_h,
    };
    let input = TuiRect {
        x: area.x,
        y: conversation.y + conversation.height,
        width: area.width,
        height: ih as i32,
    };
    [header, conversation, input, footer]
}

#[cfg(test)]
pub fn input_height(area: TuiRect, app: &App) -> u16 {
    let w = (area.width as usize).saturating_sub(4).max(1);
    let lines = visual_line_count(&app.input, w);
    let desired = lines.min(MAX_VISIBLE_INPUT_LINES) as u16 + 2;
    let max_h = area
        .height
        .saturating_sub(
            HEADER_HEIGHT as i32 + FOOTER_HEIGHT as i32 + MIN_CONVERSATION_HEIGHT as i32,
        )
        .max(1) as u16;
    desired.min(max_h)
}

#[cfg(test)]
pub fn cursor_position(area: TuiRect, input: &str, cursor: usize) -> (u16, u16) {
    let prefix = &input[..cursor.min(input.len())];
    let width = area.width.max(1) as usize;
    let mut col = 0;
    let mut row = 0;
    for ch in prefix.chars() {
        if ch == '\n' {
            row += 1;
            col = 0;
            continue;
        }
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if col > 0 && col + cw > width {
            row += 1;
            col = 0;
        }
        col += cw;
    }
    if col >= width {
        row += col / width;
        col %= width;
    }
    let x = (area.x.max(0) as u16).saturating_add(col.min(width.saturating_sub(1)) as u16);
    let y = (area.y.max(0) as u16)
        .saturating_add(row as u16)
        .min((area.bottom() - 1).max(0) as u16);
    (x, y)
}

#[cfg(test)]
pub fn visual_line_count(input: &str, width: usize) -> usize {
    input
        .split('\n')
        .map(|line| {
            let w = UnicodeWidthStr::width(line);
            w.max(1).div_ceil(width.max(1))
        })
        .sum::<usize>()
        .max(1)
}

pub fn scroll_conversation(app: &mut App, delta: i16) {
    app.alt_screen.primary_scroll_view.scroll_by(delta as i32);
}

pub fn session_name(app: &App) -> String {
    app.session.name.clone().unwrap_or_else(|| {
        format!(
            "session {}",
            app.session.id.get(..8).unwrap_or(&app.session.id)
        )
    })
}

#[cfg(test)]
pub fn wrap_content(content: &str, width: usize) -> Vec<String> {
    crossh_tui::ansi::wrap_text_with_ansi(content, width)
}

#[cfg(test)]
pub fn markdown_content(content: &str, width: usize) -> Vec<String> {
    let mut md = crossh_tui::markdown::Markdown::new(content, 0, 0);
    md.render(width)
}

//! Terminal grid snapshots and GPUI rendering primitives.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

#[cfg(test)]
use alacritty_terminal::grid::Dimensions;
#[cfg(test)]
use alacritty_terminal::index::{Column, Line};
#[cfg(test)]
use alacritty_terminal::term::cell::Cell;
#[cfg(test)]
use alacritty_terminal::term::cell::Flags as CellFlags;
#[cfg(test)]
use alacritty_terminal::term::{Term, TermMode};
use chrono::Local;
use gpui::*;
use terminal as zed_terminal;
#[cfg(test)]
use vte::ansi::Color;
use vte::ansi::{CursorShape, NamedColor, Rgb};

#[cfg(test)]
use super::view::NoopListener;
use super::view::{
    KITTY_PLACEHOLDER_CHAR, KittyPlaceholder, KittyPlaceholderState, TIMESTAMP_GUTTER_GAP,
    TIMESTAMP_GUTTER_WIDTH,
};
use crate::shared::ui::theme;

pub(crate) fn connecting_or_error_view(msg: &str, focus: &FocusHandle) -> impl IntoElement {
    div()
        .id("terminal-error")
        .size_full()
        .bg(theme::canvas())
        .track_focus(focus)
        .flex()
        .items_center()
        .justify_center()
        .text_sm()
        .text_color(theme::warning())
        .child(SharedString::from(msg.to_string()))
}

/// 一个单元格的渲染快照（owned，避免绘制时持有对 term 的借用）。
#[derive(Clone)]
pub(crate) struct RenderCell {
    pub(crate) ch: char,
    pub(crate) fg: Hsla,
    pub(crate) bg: Hsla,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) underline: UnderlineKind,
    pub(crate) underline_color: Hsla,
    pub(crate) strikeout: bool,
    pub(crate) spacer: bool,
    pub(crate) wide: bool,
    pub(crate) zero_width: String,
    pub(crate) kitty_placeholder: bool,
    pub(crate) is_url: bool,
    pub(crate) hyperlink: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct RenderTextRun {
    pub(crate) start_col: usize,
    pub(crate) cell_count: usize,
    pub(crate) force_width_cells: usize,
    pub(crate) text: String,
    pub(crate) fg: Hsla,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) underline: UnderlineKind,
    pub(crate) underline_color: Hsla,
    pub(crate) strikeout: bool,
    pub(crate) is_url: bool,
}

/// GPUI exposes solid and wavy underlines. The other terminal underline modes
/// still retain their semantic presence and use the closest available paint style.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum UnderlineKind {
    #[default]
    None,
    Solid,
    Wavy,
}

#[derive(Clone, Copy)]
pub(crate) struct EffectiveCellStyle {
    pub(crate) fg: Hsla,
    pub(crate) bg: Hsla,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) underline: UnderlineKind,
    pub(crate) underline_color: Hsla,
    pub(crate) strikeout: bool,
}

/// Resolve the terminal cell's rendition before it reaches GPUI.
///
/// ANSI inverse is a cell attribute, not a color. It must therefore be
/// applied after both colors have been looked up in the current palette. This
/// also gives hidden text, dim text, and underline colors one consistent
/// place to resolve their interactions.
#[cfg(test)]
pub(crate) fn effective_cell_style(
    cell: &Cell,
    colors: &alacritty_terminal::term::color::Colors,
    default_fg: Hsla,
    default_bg: Hsla,
) -> EffectiveCellStyle {
    let mut fg_color = cell.fg;
    if cell.flags.contains(CellFlags::BOLD) && !cell.flags.contains(CellFlags::DIM) {
        fg_color = brighten_color(fg_color);
    }

    let mut fg = color_to_hsla(&fg_color, colors).unwrap_or(default_fg);
    let mut bg = color_to_hsla(&cell.bg, colors).unwrap_or(default_bg);
    if cell.flags.contains(CellFlags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }

    if cell.flags.contains(CellFlags::DIM) {
        fg = dimen(fg);
    }
    if cell.flags.contains(CellFlags::HIDDEN) {
        fg = bg;
    }

    let underline = if cell.flags.contains(CellFlags::UNDERCURL) {
        UnderlineKind::Wavy
    } else if cell.flags.intersects(CellFlags::ALL_UNDERLINES) {
        // GPUI currently has no dotted/dashed/double underline primitive. A
        // solid line is preferable to silently dropping the terminal style.
        UnderlineKind::Solid
    } else {
        UnderlineKind::None
    };
    let underline_color = cell
        .underline_color()
        .and_then(|color| color_to_hsla(&color, colors))
        .unwrap_or(fg);

    EffectiveCellStyle {
        fg,
        bg,
        bold: cell.flags.contains(CellFlags::BOLD),
        italic: cell.flags.contains(CellFlags::ITALIC),
        underline,
        underline_color,
        strikeout: cell.flags.contains(CellFlags::STRIKEOUT),
    }
}

fn zed_color_to_hsla(color: &zed_terminal::Color) -> Hsla {
    match color {
        zed_terminal::Color::Spec(zed_terminal::Rgb { r, g, b }) => rgb_to_hsla(Rgb {
            r: *r,
            g: *g,
            b: *b,
        }),
        zed_terminal::Color::Named(name) => default_palette(name),
        zed_terminal::Color::Indexed(index) => default_palette_indexed(*index as usize),
    }
}

fn brighten_zed_color(color: zed_terminal::Color) -> zed_terminal::Color {
    match color {
        zed_terminal::Color::Named(name) => zed_terminal::Color::Named(name.to_bright()),
        other => other,
    }
}

fn effective_zed_cell_style(cell: &zed_terminal::Cell) -> EffectiveCellStyle {
    let mut foreground = cell.foreground();
    if cell.is_bold() && !cell.is_dim() {
        foreground = brighten_zed_color(foreground);
    }

    let mut fg = zed_color_to_hsla(&foreground);
    let mut bg = zed_color_to_hsla(&cell.background());
    if cell.is_inverse() {
        std::mem::swap(&mut fg, &mut bg);
    }
    if cell.is_dim() {
        fg = dimen(fg);
    }

    let underline = if cell.has_undercurl() {
        UnderlineKind::Wavy
    } else if cell.has_underline() {
        UnderlineKind::Solid
    } else {
        UnderlineKind::None
    };

    EffectiveCellStyle {
        fg,
        bg,
        bold: cell.is_bold(),
        italic: cell.is_italic(),
        underline,
        underline_color: fg,
        strikeout: cell.has_strikeout(),
    }
}

#[cfg(test)]
pub(crate) fn brighten_color(color: Color) -> Color {
    match color {
        Color::Named(name) => Color::Named(name.to_bright()),
        other => other,
    }
}

impl RenderTextRun {
    fn from_cell(col: usize, cell: &RenderCell) -> Self {
        let mut text = String::with_capacity(cell.ch.len_utf8() + cell.zero_width.len());
        text.push(cell.ch);
        text.push_str(&cell.zero_width);

        let cell_width = if cell.wide { 2 } else { 1 };
        let is_url = cell.is_url || cell.hyperlink.is_some();
        Self {
            start_col: col,
            cell_count: cell_width,
            force_width_cells: cell_width,
            text,
            fg: if is_url {
                rgb_to_hsla(Rgb {
                    r: 0x4f,
                    g: 0xaf,
                    b: 0xff,
                })
            } else {
                cell.fg
            },
            bold: cell.bold,
            italic: cell.italic,
            underline: if is_url && cell.underline == UnderlineKind::None {
                UnderlineKind::Solid
            } else {
                cell.underline
            },
            underline_color: if is_url {
                rgb_to_hsla(Rgb {
                    r: 0x4f,
                    g: 0xaf,
                    b: 0xff,
                })
            } else {
                cell.underline_color
            },
            strikeout: cell.strikeout,
            is_url,
        }
    }

    fn has_same_style(&self, other: &Self) -> bool {
        self.force_width_cells == 1
            && other.force_width_cells == 1
            && self.fg == other.fg
            && self.bold == other.bold
            && self.italic == other.italic
            && self.underline == other.underline
            && self.underline_color == other.underline_color
            && self.strikeout == other.strikeout
            && self.is_url == other.is_url
    }
}

/// 将终端网格转换为带有明确列起点的文本 run。
///
/// 普通字符可以按样式合并，但宽字符必须单独成 run：GPUI 的固定宽度排版
/// 按 glyph 推进，而终端网格按 cell 推进。把两者混在同一个 run 中会让中文
/// 只占一列，随后字符就会覆盖它。
pub(crate) fn terminal_text_runs(row: &[RenderCell]) -> Vec<RenderTextRun> {
    let mut runs = Vec::with_capacity(row.len() / 8 + 1);
    let mut current: Option<RenderTextRun> = None;

    for (col, cell) in row.iter().enumerate() {
        if cell.spacer || cell.kitty_placeholder {
            continue;
        }

        let cell_run = RenderTextRun::from_cell(col, cell);
        if cell.wide {
            if let Some(run) = current.take() {
                runs.push(run);
            }
            runs.push(cell_run);
            continue;
        }

        if let Some(run) = current.as_mut()
            && run.start_col + run.cell_count == col
            && run.has_same_style(&cell_run)
        {
            run.text.push_str(&cell_run.text);
            run.cell_count += 1;
        } else {
            if let Some(run) = current.take() {
                runs.push(run);
            }
            current = Some(cell_run);
        }
    }

    if let Some(run) = current {
        runs.push(run);
    }

    runs
}

/// Returns the visual cell span occupied by the terminal cursor.
///
/// Alacritty stores a wide character in a leading cell followed by a spacer,
/// but the cursor can temporarily point at either half while an application is
/// editing the line. Keep the cursor rectangle and the glyph it repaints on
/// the same two-cell span in both cases.
pub(crate) fn cursor_visual_span(row: &[RenderCell], cursor_col: usize) -> (usize, usize, usize) {
    let Some(cell) = row.get(cursor_col) else {
        return (cursor_col, 1, cursor_col);
    };

    if cell.wide {
        return (cursor_col, 2, cursor_col);
    }

    if cell.spacer {
        if cursor_col > 0 && row[cursor_col - 1].wide {
            return (cursor_col - 1, 2, cursor_col - 1);
        }
        if row.get(cursor_col + 1).is_some_and(|next| next.wide) {
            return (cursor_col, 2, cursor_col + 1);
        }
    }

    (cursor_col, 1, cursor_col)
}

/// 可见视口快照 + 光标位置 + 选择区域。
pub(crate) struct Snapshot {
    pub(crate) rows: Vec<Vec<RenderCell>>,
    pub(crate) cursor: Option<(usize, usize)>, // (col, row_within_viewport)
    /// viewport 内 ((col,row), (col,row)) 选择起止。
    pub(crate) selection: Option<((usize, usize), (usize, usize))>,
    pub(crate) cols: usize,
    /// 当前显示偏移（滚动条用）。
    pub(crate) display_offset: usize,
    /// 历史总行数（滚动条用）。
    pub(crate) history_len: usize,
    /// 光标是否可见（闪烁控制 + DECTCEM）。
    pub(crate) cursor_visible: bool,
    /// DECSCUSR/OSC 50 设置的光标形状。
    pub(crate) cursor_shape: CursorShape,
    /// 可见区内的 URL：(row_in_viewport, col_start, col_end, url_string)。
    pub(crate) urls: Vec<(usize, usize, usize, String)>,
    /// 可见区每一行的时间戳；换行续行和 alternate screen 为 None。
    pub(crate) timestamps: Vec<Option<String>>,
    /// Kitty Unicode placeholders decoded from the visible terminal grid.
    pub(crate) kitty_placeholders: Vec<KittyPlaceholder>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RowSignature {
    pub(crate) hash: u64,
    pub(crate) has_content: bool,
    pub(crate) text: String,
    pub(crate) wraps_to_next: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg(test)]
pub(crate) struct LogicalTimestampLine {
    pub(crate) text: String,
    pub(crate) timestamp: Option<String>,
}

/// 保存终端主屏幕的行时间戳。它和 Zed terminal 的内容网格分开，避免任何 UI 元数据
/// 进入 PTY，也避免 ANSI 控制序列被误显示成终端内容。
#[derive(Default)]
pub(crate) struct TerminalTimestampState {
    pub(crate) lines: Vec<Option<String>>,
    pub(crate) signatures: Vec<RowSignature>,
    pub(crate) columns: usize,
    pub(crate) screen_lines: usize,
}

impl TerminalTimestampState {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    #[cfg(test)]
    pub(crate) fn observe(&mut self, term: &Term<NoopListener>, timestamp: String) {
        let grid = term.grid();
        let signatures = terminal_row_signatures(term);
        let columns = grid.columns();
        let screen_lines = grid.screen_lines();
        let shape_changed =
            self.columns != 0 && (self.columns != columns || self.screen_lines != screen_lines);

        let old_signatures = std::mem::take(&mut self.signatures);
        let old_lines = std::mem::take(&mut self.lines);
        let mut next_lines = vec![None; signatures.len()];
        let mut mapping = vec![None; signatures.len()];

        if !shape_changed && !old_signatures.is_empty() {
            if signatures.len() > old_signatures.len() {
                for (new_index, old_index) in
                    mapping.iter_mut().enumerate().take(old_signatures.len())
                {
                    *old_index = Some(new_index);
                }
            } else if signatures.len() == old_signatures.len() {
                if let Some(shift) = detect_scroll_shift(&old_signatures, &signatures) {
                    for (new_index, mapped_old_index) in mapping.iter_mut().enumerate() {
                        let old_index = new_index + shift;
                        if old_index < old_signatures.len() {
                            *mapped_old_index = Some(old_index);
                        }
                    }
                } else {
                    for (new_index, old_index) in mapping.iter_mut().enumerate() {
                        *old_index = Some(new_index);
                    }
                }
            }
        }

        for (new_index, signature) in signatures.iter().enumerate() {
            let Some(old_index) = mapping[new_index] else {
                if signature.has_content {
                    next_lines[new_index] = Some(timestamp.clone());
                }
                continue;
            };

            if old_signatures.get(old_index) == Some(signature) {
                next_lines[new_index] = old_lines.get(old_index).cloned().flatten();
            } else if signature.has_content {
                next_lines[new_index] = Some(timestamp.clone());
            }
        }

        // 即使输出只有 ANSI 控制序列，当前编辑行也代表一次新的终端活动。
        // 这能让空提示符行在 gutter 中有时间，而不会给所有空白行加时间。
        let cursor_index = grid.history_size() as i32 + grid.cursor.point.line.0;
        if let Ok(cursor_index) = usize::try_from(cursor_index)
            && cursor_index < next_lines.len()
        {
            next_lines[cursor_index] = Some(timestamp);
        }

        self.lines = next_lines;
        self.signatures = signatures;
        self.columns = columns;
        self.screen_lines = screen_lines;
    }

    #[cfg(test)]
    pub(crate) fn sync_to_term(&mut self, term: &Term<NoopListener>) {
        let signatures = terminal_row_signatures(term);
        let old_signatures = std::mem::take(&mut self.signatures);
        let old_lines = std::mem::take(&mut self.lines);
        let next_lines = remap_timestamps_after_resize(&old_signatures, &old_lines, &signatures);

        self.lines = next_lines;
        self.signatures = signatures;
        self.columns = term.grid().columns();
        self.screen_lines = term.grid().screen_lines();
    }

    #[cfg(test)]
    pub(crate) fn visible(&self, term: &Term<NoopListener>) -> Vec<Option<String>> {
        let grid = term.grid();
        let rows = grid.screen_lines();
        if term.mode().contains(TermMode::ALT_SCREEN) {
            return vec![None; rows];
        }

        let history = grid.history_size();
        let display_offset = grid.display_offset();
        let start = history.saturating_sub(display_offset);
        (0..rows)
            .map(|row| {
                let line = Line(-(display_offset as i32) + row as i32);
                let index = start + row;
                let continuation = row > 0
                    && grid[Line(line.0 - 1)][Column(grid.columns() - 1)]
                        .flags
                        .contains(CellFlags::WRAPLINE);
                if continuation {
                    None
                } else {
                    self.lines.get(index).cloned().flatten()
                }
            })
            .collect()
    }

    pub(crate) fn observe_content(&mut self, content: &zed_terminal::Content, timestamp: String) {
        let signatures = content_row_signatures(content);
        let rows = signatures.len();
        let shape_changed = self.columns != 0
            && (self.columns != content.terminal_bounds.num_columns() || self.screen_lines != rows);
        let old_signatures = std::mem::take(&mut self.signatures);
        let old_lines = std::mem::take(&mut self.lines);
        let mut next_lines = vec![None; rows];
        let mut mapping = vec![None; rows];

        if !shape_changed && old_signatures.len() == rows {
            if let Some(shift) = detect_scroll_shift(&old_signatures, &signatures) {
                for (new_index, mapped_old_index) in mapping.iter_mut().enumerate() {
                    let old_index = new_index + shift;
                    if old_index < old_signatures.len() {
                        *mapped_old_index = Some(old_index);
                    }
                }
            } else {
                for (new_index, old_index) in mapping.iter_mut().enumerate() {
                    *old_index = Some(new_index);
                }
            }
        }

        for (new_index, signature) in signatures.iter().enumerate() {
            let Some(old_index) = mapping[new_index] else {
                if signature.has_content {
                    next_lines[new_index] = Some(timestamp.clone());
                }
                continue;
            };
            if old_signatures.get(old_index) == Some(signature) {
                next_lines[new_index] = old_lines.get(old_index).cloned().flatten();
            } else if signature.has_content {
                next_lines[new_index] = Some(timestamp.clone());
            }
        }

        let cursor_index = content
            .cursor
            .point
            .line
            .saturating_add(content.display_offset as i32);
        if let Ok(cursor_index) = usize::try_from(cursor_index)
            && cursor_index < next_lines.len()
        {
            next_lines[cursor_index] = Some(timestamp);
        }

        self.lines = next_lines;
        self.signatures = signatures;
        self.columns = content.terminal_bounds.num_columns();
        self.screen_lines = rows;
    }

    pub(crate) fn visible_content(&self, content: &zed_terminal::Content) -> Vec<Option<String>> {
        let rows = content.terminal_bounds.num_lines();
        if content.mode.contains(zed_terminal::Modes::ALT_SCREEN) {
            return vec![None; rows];
        }
        (0..rows)
            .map(|row| self.lines.get(row).cloned().flatten())
            .collect()
    }
}

/// Convert Zed core's terminal-relative selection into the viewport-relative
/// coordinates consumed by Crossh's renderer. Zed can select the entire
/// scrollback, so endpoints outside the current viewport are clamped to its
/// first/last visible cell.
pub(crate) fn selection_for_content(
    content: &zed_terminal::Content,
) -> Option<((usize, usize), (usize, usize))> {
    let rows = content.terminal_bounds.num_lines();
    let cols = content.terminal_bounds.num_columns();
    let selection = content.selection?;
    if rows == 0 || cols == 0 {
        return None;
    }

    let to_viewport = |point: zed_terminal::Point| {
        let row = i64::from(point.line) + content.display_offset as i64;
        let row = row.clamp(0, rows.saturating_sub(1) as i64) as usize;
        let col = point.column.min(cols.saturating_sub(1));
        (col, row)
    };

    Some((to_viewport(selection.start), to_viewport(selection.end)))
}

fn content_row_signatures(content: &zed_terminal::Content) -> Vec<RowSignature> {
    let rows = content.terminal_bounds.num_lines();
    (0..rows)
        .map(|viewport_row| {
            let line = -(content.display_offset as i32) + viewport_row as i32;
            let mut hasher = DefaultHasher::new();
            let mut text = String::new();
            for indexed in content.cells.iter().filter(|cell| cell.point.line == line) {
                let cell = &indexed.cell;
                cell.character().hash(&mut hasher);
                cell.is_inverse().hash(&mut hasher);
                cell.is_bold().hash(&mut hasher);
                cell.is_italic().hash(&mut hasher);
                if !cell.is_wide_char_spacer() {
                    text.push(if cell.character() == '\0' {
                        ' '
                    } else {
                        cell.character()
                    });
                    if let Some(zerowidth) = cell.zerowidth() {
                        for &character in zerowidth {
                            character.hash(&mut hasher);
                            text.push(character);
                        }
                    }
                }
            }
            while text.ends_with(' ') {
                text.pop();
            }
            RowSignature {
                hash: hasher.finish(),
                has_content: text.chars().any(|character| character != ' '),
                text,
                wraps_to_next: false,
            }
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn terminal_row_signatures(term: &Term<NoopListener>) -> Vec<RowSignature> {
    let grid = term.grid();
    let history = grid.history_size();
    let mut signatures = Vec::with_capacity(grid.total_lines());

    for line in -(history as i32)..grid.screen_lines() as i32 {
        let row = &grid[Line(line)];
        let mut hasher = DefaultHasher::new();
        let wraps_to_next = row
            .last()
            .is_some_and(|cell| cell.flags.contains(CellFlags::WRAPLINE));
        let mut text = String::new();
        for cell in row {
            cell.c.hash(&mut hasher);
            cell.flags.hash(&mut hasher);
            if let Some(zerowidth) = cell.zerowidth() {
                zerowidth.hash(&mut hasher);
            }

            if !cell
                .flags
                .intersects(CellFlags::WIDE_CHAR_SPACER | CellFlags::LEADING_WIDE_CHAR_SPACER)
            {
                text.push(if cell.c == '\0' { ' ' } else { cell.c });
                if let Some(zerowidth) = cell.zerowidth() {
                    for &character in zerowidth {
                        text.push(character);
                    }
                }
            }
        }
        if !wraps_to_next {
            while text.ends_with(' ') {
                text.pop();
            }
        }
        let has_content = text.chars().any(|character| character != ' ');
        signatures.push(RowSignature {
            hash: hasher.finish(),
            has_content,
            text,
            wraps_to_next,
        });
    }

    signatures
}

#[cfg(test)]
pub(crate) fn logical_timestamp_lines(
    signatures: &[RowSignature],
    timestamps: &[Option<String>],
) -> Vec<LogicalTimestampLine> {
    let mut logical_lines = Vec::new();
    let mut text = String::new();
    let mut timestamp = None;

    for (index, signature) in signatures.iter().enumerate() {
        if timestamp.is_none() {
            timestamp = timestamps.get(index).cloned().flatten();
        }
        text.push_str(&signature.text);

        if !signature.wraps_to_next {
            logical_lines.push(LogicalTimestampLine {
                text: std::mem::take(&mut text),
                timestamp: timestamp.take(),
            });
        }
    }

    if !text.is_empty()
        || timestamp.is_some()
        || signatures
            .last()
            .is_some_and(|signature| signature.wraps_to_next)
    {
        logical_lines.push(LogicalTimestampLine { text, timestamp });
    }

    logical_lines
}

#[cfg(test)]
pub(crate) fn remap_timestamps_after_resize(
    old_signatures: &[RowSignature],
    old_timestamps: &[Option<String>],
    new_signatures: &[RowSignature],
) -> Vec<Option<String>> {
    let old_logical_lines = logical_timestamp_lines(old_signatures, old_timestamps);
    let new_logical_lines = logical_timestamp_lines(new_signatures, &[]);
    let mut logical_timestamps = vec![None; new_logical_lines.len()];
    let mut old_start = 0;

    for (new_index, new_line) in new_logical_lines.iter().enumerate() {
        let Some(relative_old_index) = old_logical_lines[old_start..]
            .iter()
            .position(|old_line| old_line.text == new_line.text)
        else {
            continue;
        };
        let old_index = old_start + relative_old_index;
        logical_timestamps[new_index] = old_logical_lines[old_index].timestamp.clone();
        old_start = old_index + 1;
    }

    let mut timestamps = vec![None; new_signatures.len()];
    let mut logical_index = 0;
    for (row_index, signature) in new_signatures.iter().enumerate() {
        if let Some(timestamp) = logical_timestamps.get(logical_index) {
            timestamps[row_index] = timestamp.clone();
        }
        if !signature.wraps_to_next {
            logical_index += 1;
        }
    }

    timestamps
}

pub(crate) fn detect_scroll_shift(old: &[RowSignature], new: &[RowSignature]) -> Option<usize> {
    if old.len() != new.len() || old.len() < 4 {
        return None;
    }

    for shift in 1..=old.len().saturating_sub(1).min(8) {
        let overlap = old.len() - shift;
        let matches = old[shift..]
            .iter()
            .zip(&new[..overlap])
            .filter(|(old, new)| old == new)
            .count();
        let informative_matches = old[shift..]
            .iter()
            .zip(&new[..overlap])
            .filter(|(old, new)| old == new && old.has_content)
            .count();
        if matches >= overlap.saturating_sub(1) && informative_matches > 0 {
            return Some(shift);
        }
    }

    None
}

pub(crate) fn format_timestamp(timestamp: chrono::DateTime<Local>) -> String {
    timestamp.format("%H:%M:%S%.3f").to_string()
}

pub(crate) fn terminal_bounds_for(
    canvas_bounds: Bounds<Pixels>,
    show_timestamps: bool,
) -> Bounds<Pixels> {
    let gutter_width = if show_timestamps {
        TIMESTAMP_GUTTER_WIDTH
            .min((canvas_bounds.size.width.as_f32() - TIMESTAMP_GUTTER_GAP - 1.0).max(0.0))
    } else {
        0.0
    };
    let gap = if show_timestamps {
        TIMESTAMP_GUTTER_GAP
    } else {
        0.0
    };
    Bounds {
        origin: Point::new(
            canvas_bounds.origin.x + px(gutter_width + gap),
            canvas_bounds.origin.y,
        ),
        size: gpui::size(
            px((canvas_bounds.size.width.as_f32() - gutter_width - gap).max(1.0)),
            canvas_bounds.size.height,
        ),
    }
}

/// 把 alacritty 的绝对行号转换成当前 viewport 内的行列。
///
/// 终端滚动时光标仍然保留在 grid 的绝对位置，候选框必须使用同一套
/// display_offset 换算，否则会落在错误位置，或在不可见时错误地回退到左下角。
pub(crate) fn cursor_viewport_position(
    cursor_line: i32,
    cursor_column: usize,
    display_offset: usize,
    rows: usize,
    cols: usize,
) -> Option<(usize, usize)> {
    if rows == 0 || cols == 0 {
        return None;
    }

    let display_offset = i32::try_from(display_offset).unwrap_or(i32::MAX);
    let viewport_row = cursor_line.saturating_add(display_offset);
    if viewport_row < 0 || viewport_row >= rows as i32 {
        return None;
    }

    Some((cursor_column.min(cols - 1), viewport_row as usize))
}

pub(crate) fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

const URL_PREFIXES: [&str; 3] = ["https://", "http://", "www."];

pub(crate) fn next_url_start(chars: &[char], from: usize) -> Option<(usize, usize)> {
    for index in from..chars.len() {
        for prefix in URL_PREFIXES {
            let prefix_len = prefix.len();
            if index + prefix_len <= chars.len()
                && chars[index..]
                    .iter()
                    .take(prefix_len)
                    .copied()
                    .eq(prefix.chars())
            {
                return Some((index, prefix_len));
            }
        }
    }
    None
}

pub(crate) fn is_url_delimiter(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '"' | '\'' | '>' | '<' | ')' | ']')
}

pub(crate) fn is_trailing_url_punctuation(ch: char) -> bool {
    matches!(ch, '.' | ',' | ';' | ':' | '!' | '?')
}

/// Find plain-text URLs using logical characters, then translate their ranges
/// back to terminal columns. A terminal cell is not a UTF-8 byte: wide cells
/// and non-ASCII text make those coordinate systems diverge.
pub(crate) fn detect_plain_urls(
    row: &mut [RenderCell],
    row_index: usize,
) -> Vec<(usize, usize, usize, String)> {
    let display_chars: Vec<(usize, char)> = row
        .iter()
        .enumerate()
        .filter(|(_, cell)| !cell.spacer && !cell.kitty_placeholder)
        .map(|(col, cell)| (col, cell.ch))
        .collect();
    let chars: Vec<char> = display_chars.iter().map(|(_, ch)| *ch).collect();
    let mut urls = Vec::new();
    let mut position = 0;

    while let Some((url_start, prefix_len)) = next_url_start(&chars, position) {
        let mut url_end = url_start + prefix_len;
        while url_end < chars.len() && !is_url_delimiter(chars[url_end]) {
            url_end += 1;
        }
        while url_end > url_start + prefix_len && is_trailing_url_punctuation(chars[url_end - 1]) {
            url_end -= 1;
        }

        if url_end > url_start + prefix_len {
            let start_col = display_chars[url_start].0;
            let end_cell_col = display_chars[url_end - 1].0;
            let end_col = end_cell_col + if row[end_cell_col].wide { 2 } else { 1 };
            let url = chars[url_start..url_end].iter().collect();

            for cell in row.iter_mut().take(end_col).skip(start_col) {
                cell.is_url = true;
            }
            urls.push((row_index, start_col, end_col, url));
        }

        position = url_end.max(url_start + prefix_len);
    }

    urls
}

/// Kitty reserves this stable list of combining marks for placeholder row and
/// column numbers. The first entries cover the common compact grid sizes;
/// keeping the mapping explicit avoids treating an unrelated combining mark
/// as image metadata.
const KITTY_PLACEHOLDER_DIACRITICS: &[char] = &[
    '\u{0305}',
    '\u{030d}',
    '\u{030e}',
    '\u{0310}',
    '\u{0312}',
    '\u{033d}',
    '\u{033e}',
    '\u{033f}',
    '\u{0346}',
    '\u{034a}',
    '\u{034b}',
    '\u{034c}',
    '\u{0350}',
    '\u{0351}',
    '\u{0352}',
    '\u{0357}',
    '\u{035b}',
    '\u{0363}',
    '\u{0364}',
    '\u{0365}',
    '\u{0366}',
    '\u{0367}',
    '\u{0368}',
    '\u{0369}',
    '\u{036a}',
    '\u{036b}',
    '\u{036c}',
    '\u{036d}',
    '\u{036e}',
    '\u{036f}',
    '\u{0483}',
    '\u{0484}',
    '\u{0485}',
    '\u{0486}',
    '\u{0487}',
    '\u{0592}',
    '\u{0593}',
    '\u{0594}',
    '\u{0595}',
    '\u{0597}',
    '\u{0598}',
    '\u{0599}',
    '\u{059c}',
    '\u{059d}',
    '\u{059e}',
    '\u{059f}',
    '\u{05a0}',
    '\u{05a1}',
    '\u{05a8}',
    '\u{05a9}',
    '\u{05ab}',
    '\u{05ac}',
    '\u{05af}',
    '\u{05c4}',
    '\u{0610}',
    '\u{0611}',
    '\u{0612}',
    '\u{0613}',
    '\u{0614}',
    '\u{0615}',
    '\u{0616}',
    '\u{0617}',
    '\u{0657}',
    '\u{0658}',
    '\u{0659}',
    '\u{065a}',
    '\u{065b}',
    '\u{065d}',
    '\u{065e}',
    '\u{06d6}',
    '\u{06d7}',
    '\u{06d8}',
    '\u{06d9}',
    '\u{06da}',
    '\u{06db}',
    '\u{06dc}',
    '\u{06df}',
    '\u{06e0}',
    '\u{06e1}',
    '\u{06e2}',
    '\u{06e4}',
    '\u{06e7}',
    '\u{06e8}',
    '\u{06eb}',
    '\u{06ec}',
    '\u{0730}',
    '\u{0732}',
    '\u{0733}',
    '\u{0735}',
    '\u{0736}',
    '\u{073a}',
    '\u{073d}',
    '\u{073f}',
    '\u{0740}',
    '\u{0741}',
    '\u{0743}',
    '\u{0745}',
    '\u{0747}',
    '\u{0749}',
    '\u{074a}',
    '\u{07eb}',
    '\u{07ec}',
    '\u{07ed}',
    '\u{07ee}',
    '\u{07ef}',
    '\u{07f0}',
    '\u{07f1}',
    '\u{07f3}',
    '\u{0816}',
    '\u{0817}',
    '\u{0818}',
    '\u{0819}',
    '\u{081b}',
    '\u{081c}',
    '\u{081d}',
    '\u{081e}',
    '\u{081f}',
    '\u{0820}',
    '\u{0821}',
    '\u{0822}',
    '\u{0823}',
    '\u{0825}',
    '\u{0826}',
    '\u{0827}',
    '\u{0829}',
    '\u{082a}',
    '\u{082b}',
    '\u{082c}',
    '\u{082d}',
    '\u{0951}',
    '\u{0953}',
    '\u{0954}',
    '\u{0f82}',
    '\u{0f83}',
    '\u{0f86}',
    '\u{0f87}',
    '\u{135d}',
    '\u{135e}',
    '\u{135f}',
    '\u{17dd}',
    '\u{193a}',
    '\u{1a17}',
    '\u{1a75}',
    '\u{1a76}',
    '\u{1a77}',
    '\u{1a78}',
    '\u{1a79}',
    '\u{1a7a}',
    '\u{1a7b}',
    '\u{1a7c}',
    '\u{1b6b}',
    '\u{1b6d}',
    '\u{1b6e}',
    '\u{1b6f}',
    '\u{1b70}',
    '\u{1b71}',
    '\u{1b72}',
    '\u{1b73}',
    '\u{1cd0}',
    '\u{1cd1}',
    '\u{1cd2}',
    '\u{1cda}',
    '\u{1cdb}',
    '\u{1ce0}',
    '\u{1dc0}',
    '\u{1dc1}',
    '\u{1dc3}',
    '\u{1dc4}',
    '\u{1dc5}',
    '\u{1dc6}',
    '\u{1dc7}',
    '\u{1dc8}',
    '\u{1dc9}',
    '\u{1dcb}',
    '\u{1dcc}',
    '\u{1dd1}',
    '\u{1dd2}',
    '\u{1dd3}',
    '\u{1dd4}',
    '\u{1dd5}',
    '\u{1dd6}',
    '\u{1dd7}',
    '\u{1dd8}',
    '\u{1dd9}',
    '\u{1dda}',
    '\u{1ddb}',
    '\u{1ddc}',
    '\u{1ddd}',
    '\u{1dde}',
    '\u{1ddf}',
    '\u{1de0}',
    '\u{1de1}',
    '\u{1de2}',
    '\u{1de3}',
    '\u{1de4}',
    '\u{1de5}',
    '\u{1de6}',
    '\u{1dfe}',
    '\u{20d0}',
    '\u{20d1}',
    '\u{20d4}',
    '\u{20d5}',
    '\u{20d6}',
    '\u{20d7}',
    '\u{20db}',
    '\u{20dc}',
    '\u{20e1}',
    '\u{20e7}',
    '\u{20e9}',
    '\u{20f0}',
    '\u{2cef}',
    '\u{2cf0}',
    '\u{2cf1}',
    '\u{2de0}',
    '\u{2de1}',
    '\u{2de2}',
    '\u{2de3}',
    '\u{2de4}',
    '\u{2de5}',
    '\u{2de6}',
    '\u{2de7}',
    '\u{2de8}',
    '\u{2de9}',
    '\u{2dea}',
    '\u{2deb}',
    '\u{2dec}',
    '\u{2ded}',
    '\u{2dee}',
    '\u{2def}',
    '\u{2df0}',
    '\u{2df1}',
    '\u{2df2}',
    '\u{2df3}',
    '\u{2df4}',
    '\u{2df5}',
    '\u{2df6}',
    '\u{2df7}',
    '\u{2df8}',
    '\u{2df9}',
    '\u{2dfa}',
    '\u{2dfb}',
    '\u{2dfc}',
    '\u{2dfd}',
    '\u{2dfe}',
    '\u{2dff}',
    '\u{a66f}',
    '\u{a67c}',
    '\u{a67d}',
    '\u{a6f0}',
    '\u{a6f1}',
    '\u{a8e0}',
    '\u{a8e1}',
    '\u{a8e2}',
    '\u{a8e3}',
    '\u{a8e4}',
    '\u{a8e5}',
    '\u{a8e6}',
    '\u{a8e7}',
    '\u{a8e8}',
    '\u{a8e9}',
    '\u{a8ea}',
    '\u{a8eb}',
    '\u{a8ec}',
    '\u{a8ed}',
    '\u{a8ee}',
    '\u{a8ef}',
    '\u{a8f0}',
    '\u{a8f1}',
    '\u{aab0}',
    '\u{aab2}',
    '\u{aab3}',
    '\u{aab7}',
    '\u{aab8}',
    '\u{aabe}',
    '\u{aabf}',
    '\u{aac1}',
    '\u{fe20}',
    '\u{fe21}',
    '\u{fe22}',
    '\u{fe23}',
    '\u{fe24}',
    '\u{fe25}',
    '\u{fe26}',
    '\u{10a0f}',
    '\u{10a38}',
    '\u{1d185}',
    '\u{1d186}',
    '\u{1d187}',
    '\u{1d188}',
    '\u{1d189}',
    '\u{1d1aa}',
    '\u{1d1ab}',
    '\u{1d1ac}',
    '\u{1d1ad}',
    '\u{1d242}',
    '\u{1d243}',
    '\u{1d244}',
];

pub(crate) fn kitty_placeholder_diacritic_value(character: char) -> Option<usize> {
    KITTY_PLACEHOLDER_DIACRITICS
        .iter()
        .position(|candidate| *candidate == character)
}

#[cfg(test)]
pub(crate) fn kitty_placeholder_color_value(color: &Color) -> Option<u32> {
    match color {
        Color::Indexed(value) => Some(*value as u32),
        Color::Spec(Rgb { r, g, b }) => Some((*r as u32) << 16 | (*g as u32) << 8 | *b as u32),
        Color::Named(_) => None,
    }
}

#[cfg(test)]
pub(crate) fn decode_kitty_placeholder(
    cell: &Cell,
    zero_width: &str,
    viewport_row: usize,
    viewport_column: usize,
    previous: Option<KittyPlaceholderState>,
) -> Option<(KittyPlaceholder, KittyPlaceholderState)> {
    if cell.c != KITTY_PLACEHOLDER_CHAR {
        return None;
    }
    let foreground = kitty_placeholder_color_value(&cell.fg)?;
    let underline = cell
        .underline_color()
        .and_then(|color| kitty_placeholder_color_value(&color));
    let marks = zero_width
        .chars()
        .filter_map(kitty_placeholder_diacritic_value)
        .take(3)
        .collect::<Vec<_>>();
    let same_colors = previous.is_some_and(|previous| {
        previous.foreground == foreground && previous.underline == underline
    });
    let previous = previous
        .filter(|previous| previous.foreground == foreground && previous.underline == underline);

    let (row, column, image_id_high) = match marks.as_slice() {
        [] => {
            let previous = previous?;
            (
                previous.row,
                previous.column.checked_add(1)?,
                previous.image_id_high,
            )
        }
        [row] => {
            let column = if same_colors && previous.is_some_and(|previous| previous.row == *row) {
                previous?.column.checked_add(1)?
            } else {
                0
            };
            let image_id_high = previous
                .filter(|previous| previous.row == *row)
                .map(|previous| previous.image_id_high)
                .unwrap_or(0);
            (*row, column, image_id_high)
        }
        [row, column] => {
            let image_id_high = previous
                .filter(|previous| {
                    previous.row == *row && previous.column.checked_add(1) == Some(*column)
                })
                .map(|previous| previous.image_id_high)
                .unwrap_or(0);
            (*row, *column, image_id_high)
        }
        [row, column, image_id_high] => (*row, *column, u8::try_from(*image_id_high).ok()?),
        _ => unreachable!(),
    };
    let image_id = (foreground & 0x00ff_ffff) | (u32::from(image_id_high) << 24);
    let state = KittyPlaceholderState {
        foreground,
        underline,
        row,
        column,
        image_id_high,
    };
    Some((
        KittyPlaceholder {
            image_id,
            placement_id: underline.filter(|value| *value != 0),
            row,
            column,
            viewport_row,
            viewport_column,
        },
        state,
    ))
}

fn decode_zed_kitty_placeholder(
    cell: &zed_terminal::Cell,
    zero_width: &str,
    viewport_row: usize,
    viewport_column: usize,
    previous: Option<KittyPlaceholderState>,
) -> Option<(KittyPlaceholder, KittyPlaceholderState)> {
    if cell.character() != KITTY_PLACEHOLDER_CHAR {
        return None;
    }
    let foreground = match cell.foreground() {
        zed_terminal::Color::Indexed(value) => value as u32,
        zed_terminal::Color::Spec(zed_terminal::Rgb { r, g, b }) => {
            (r as u32) << 16 | (g as u32) << 8 | b as u32
        }
        zed_terminal::Color::Named(_) => return None,
    };
    let marks = zero_width
        .chars()
        .filter_map(kitty_placeholder_diacritic_value)
        .take(3)
        .collect::<Vec<_>>();
    let previous = previous.filter(|previous| previous.foreground == foreground);
    let (row, column, image_id_high) = match marks.as_slice() {
        [] => {
            let previous = previous?;
            (
                previous.row,
                previous.column.checked_add(1)?,
                previous.image_id_high,
            )
        }
        [row] => {
            let column = previous
                .filter(|previous| previous.row == *row)
                .and_then(|previous| previous.column.checked_add(1))
                .unwrap_or(0);
            let image_id_high = previous
                .filter(|previous| previous.row == *row)
                .map(|previous| previous.image_id_high)
                .unwrap_or(0);
            (*row, column, image_id_high)
        }
        [row, column] => {
            let image_id_high = previous
                .filter(|previous| {
                    previous.row == *row && previous.column.checked_add(1) == Some(*column)
                })
                .map(|previous| previous.image_id_high)
                .unwrap_or(0);
            (*row, *column, image_id_high)
        }
        [row, column, image_id_high] => (*row, *column, u8::try_from(*image_id_high).ok()?),
        _ => unreachable!(),
    };
    let state = KittyPlaceholderState {
        foreground,
        underline: None,
        row,
        column,
        image_id_high,
    };
    Some((
        KittyPlaceholder {
            image_id: (foreground & 0x00ff_ffff) | (u32::from(image_id_high) << 24),
            placement_id: None,
            row,
            column,
            viewport_row,
            viewport_column,
        },
        state,
    ))
}

/// 把 Term 可见区快照成 owned 数据。
#[cfg(test)]
pub(crate) fn snapshot_visible(
    term: &Term<NoopListener>,
    selection: Option<((usize, usize), (usize, usize))>,
    _cols: usize,
    cursor_visible: bool,
    timestamps: &[Option<String>],
) -> Snapshot {
    let grid = term.grid();
    let display_offset = grid.display_offset();
    let cols = term.columns();
    let rows = term.screen_lines();
    let top_visible = Line(-(display_offset as i32));
    let colors = term.colors();
    let default_fg = fg_of(term);
    let default_bg = bg_of(term);
    let cursor_shape = term.cursor_style().shape;

    log::trace!(
        "snapshot_visible: display_offset={} top_visible={} cols={} rows={} total_lines={}",
        display_offset,
        top_visible.0,
        cols,
        rows,
        grid.total_lines()
    );

    let mut out_rows: Vec<Vec<RenderCell>> = Vec::with_capacity(rows);
    let mut kitty_placeholders = Vec::new();
    for r in 0..rows {
        let line = Line(top_visible.0 + r as i32);
        let row = &grid[line];
        let mut out: Vec<RenderCell> = Vec::with_capacity(cols);
        let mut previous_placeholder = None;
        for c in 0..cols {
            let cell: &Cell = &row[Column(c)];
            let style = effective_cell_style(cell, colors, default_fg, default_bg);
            let mut zero_width = String::new();
            if let Some(chars) = cell.zerowidth() {
                zero_width.extend(chars.iter().copied());
            }
            let kitty_placeholder =
                decode_kitty_placeholder(cell, &zero_width, r, c, previous_placeholder)
                    .map(|(placeholder, state)| {
                        kitty_placeholders.push(placeholder);
                        previous_placeholder = Some(state);
                        true
                    })
                    .unwrap_or_else(|| {
                        previous_placeholder = None;
                        false
                    });
            out.push(RenderCell {
                ch: if cell.c == '\0' { ' ' } else { cell.c },
                fg: style.fg,
                bg: style.bg,
                bold: style.bold,
                italic: style.italic,
                underline: style.underline,
                underline_color: style.underline_color,
                strikeout: style.strikeout,
                spacer: cell
                    .flags
                    .intersects(CellFlags::WIDE_CHAR_SPACER | CellFlags::LEADING_WIDE_CHAR_SPACER),
                wide: cell.flags.contains(CellFlags::WIDE_CHAR),
                zero_width,
                kitty_placeholder,
                is_url: false,
                hyperlink: cell.hyperlink().map(|link| link.uri().to_string()),
            });
        }
        out_rows.push(out);
    }

    // 光标位置（视口内）。
    let cursor = cursor_viewport_position(
        grid.cursor.point.line.0,
        grid.cursor.point.column.0,
        display_offset,
        rows,
        cols,
    );

    let display_offset = grid.display_offset();
    let history_len = grid.history_size();
    // URL 检测与标记。
    let mut urls: Vec<(usize, usize, usize, String)> = Vec::new();
    for vy in 0..rows.min(out_rows.len()) {
        // OSC 8 hyperlinks are authoritative: preserve their target even
        // when the visible label does not contain a URL.
        let mut col = 0;
        while col < out_rows[vy].len() {
            let Some(url) = out_rows[vy][col].hyperlink.clone() else {
                col += 1;
                continue;
            };
            let start = col;
            while col < out_rows[vy].len()
                && out_rows[vy][col].hyperlink.as_deref() == Some(url.as_str())
            {
                out_rows[vy][col].is_url = true;
                col += 1;
            }
            urls.push((vy, start, col, url));
        }

        urls.extend(detect_plain_urls(&mut out_rows[vy], vy));
    }

    Snapshot {
        rows: out_rows,
        cursor,
        selection,
        cols,
        display_offset,
        history_len,
        cursor_visible,
        cursor_shape,
        urls,
        timestamps: timestamps.to_vec(),
        kitty_placeholders,
    }
}

/// Snapshot the viewport exposed by Zed's terminal core. `Content.cells` uses
/// terminal-relative line coordinates, so adding `display_offset` maps cells
/// directly onto the current viewport rows.
pub(crate) fn snapshot_visible_content(
    content: &zed_terminal::Content,
    selection: Option<((usize, usize), (usize, usize))>,
    cursor_visible: bool,
    timestamps: &[Option<String>],
    history_len: usize,
) -> Snapshot {
    let cols = content.terminal_bounds.num_columns();
    let rows = content.terminal_bounds.num_lines();
    let default_fg = fg_of_content(content);
    let default_bg = bg_of_content(content);
    let mut out_rows = vec![vec![default_render_cell(default_fg, default_bg); cols]; rows];
    let mut previous_placeholders = vec![None; rows];
    let mut kitty_placeholders = Vec::new();

    for indexed in &content.cells {
        let Some(viewport_row) = indexed
            .point
            .line
            .checked_add(content.display_offset as i32)
            .and_then(|line| usize::try_from(line).ok())
            .filter(|row| *row < rows)
        else {
            continue;
        };
        let column = indexed.point.column;
        if column >= cols {
            continue;
        }
        let cell = &indexed.cell;
        let style = effective_zed_cell_style(cell);
        let mut zero_width = String::new();
        if let Some(chars) = cell.zerowidth() {
            zero_width.extend(chars.iter().copied());
        }
        let kitty_placeholder = decode_zed_kitty_placeholder(
            cell,
            &zero_width,
            viewport_row,
            column,
            previous_placeholders[viewport_row],
        )
        .map(|(placeholder, state)| {
            kitty_placeholders.push(placeholder);
            previous_placeholders[viewport_row] = Some(state);
            true
        })
        .unwrap_or_else(|| {
            previous_placeholders[viewport_row] = None;
            false
        });
        let wide = content.cells.iter().any(|next| {
            next.point.line == indexed.point.line
                && next.point.column == column.saturating_add(1)
                && next.is_wide_char_spacer()
        });
        out_rows[viewport_row][column] = RenderCell {
            ch: if cell.character() == '\0' {
                ' '
            } else {
                cell.character()
            },
            fg: style.fg,
            bg: style.bg,
            bold: style.bold,
            italic: style.italic,
            underline: style.underline,
            underline_color: style.underline_color,
            strikeout: style.strikeout,
            spacer: cell.is_wide_char_spacer(),
            wide,
            zero_width,
            kitty_placeholder,
            is_url: false,
            hyperlink: cell.hyperlink().map(|link| link.uri().to_owned()),
        };
    }

    let cursor = cursor_viewport_position(
        content.cursor.point.line,
        content.cursor.point.column,
        content.display_offset,
        rows,
        cols,
    );
    let mut urls = Vec::new();
    for (row, out_row) in out_rows.iter_mut().enumerate().take(rows) {
        let mut col = 0;
        while col < out_row.len() {
            let Some(url) = out_row[col].hyperlink.clone() else {
                col += 1;
                continue;
            };
            let start = col;
            while col < out_row.len() && out_row[col].hyperlink.as_deref() == Some(url.as_str()) {
                out_row[col].is_url = true;
                col += 1;
            }
            urls.push((row, start, col, url));
        }
        urls.extend(detect_plain_urls(out_row, row));
    }

    Snapshot {
        rows: out_rows,
        cursor,
        selection,
        cols,
        display_offset: content.display_offset,
        history_len,
        cursor_visible,
        cursor_shape: zed_cursor_shape(content.cursor.shape),
        urls,
        timestamps: timestamps.to_vec(),
        kitty_placeholders,
    }
}

fn default_render_cell(fg: Hsla, bg: Hsla) -> RenderCell {
    RenderCell {
        ch: ' ',
        fg,
        bg,
        bold: false,
        italic: false,
        underline: UnderlineKind::None,
        underline_color: fg,
        strikeout: false,
        spacer: false,
        wide: false,
        zero_width: String::new(),
        kitty_placeholder: false,
        is_url: false,
        hyperlink: None,
    }
}

fn zed_cursor_shape(shape: zed_terminal::CursorShape) -> CursorShape {
    match shape {
        zed_terminal::CursorShape::Block => CursorShape::Block,
        zed_terminal::CursorShape::Underline => CursorShape::Underline,
        zed_terminal::CursorShape::Bar => CursorShape::Beam,
        zed_terminal::CursorShape::HollowBlock => CursorShape::HollowBlock,
        zed_terminal::CursorShape::Hidden => CursorShape::Hidden,
    }
}

pub(crate) fn bg_of_content(_content: &zed_terminal::Content) -> Hsla {
    zed_color_to_hsla(&zed_terminal::Color::Named(
        zed_terminal::NamedColor::Background,
    ))
}

pub(crate) fn fg_of_content(_content: &zed_terminal::Content) -> Hsla {
    zed_color_to_hsla(&zed_terminal::Color::Named(
        zed_terminal::NamedColor::Foreground,
    ))
}

#[cfg(test)]
pub(crate) fn bg_of(term: &Term<NoopListener>) -> Hsla {
    color_to_hsla(&Color::Named(NamedColor::Background), term.colors())
        .unwrap_or_else(|| default_palette(&NamedColor::Background))
}
#[cfg(test)]
pub(crate) fn fg_of(term: &Term<NoopListener>) -> Hsla {
    color_to_hsla(&Color::Named(NamedColor::Foreground), term.colors())
        .unwrap_or_else(|| default_palette(&NamedColor::Foreground))
}

const MAX_KITTY_NOTIFICATION_TEXT_BYTES: usize = 8 * 1024;

pub(crate) fn append_bounded_notification_text(target: &mut String, value: &str) {
    let remaining = MAX_KITTY_NOTIFICATION_TEXT_BYTES.saturating_sub(target.len());
    if remaining == 0 {
        return;
    }
    let mut end = remaining.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    target.push_str(&value[..end]);
}

#[cfg(test)]
pub(crate) fn color_to_hsla(
    color: &Color,
    colors: &alacritty_terminal::term::color::Colors,
) -> Option<Hsla> {
    match color {
        Color::Spec(rgb) => Some(rgb_to_hsla(*rgb)),
        Color::Named(n) => {
            let idx = *n as usize;
            colors[idx]
                .map(rgb_to_hsla)
                .or_else(|| Some(default_palette(n)))
        }
        Color::Indexed(i) => {
            let idx = *i as usize;
            if idx < 256 {
                colors[idx]
                    .map(rgb_to_hsla)
                    .or_else(|| Some(default_palette_indexed(idx)))
            } else {
                None
            }
        }
    }
}

pub(crate) fn rgb_to_hsla(Rgb { r, g, b }: Rgb) -> Hsla {
    Hsla::from(gpui::rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32))
}

/// 内置默认 16 色调色板（极简）。
pub(crate) fn default_palette(n: &NamedColor) -> Hsla {
    rgb_to_hsla(default_palette_rgb(n))
}

pub(crate) fn default_palette_rgb(n: &NamedColor) -> Rgb {
    use NamedColor::*;
    let rgb = match n {
        Black | DimBlack => [0x00, 0x00, 0x00],
        Red | DimRed => [0xc5, 0x28, 0x28],
        Green | DimGreen => [0x23, 0xa1, 0x2e],
        Yellow | DimYellow => [0xc0, 0x8c, 0x1e],
        Blue | DimBlue => [0x10, 0x7d, 0xcf],
        Magenta | DimMagenta => [0xbe, 0x3e, 0xbe],
        Cyan | DimCyan => [0x12, 0x9a, 0xa1],
        White | DimWhite => [0xc0, 0xc0, 0xc0],
        BrightBlack => [0x76, 0x76, 0x76],
        BrightRed => [0xff, 0x6b, 0x6b],
        BrightGreen => [0x52, 0xd4, 0x52],
        BrightYellow => [0xff, 0xd1, 0x73],
        BrightBlue => [0x6b, 0xb6, 0xff],
        BrightMagenta => [0xff, 0x7e, 0xff],
        BrightCyan => [0x6b, 0xe7, 0xeb],
        BrightWhite | BrightForeground => [0xff, 0xff, 0xff],
        Foreground => [0xe7, 0xed, 0xf1],
        Background => [0x0f, 0x11, 0x14],
        Cursor => [0x69, 0xd7, 0xb0],
        DimForeground => [0x9a, 0xa6, 0xb0],
    };
    Rgb {
        r: rgb[0],
        g: rgb[1],
        b: rgb[2],
    }
}

/// 256 色的回退（xterm 配色：16 色 + 6×6×6 立方 + 24 级灰度）。
pub(crate) fn default_palette_indexed(i: usize) -> Hsla {
    rgb_to_hsla(default_palette_indexed_rgb(i))
}

pub(crate) fn default_palette_indexed_rgb(i: usize) -> Rgb {
    if i < 16 {
        // 前 16 色用内置调色板里的对应项。
        let n = match i {
            0 => NamedColor::Black,
            1 => NamedColor::Red,
            2 => NamedColor::Green,
            3 => NamedColor::Yellow,
            4 => NamedColor::Blue,
            5 => NamedColor::Magenta,
            6 => NamedColor::Cyan,
            7 => NamedColor::White,
            8 => NamedColor::BrightBlack,
            9 => NamedColor::BrightRed,
            10 => NamedColor::BrightGreen,
            11 => NamedColor::BrightYellow,
            12 => NamedColor::BrightBlue,
            13 => NamedColor::BrightMagenta,
            14 => NamedColor::BrightCyan,
            _ => NamedColor::BrightWhite,
        };
        default_palette_rgb(&n)
    } else if i < 232 {
        let i = i - 16;
        let r = i / 36;
        let g = (i / 6) % 6;
        let b = i % 6;
        let v = |x: usize| if x == 0 { 0 } else { 0x37 + 0x28 * x };
        Rgb {
            r: v(r) as u8,
            g: v(g) as u8,
            b: v(b) as u8,
        }
    } else {
        let v = 8 + (i - 232) * 10;
        Rgb {
            r: v.min(255) as u8,
            g: v.min(255) as u8,
            b: v.min(255) as u8,
        }
    }
}

pub(crate) fn dimen(c: Hsla) -> Hsla {
    Hsla { a: c.a * 0.6, ..c }
}

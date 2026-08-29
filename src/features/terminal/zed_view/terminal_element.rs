//! Derived from Zed's terminal_view TerminalElement at revision
//! 90d024b88abc91264d9a0ad260eb4f365fa695c3. Application-only editor and
//! workspace integrations are intentionally omitted from this fork. The
//! terminal-local context menu is routed through TerminalView below.
// SPDX-License-Identifier: GPL-3.0-or-later

use gpui::{
    AbsoluteLength, App, Bounds, ContentMask, Context, Corners, DispatchPhase, Edges, Element,
    ElementId, Entity, FocusHandle, Font, FontFeatures, FontStyle, FontWeight, GlobalElementId,
    HighlightStyle, Hitbox, Hsla, InputHandler, InteractiveElement, Interactivity, IntoElement,
    LayoutId, Length, ModifiersChangedEvent, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels,
    Point as GpuiPoint, StatefulInteractiveElement, StrikethroughStyle, TextRun, TextStyle,
    UTF16Selection, UnderlineStyle, WhiteSpace, Window, fill, point, px, quad, relative, size,
};
use itertools::Itertools;
use settings::Settings;
use std::time::Instant;
use terminal::{
    Cell, Color, CursorShape, IndexedCell, Modes, NamedColor, Point, Range, Terminal,
    TerminalBounds, is_app_chosen_exact_color as terminal_is_app_chosen_exact_color,
    is_default_background_color, terminal_settings::TerminalSettings,
};
use theme::{ActiveTheme, Theme};
use theme_settings::ThemeSettings;
use unicode_width::UnicodeWidthChar;
use util::ResultExt;

use std::fmt::Debug;
use std::mem;

use crossh_terminal::timestamps::TerminalRow;

use super::TerminalView;
mod apca_contrast;
use apca_contrast::ensure_minimum_contrast;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum EditorCursorShape {
    Bar,
    #[default]
    Block,
    Underline,
    Hollow,
}

struct CursorLayout {
    origin: GpuiPoint<Pixels>,
    block_width: Pixels,
    line_height: Pixels,
    color: Hsla,
    shape: EditorCursorShape,
    block_text: Option<gpui::ShapedLine>,
}

impl CursorLayout {
    fn new(
        origin: GpuiPoint<Pixels>,
        block_width: Pixels,
        line_height: Pixels,
        color: Hsla,
        shape: EditorCursorShape,
        block_text: Option<gpui::ShapedLine>,
    ) -> Self {
        Self {
            origin,
            block_width,
            line_height,
            color,
            shape,
            block_text,
        }
    }

    fn paint(&mut self, origin: GpuiPoint<Pixels>, window: &mut Window, cx: &mut App) {
        let bounds = match self.shape {
            EditorCursorShape::Bar => Bounds {
                origin: self.origin + origin,
                size: size(px(2.), self.line_height),
            },
            EditorCursorShape::Underline => Bounds {
                origin: self.origin + origin + point(px(0.), self.line_height - px(2.)),
                size: size(self.block_width, px(2.)),
            },
            EditorCursorShape::Block | EditorCursorShape::Hollow => Bounds {
                origin: self.origin + origin,
                size: size(self.block_width, self.line_height),
            },
        };
        let quad = if self.shape == EditorCursorShape::Hollow {
            gpui::outline(bounds, self.color, gpui::BorderStyle::Solid)
        } else {
            fill(bounds, self.color)
        };
        window.paint_quad(quad);
        if let Some(block_text) = &self.block_text {
            block_text
                .paint(
                    self.origin + origin,
                    self.line_height,
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
        }
    }
}

#[derive(Debug)]
struct HighlightedRangeLine {
    start_x: Pixels,
    end_x: Pixels,
}

struct HighlightedRange {
    start_y: Pixels,
    line_height: Pixels,
    lines: Vec<HighlightedRangeLine>,
    color: Hsla,
}

impl HighlightedRange {
    fn paint(&self, window: &mut Window) {
        let radius = px(crossh_ui::theme::RADIUS_SM);
        let min_rounded_width = radius * 2.;
        for (line_index, line) in self.lines.iter().enumerate() {
            let bounds = Bounds::new(
                GpuiPoint::new(
                    line.start_x,
                    self.start_y + line_index as f32 * self.line_height,
                ),
                size(line.end_x - line.start_x, self.line_height),
            );

            // Round only the outer corners of the selection so adjacent lines
            // read as one contiguous block. Skip rounding on lines too narrow
            // to fit the radius, otherwise short spans degenerate into blobs.
            let rounded_line = bounds.size.width >= min_rounded_width;
            let first_line = line_index == 0;
            let last_line = line_index == self.lines.len() - 1;
            let corner = |rounded: bool| {
                if rounded && rounded_line {
                    radius
                } else {
                    px(0.)
                }
            };
            window.paint_quad(quad(
                bounds,
                Corners {
                    top_left: corner(first_line),
                    top_right: corner(first_line),
                    bottom_left: corner(last_line),
                    bottom_right: corner(last_line),
                },
                self.color,
                Edges {
                    top: px(0.),
                    right: px(0.),
                    bottom: px(0.),
                    left: px(0.),
                },
                gpui::transparent_black(),
                gpui::BorderStyle::default(),
            ));
        }
    }
}

/// The color used to highlight the active text selection.
///
/// The terminal core's default local-player color (`blue().dark().step_3()`,
/// roughly `#0d2847`) is nearly the same lightness as the terminal background
/// and reads as invisible. Crossh's selection color keeps the highlight
/// clearly visible while leaving glyphs legible underneath.
fn selection_highlight_color() -> Hsla {
    crossh_ui::theme::selection()
}

fn apply_hovered_link_style(
    point: Point,
    hyperlink: Option<(HighlightStyle, &Range)>,
    text_run: &mut TextRun,
) {
    if let Some((style, range)) = hyperlink
        && range.contains(point)
    {
        if let Some(underline) = style.underline {
            text_run.underline = Some(underline);
        }
        if let Some(color) = style.color {
            text_run.color = color;
        }
    }
}

const TIMESTAMP_GUTTER_WIDTH: f32 = 104.0;
const TIMESTAMP_GUTTER_GAP: f32 = 8.0;
const TIMESTAMP_GUTTER_PADDING: f32 = 8.0;

fn timestamp_rows(cells: &[IndexedCell], row_count: usize, columns: usize) -> Vec<TerminalRow> {
    let mut rows = Vec::with_capacity(row_count);
    let mut current_line = None;
    let mut text = String::with_capacity(columns);

    for indexed_cell in cells {
        if current_line != Some(indexed_cell.point.line) {
            if current_line.is_some() {
                rows.push(TerminalRow::new(std::mem::take(&mut text)));
            }
            current_line = Some(indexed_cell.point.line);
        }

        if indexed_cell.cell.is_wide_char_spacer() {
            continue;
        }

        let character = indexed_cell.cell.character();
        text.push(if character == '\0' { ' ' } else { character });
        if let Some(zero_width) = indexed_cell.cell.zerowidth() {
            text.extend(zero_width.iter().copied());
        }
    }

    if current_line.is_some() {
        rows.push(TerminalRow::new(text));
    }

    rows.resize_with(row_count, TerminalRow::default);
    rows.truncate(row_count);
    rows
}

fn paint_timestamp_gutter(
    timestamps: &[Option<String>],
    canvas_bounds: Bounds<Pixels>,
    terminal_bounds: &TerminalBounds,
    line_height: Pixels,
    text_style: &TextStyle,
    window: &mut Window,
    cx: &mut App,
) {
    let reserved_width = terminal_bounds.bounds.origin.x - canvas_bounds.origin.x;
    let gap = px(TIMESTAMP_GUTTER_GAP);
    let padding = px(TIMESTAMP_GUTTER_PADDING);
    let gutter_width = reserved_width - gap;
    if gutter_width <= px(1.) {
        return;
    }

    let divider_bounds = Bounds {
        origin: point(
            terminal_bounds.bounds.origin.x - gap - px(1.),
            canvas_bounds.origin.y,
        ),
        size: size(px(1.), canvas_bounds.size.height),
    };
    window.paint_quad(fill(divider_bounds, Hsla::from(crossh_ui::theme::border())));

    let text_width = gutter_width - padding;
    if text_width <= px(1.) {
        return;
    }

    let timestamp_color = Hsla {
        a: text_style.color.a * 0.48,
        ..text_style.color
    };
    let font = text_style.font();
    let font_size = (text_style.font_size.to_pixels(window.rem_size()) - px(2.)).max(px(1.));

    for (row, timestamp) in timestamps.iter().enumerate() {
        let Some(timestamp) = timestamp else {
            continue;
        };

        let shaped = window.text_system().shape_line(
            timestamp.clone().into(),
            font_size,
            &[TextRun {
                len: timestamp.len(),
                font: font.clone(),
                color: timestamp_color,
                ..Default::default()
            }],
            None,
        );
        shaped
            .paint(
                point(
                    canvas_bounds.origin.x + padding / 2.,
                    terminal_bounds.bounds.origin.y + row as f32 * line_height,
                ),
                line_height,
                gpui::TextAlign::Right,
                Some(text_width),
                window,
                cx,
            )
            .log_err();
    }
}

/// The information generated during layout that is necessary for painting.
pub struct LayoutState {
    hitbox: Hitbox,
    batched_text_runs: Vec<BatchedTextRun>,
    block_element_rects: Vec<BlockElementLayoutRect>,
    rects: Vec<LayoutRect>,
    relative_highlighted_ranges: Vec<(Range, Hsla)>,
    cursor: Option<CursorLayout>,
    ime_cursor_bounds: Option<Bounds<Pixels>>,
    background_color: Hsla,
    dimensions: TerminalBounds,
    mode: Modes,
    display_offset: usize,
    hovered_link: bool,
    base_text_style: TextStyle,
    timestamp_rows: Vec<Option<String>>,
}

/// Helper struct for converting terminal cursor points to displayed cursor points.
#[derive(Copy, Clone)]
struct DisplayCursor {
    line: i32,
    col: usize,
}

impl DisplayCursor {
    fn from(cursor_point: Point, display_offset: usize) -> Self {
        Self {
            line: cursor_point.line + display_offset as i32,
            col: cursor_point.column,
        }
    }

    pub fn line(&self) -> i32 {
        self.line
    }

    pub fn col(&self) -> usize {
        self.col
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub struct LayoutPoint {
    line: i32,
    column: i32,
}

impl LayoutPoint {
    fn new(line: i32, column: i32) -> Self {
        Self { line, column }
    }

    #[cfg(test)]
    fn line(&self) -> i32 {
        self.line
    }

    #[cfg(test)]
    fn column(&self) -> i32 {
        self.column
    }
}

/// A batched text run that combines multiple adjacent cells with the same style
#[derive(Debug)]
pub struct BatchedTextRun {
    pub start_point: LayoutPoint,
    pub text: String,
    pub cell_count: usize,
    pub style: TextRun,
    pub font_size: AbsoluteLength,
}

impl BatchedTextRun {
    fn new_from_char(
        start_point: LayoutPoint,
        c: char,
        style: TextRun,
        font_size: AbsoluteLength,
    ) -> Self {
        let mut text = String::with_capacity(100); // Pre-allocate for typical line length
        text.push(c);
        BatchedTextRun {
            start_point,
            text,
            cell_count: 1,
            style,
            font_size,
        }
    }

    fn can_append(&self, other_style: &TextRun, other_font_size: AbsoluteLength) -> bool {
        self.font_size == other_font_size
            && self.style.font == other_style.font
            && self.style.color == other_style.color
            && self.style.background_color == other_style.background_color
            && self.style.underline == other_style.underline
            && self.style.strikethrough == other_style.strikethrough
    }

    fn append_char(&mut self, c: char) {
        self.append_char_internal(c, true);
    }

    fn append_zero_width_chars(&mut self, chars: &[char]) {
        for &c in chars {
            self.append_char_internal(c, false);
        }
    }

    fn append_char_internal(&mut self, c: char, counts_cell: bool) {
        self.text.push(c);
        if counts_cell {
            self.cell_count += 1;
        }
        self.style.len += c.len_utf8();
    }

    pub fn paint(
        &self,
        origin: GpuiPoint<Pixels>,
        dimensions: &TerminalBounds,
        window: &mut Window,
        cx: &mut App,
    ) {
        let pos = GpuiPoint::new(
            origin.x + self.start_point.column as f32 * dimensions.cell_width,
            origin.y + self.start_point.line as f32 * dimensions.line_height,
        );

        window
            .text_system()
            .shape_line(
                self.text.clone().into(),
                self.font_size.to_pixels(window.rem_size()),
                std::slice::from_ref(&self.style),
                Some(dimensions.cell_width),
            )
            .paint(
                pos,
                dimensions.line_height,
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            )
            .log_err();
    }
}

/// 歧义宽度字符缩字入格的判定与缩放（spec 20260826）。
/// `shaped_width` 为字符的自然排版步进（无 force_width），`cell_width` 为终端格宽。
/// 与 gpui `apply_force_width_to_layout` 的 1px 容差保持一致。
const AMBIGUOUS_SHRINK_TOLERANCE: Pixels = px(1.0);

/// 若字形步进超过格宽（加容差），返回缩放因子 (<1)，否则 `None`。
/// 因子 = cell_width / shaped_width，缩放后字形恰好 1 格宽。
pub fn ambiguous_shrink_factor(shaped_width: Pixels, cell_width: Pixels) -> Option<f32> {
    if shaped_width > cell_width + AMBIGUOUS_SHRINK_TOLERANCE {
        let factor = f32::from(cell_width) / f32::from(shaped_width);
        // 因子 (0,1) 且有限
        if factor > 0.0 && factor < 1.0 && factor.is_finite() {
            return Some(factor);
        }
    }
    None
}

/// Block element glyphs are painted on a subcell grid: each terminal cell is
/// divided into 8 columns (for eighth blocks) and 24 lines (LCM of the 8-way
/// splits of eighth blocks and the 3-way splits of sextants).
const BLOCK_SUBCELL_COLUMNS: i32 = 8;
const BLOCK_SUBCELL_LINES: i32 = 24;

#[derive(Clone, Debug)]
pub struct BlockElementLayoutRect {
    point: LayoutPoint,
    num_of_columns: usize,
    num_of_lines: usize,
    color: Hsla,
}

impl BlockElementLayoutRect {
    fn new(point: LayoutPoint, num_of_columns: usize, num_of_lines: usize, color: Hsla) -> Self {
        Self {
            point,
            num_of_columns,
            num_of_lines,
            color,
        }
    }

    pub fn paint(
        &self,
        origin: GpuiPoint<Pixels>,
        dimensions: &TerminalBounds,
        window: &mut Window,
    ) {
        let subcell_width = dimensions.cell_width / BLOCK_SUBCELL_COLUMNS as f32;
        let subcell_height = dimensions.line_height / BLOCK_SUBCELL_LINES as f32;
        let position = point(
            origin.x + self.point.column as f32 * subcell_width,
            origin.y + self.point.line as f32 * subcell_height,
        );
        let size = size(
            subcell_width * self.num_of_columns as f32,
            subcell_height * self.num_of_lines as f32,
        );

        window.paint_quad(fill(Bounds::new(position, size), self.color));
    }

    #[cfg(test)]
    fn line(&self) -> i32 {
        (self.point.line + self.num_of_lines as i32 - 1) / BLOCK_SUBCELL_LINES
    }
}

#[derive(Clone, Debug, Default)]
pub struct LayoutRect {
    point: LayoutPoint,
    num_of_cells: usize,
    color: Hsla,
}

impl LayoutRect {
    fn new(point: LayoutPoint, num_of_cells: usize, color: Hsla) -> LayoutRect {
        LayoutRect {
            point,
            num_of_cells,
            color,
        }
    }

    pub fn paint(
        &self,
        origin: GpuiPoint<Pixels>,
        dimensions: &TerminalBounds,
        window: &mut Window,
    ) {
        let position = {
            let layout_point = self.point;
            point(
                (origin.x + layout_point.column as f32 * dimensions.cell_width).floor(),
                origin.y + layout_point.line as f32 * dimensions.line_height,
            )
        };
        let size = point(
            (dimensions.cell_width * self.num_of_cells as f32).ceil(),
            dimensions.line_height,
        )
        .into();

        window.paint_quad(fill(Bounds::new(position, size), self.color));
    }
}

/// Represents a rectangular region with a specific color on a logical grid.
#[derive(Debug, Clone)]
struct BackgroundRegion {
    start_line: i32,
    start_col: i32,
    end_line: i32,
    end_col: i32,
    color: Hsla,
}

impl BackgroundRegion {
    fn new(line: i32, col: i32, color: Hsla) -> Self {
        BackgroundRegion {
            start_line: line,
            start_col: col,
            end_line: line,
            end_col: col,
            color,
        }
    }

    fn with_extents(
        start_line: i32,
        start_col: i32,
        end_line: i32,
        end_col: i32,
        color: Hsla,
    ) -> Self {
        BackgroundRegion {
            start_line,
            start_col,
            end_line,
            end_col,
            color,
        }
    }

    /// Check if this region can be merged with another region
    fn can_merge_with(&self, other: &BackgroundRegion) -> bool {
        if self.color != other.color {
            return false;
        }

        // Check if regions are adjacent horizontally
        if self.start_line == other.start_line && self.end_line == other.end_line {
            return self.end_col + 1 == other.start_col || other.end_col + 1 == self.start_col;
        }

        // Check if regions are adjacent vertically with same column span
        if self.start_col == other.start_col && self.end_col == other.end_col {
            return self.end_line + 1 == other.start_line || other.end_line + 1 == self.start_line;
        }

        false
    }

    /// Merge this region with another region
    fn merge_with(&mut self, other: &BackgroundRegion) {
        self.start_line = self.start_line.min(other.start_line);
        self.start_col = self.start_col.min(other.start_col);
        self.end_line = self.end_line.max(other.end_line);
        self.end_col = self.end_col.max(other.end_col);
    }
}

pub trait TerminalLayoutCell {
    fn point(&self) -> Point;
    fn cell(&self) -> &Cell;
}

impl TerminalLayoutCell for IndexedCell {
    fn point(&self) -> Point {
        self.point
    }

    fn cell(&self) -> &Cell {
        &self.cell
    }
}

impl TerminalLayoutCell for &IndexedCell {
    fn point(&self) -> Point {
        self.point
    }

    fn cell(&self) -> &Cell {
        &self.cell
    }
}

/// Merge grid regions to minimize the number of rectangles.
fn merge_background_regions(regions: Vec<BackgroundRegion>) -> Vec<BackgroundRegion> {
    if regions.is_empty() {
        return regions;
    }

    let mut merged = regions;
    let mut changed = true;

    // Keep merging until no more merges are possible
    while changed {
        changed = false;
        let mut i = 0;

        while i < merged.len() {
            let mut j = i + 1;
            while j < merged.len() {
                if merged[i].can_merge_with(&merged[j]) {
                    let other = merged.remove(j);
                    merged[i].merge_with(&other);
                    changed = true;
                } else {
                    j += 1;
                }
            }
            i += 1;
        }
    }

    merged
}

/// The GPUI element that paints the terminal.
/// We need to keep a reference to the model for mouse events, do we need it for any other terminal stuff, or can we move that to connection?
pub struct TerminalElement {
    terminal: Entity<Terminal>,
    terminal_view: Entity<TerminalView>,
    focus: FocusHandle,
    focused: bool,
    cursor_visible: bool,
    interactivity: Interactivity,
}

impl InteractiveElement for TerminalElement {
    fn interactivity(&mut self) -> &mut Interactivity {
        &mut self.interactivity
    }
}

impl StatefulInteractiveElement for TerminalElement {}

impl TerminalElement {
    pub fn new(
        terminal: Entity<Terminal>,
        terminal_view: Entity<TerminalView>,
        focus: FocusHandle,
        focused: bool,
        cursor_visible: bool,
    ) -> TerminalElement {
        TerminalElement {
            terminal,
            terminal_view,
            focused,
            focus: focus.clone(),
            cursor_visible,
            interactivity: Default::default(),
        }
        .track_focus(&focus)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn layout_grid<T: TerminalLayoutCell>(
        grid: impl Iterator<Item = T>,
        start_line_offset: i32,
        text_style: &TextStyle,
        hyperlink: Option<(HighlightStyle, &Range)>,
        minimum_contrast: f32,
        cell_width: Pixels,
        rem_size: Pixels,
        cx: &App,
    ) -> (
        Vec<LayoutRect>,
        Vec<BatchedTextRun>,
        Vec<BlockElementLayoutRect>,
    ) {
        let start_time = Instant::now();
        let theme = cx.theme();

        // Pre-allocate with estimated capacity to reduce reallocations
        let estimated_cells = grid.size_hint().0;
        let estimated_runs = estimated_cells / 10; // Estimate ~10 cells per run
        let estimated_regions = estimated_cells / 20; // Estimate ~20 cells per background region

        let mut batched_runs = Vec::with_capacity(estimated_runs);
        let mut block_element_regions = Vec::new();
        let mut cell_count = 0;

        // Collect background regions for efficient merging
        let mut background_regions: Vec<BackgroundRegion> = Vec::with_capacity(estimated_regions);
        let mut current_batch: Option<BatchedTextRun> = None;
        let base_font_pixels = text_style.font_size.to_pixels(rem_size);
        let mut shaped_cache: std::collections::HashMap<(char, gpui::FontId), Pixels> =
            std::collections::HashMap::new();

        // First pass: collect all cells and their backgrounds
        let linegroups = grid.into_iter().chunk_by(|cell| cell.point().line);
        for (line_index, (_, line)) in linegroups.into_iter().enumerate() {
            let display_line = start_line_offset + line_index as i32;

            // Flush any existing batch at line boundaries
            if let Some(batch) = current_batch.take() {
                batched_runs.push(batch);
            }

            let mut previous_cell_had_extras = false;
            let mut extra_offset: i32 = 0;

            for cell in line {
                let point = cell.point();
                let cell = cell.cell();
                let mut fg = cell.foreground();
                let mut bg = cell.background();
                if cell.is_inverse() {
                    mem::swap(&mut fg, &mut bg);
                }

                // Collect background regions (skip default background)
                if !is_default_background_color(bg) {
                    let color = convert_color(&bg, theme);
                    let col = point.column as i32;

                    // Try to extend the last region if it's on the same line with the same color
                    if let Some(last_region) = background_regions.last_mut()
                        && last_region.color == color
                        && last_region.start_line == display_line
                        && last_region.end_line == display_line
                        && last_region.end_col + 1 == col
                    {
                        last_region.end_col = col;
                    } else {
                        background_regions.push(BackgroundRegion::new(display_line, col, color));
                    }
                }
                // Skip wide character spacers - they're just placeholders for the second cell of wide characters
                if cell.is_wide_char_spacer() {
                    continue;
                }

                // Skip spaces that follow cells with extras (emoji variation sequences)
                if cell.character() == ' ' && previous_cell_had_extras {
                    previous_cell_had_extras = false;
                    continue;
                }
                // Update tracking for next iteration
                previous_cell_had_extras =
                    matches!(cell.zerowidth(), Some(chars) if !chars.is_empty());

                //Layout current cell text
                {
                    if !is_blank(cell) {
                        cell_count += 1;
                        let mut cell_style = TerminalElement::cell_style(
                            cell,
                            fg,
                            bg,
                            theme,
                            text_style,
                            minimum_contrast,
                        );
                        apply_hovered_link_style(point, hyperlink, &mut cell_style);

                        let original_col = point.column as i32;
                        let render_col = original_col + extra_offset;
                        let cell_point = LayoutPoint::new(display_line, render_col);
                        if Self::collect_block_element_regions(
                            cell_point,
                            cell.character(),
                            cell_style.color,
                            &mut block_element_regions,
                        ) {
                            if let Some(batch) = current_batch.take() {
                                batched_runs.push(batch);
                            }
                            continue;
                        }

                        let ch = cell.character();
                        let mut is_overwide = false;
                        if !ch.is_ascii() && ch != ' ' && ch.width().unwrap_or(1) == 1 {
                            let font_id = cx.text_system().resolve_font(&cell_style.font);
                            let shaped = *shaped_cache.entry((ch, font_id)).or_insert_with(|| {
                                cx.text_system().layout_width(font_id, base_font_pixels, ch)
                            });
                            if ambiguous_shrink_factor(shaped, cell_width).is_some() {
                                is_overwide = true;
                            }
                        }

                        let zero_width_chars = cell.zerowidth();
                        let cell_font_size = text_style.font_size;

                        // Overwide chars never batch with neighbours and occupy 2 cells
                        let can_append = if is_overwide {
                            false
                        } else if let Some(batch) = &current_batch {
                            batch.cell_count == 1
                                && batch.can_append(&cell_style, cell_font_size)
                                && batch.start_point.line == cell_point.line
                                && batch.start_point.column + batch.cell_count as i32
                                    == cell_point.column
                        } else {
                            false
                        };

                        if can_append {
                            let batch = current_batch.as_mut().unwrap();
                            batch.append_char(ch);
                            if let Some(chars) = zero_width_chars {
                                batch.append_zero_width_chars(chars);
                            }
                        } else {
                            if let Some(old_batch) = current_batch.take() {
                                batched_runs.push(old_batch);
                            }
                            let mut new_batch = BatchedTextRun::new_from_char(
                                cell_point,
                                ch,
                                cell_style,
                                cell_font_size,
                            );
                            if is_overwide {
                                new_batch.cell_count = 2;
                            }
                            if let Some(chars) = zero_width_chars {
                                new_batch.append_zero_width_chars(chars);
                            }
                            current_batch = Some(new_batch);
                        }

                        if is_overwide {
                            extra_offset += 1;
                        }
                    };
                }
            }
        }

        // Flush any remaining batch
        if let Some(batch) = current_batch {
            batched_runs.push(batch);
        }

        // Second pass: merge background regions and convert to layout rects
        let region_count = background_regions.len();
        let merged_regions = merge_background_regions(background_regions);
        let mut rects = Vec::with_capacity(merged_regions.len() * 2); // Estimate 2 rects per merged region

        // Convert merged regions to layout rects
        // Since LayoutRect only supports single-line rectangles, we need to split multi-line regions
        for region in merged_regions {
            for line in region.start_line..=region.end_line {
                rects.push(LayoutRect::new(
                    LayoutPoint::new(line, region.start_col),
                    (region.end_col - region.start_col + 1) as usize,
                    region.color,
                ));
            }
        }

        let block_element_region_count = block_element_regions.len();
        let block_element_rects = Self::block_element_regions_to_rects(block_element_regions);
        let layout_time = start_time.elapsed();

        log::debug!(
            "Terminal layout_grid: {} cells processed, \
            {} batched runs created, {} block element rects (from {} regions), {} rects (from {} merged regions), \
            layout took {:?}",
            cell_count,
            batched_runs.len(),
            block_element_rects.len(),
            block_element_region_count,
            rects.len(),
            region_count,
            layout_time
        );

        (rects, batched_runs, block_element_rects)
    }

    /// Computes the cursor position based on the cursor point and terminal dimensions.
    fn cursor_position(
        cursor_point: DisplayCursor,
        size: TerminalBounds,
    ) -> Option<GpuiPoint<Pixels>> {
        if cursor_point.line() < size.num_lines() as i32 {
            // When on pixel boundaries round the origin down
            Some(point(
                (cursor_point.col() as f32 * size.cell_width()).floor(),
                (cursor_point.line() as f32 * size.line_height()).floor(),
            ))
        } else {
            None
        }
    }

    /// Checks if a character is a decorative block/box-like character that should
    /// preserve its exact colors without contrast adjustment.
    ///
    /// This specifically targets characters used as visual connectors, separators,
    /// and borders where color matching with adjacent backgrounds is critical.
    /// Regular icons (git, folders, etc.) are excluded as they need to remain readable.
    ///
    /// Fixes https://github.com/zed-industries/zed/issues/34234
    fn is_decorative_character(ch: char) -> bool {
        matches!(
            ch as u32,
            // Unicode Box Drawing and Block Elements
            0x2500..=0x257F // Box Drawing (└ ┐ ─ │ etc.)
            | 0x2580..=0x259F // Block Elements (▀ ▄ █ ░ ▒ ▓ etc.)
            | 0x25A0..=0x25FF // Geometric Shapes (■ ▶ ● etc. - includes triangular/circular separators)
            | 0x1FB00..=0x1FB3B // Symbols for Legacy Computing sextants used by terminal QR renderers

            // Private Use Area - Powerline separator symbols only
            | 0xE0B0..=0xE0B7 // Powerline separators: triangles (E0B0-E0B3) and half circles (E0B4-E0B7)
            | 0xE0B8..=0xE0BF // Powerline separators: corner triangles
            | 0xE0C0..=0xE0CA // Powerline separators: flames (E0C0-E0C3), pixelated (E0C4-E0C7), and ice (E0C8 & E0CA)
            | 0xE0CC..=0xE0D1 // Powerline separators: honeycombs (E0CC-E0CD) and lego (E0CE-E0D1)
            | 0xE0D2..=0xE0D7 // Powerline separators: trapezoid (E0D2 & E0D4) and inverted triangles (E0D6-E0D7)
        )
    }

    /// Whether the application explicitly picked this foreground color and does not
    /// want it adjusted for contrast: 24-bit true color (`\e[38;2;R;G;Bm`) or a
    /// specific entry in the 256-color palette (`\e[38;5;Nm`) where N >= 16 (the
    /// 6x6x6 cube at 16..=231 and the 24-step grayscale ramp at 232..=255).
    /// Indices 0..=15 still go through contrast adjustment since those map to
    /// theme-defined ANSI colors that can clash with the theme background.
    fn is_app_chosen_exact_color(fg: &Color) -> bool {
        terminal_is_app_chosen_exact_color(*fg)
    }

    /// Returns the filled subcells of a sextant character as a bitmap, where
    /// bit `row * 2 + column` is set when that 2x3 subcell is filled.
    ///
    /// U+1FB00..=U+1FB3B enumerate all 2x3 fill combinations except the four
    /// that already exist as Block Elements (empty, `▌` = 0b010101,
    /// `▐` = 0b101010, and `█` = 0b111111), hence the gap adjustments.
    fn sextant_char_to_filled_bits(ch: char) -> Option<u8> {
        let offset = (ch as u32).checked_sub(0x1FB00)?;
        if offset > 0x3B {
            return None;
        }

        Some((offset + 1 + u32::from(offset >= 20) + u32::from(offset >= 40)) as u8)
    }

    /// Returns the filled quadrants of a quadrant character as a bitmap, where
    /// bit `row * 2 + column` is set when that 2x2 subcell is filled.
    fn quadrant_char_to_filled_bits(ch: char) -> Option<u8> {
        Some(match ch {
            '▘' => 0b0001,
            '▝' => 0b0010,
            '▖' => 0b0100,
            '▗' => 0b1000,
            '▚' => 0b1001,
            '▞' => 0b0110,
            '▛' => 0b0111,
            '▜' => 0b1011,
            '▙' => 0b1101,
            '▟' => 0b1110,
            _ => return None,
        })
    }

    /// Returns `(column, line, num_of_columns, num_of_lines)` in subcell units
    /// for block element characters that consist of a single rectangle.
    fn block_char_to_rect(ch: char) -> Option<(i32, i32, i32, i32)> {
        let codepoint = ch as u32;
        Some(match codepoint {
            // ▀ upper half
            0x2580 => (0, 0, 8, 12),
            // ▁▂▃▄▅▆▇█ lower blocks of 1..=8 eighths
            0x2581..=0x2588 => {
                let eighths = (codepoint - 0x2580) as i32;
                (0, 24 - eighths * 3, 8, eighths * 3)
            }
            // ▉▊▋▌▍▎▏ left blocks of 7..=1 eighths
            0x2589..=0x258F => (0, 0, (0x2590 - codepoint) as i32, 24),
            // ▐ right half
            0x2590 => (4, 0, 4, 24),
            // ▔ upper eighth
            0x2594 => (0, 0, 8, 3),
            // ▕ right eighth
            0x2595 => (7, 0, 1, 24),
            _ => return None,
        })
    }

    /// Approximates the shade characters `░▒▓` with the foreground color at
    /// reduced opacity instead of the stipple patterns fonts use, trading
    /// pattern fidelity for seamless cell coverage.
    fn shade_char_to_opacity(ch: char) -> Option<f32> {
        match ch {
            '░' => Some(0.25),
            '▒' => Some(0.5),
            '▓' => Some(0.75),
            _ => None,
        }
    }

    fn collect_block_element_regions(
        point: LayoutPoint,
        ch: char,
        color: Hsla,
        regions: &mut Vec<BackgroundRegion>,
    ) -> bool {
        if let Some((column, line, num_of_columns, num_of_lines)) = Self::block_char_to_rect(ch) {
            Self::push_block_element_region(
                point,
                column,
                line,
                num_of_columns,
                num_of_lines,
                color,
                regions,
            );
            return true;
        }

        if let Some(filled) = Self::quadrant_char_to_filled_bits(ch) {
            for row in 0..2 {
                for column in 0..2 {
                    if filled & (1 << (row * 2 + column)) != 0 {
                        Self::push_block_element_region(
                            point,
                            column * 4,
                            row * 12,
                            4,
                            12,
                            color,
                            regions,
                        );
                    }
                }
            }
            return true;
        }

        if let Some(filled) = Self::sextant_char_to_filled_bits(ch) {
            for row in 0..3 {
                for column in 0..2 {
                    if filled & (1 << (row * 2 + column)) != 0 {
                        Self::push_block_element_region(
                            point,
                            column * 4,
                            row * 8,
                            4,
                            8,
                            color,
                            regions,
                        );
                    }
                }
            }
            return true;
        }

        if let Some(opacity) = Self::shade_char_to_opacity(ch) {
            Self::push_block_element_region(point, 0, 0, 8, 24, color.opacity(opacity), regions);
            return true;
        }

        false
    }

    fn push_block_element_region(
        point: LayoutPoint,
        column: i32,
        line: i32,
        num_of_columns: i32,
        num_of_lines: i32,
        color: Hsla,
        regions: &mut Vec<BackgroundRegion>,
    ) {
        let start_line = point.line * BLOCK_SUBCELL_LINES + line;
        let start_col = point.column * BLOCK_SUBCELL_COLUMNS + column;
        let end_line = start_line + num_of_lines - 1;
        let end_col = start_col + num_of_columns - 1;

        // Extend the previous region when possible (e.g. runs of `█` in a QR
        // code) to keep the quadratic merge pass over a small input.
        if let Some(last_region) = regions.last_mut()
            && last_region.color == color
            && last_region.start_line == start_line
            && last_region.end_line == end_line
            && last_region.end_col + 1 == start_col
        {
            last_region.end_col = end_col;
            return;
        }

        regions.push(BackgroundRegion::with_extents(
            start_line, start_col, end_line, end_col, color,
        ));
    }

    fn block_element_regions_to_rects(
        regions: Vec<BackgroundRegion>,
    ) -> Vec<BlockElementLayoutRect> {
        merge_background_regions(regions)
            .into_iter()
            .map(|region| {
                BlockElementLayoutRect::new(
                    LayoutPoint::new(region.start_line, region.start_col),
                    (region.end_col - region.start_col + 1) as usize,
                    (region.end_line - region.start_line + 1) as usize,
                    region.color,
                )
            })
            .collect()
    }

    /// Converts the Alacritty cell styles to GPUI text styles and background color.
    fn cell_style(
        cell: &Cell,
        fg: Color,
        bg: Color,
        colors: &Theme,
        text_style: &TextStyle,
        minimum_contrast: f32,
    ) -> TextRun {
        let skip_contrast = Self::is_app_chosen_exact_color(&fg);
        let mut fg = convert_color(&fg, colors);
        let bg = convert_color(&bg, colors);

        if !skip_contrast && !Self::is_decorative_character(cell.character()) {
            fg = ensure_minimum_contrast(fg, bg, minimum_contrast);
        }

        // Use a dim multiplier that stays close to the existing Alacritty look.
        if cell.is_dim() {
            fg.a *= 0.7;
        }

        let underline =
            (cell.has_underline() || cell.hyperlink().is_some()).then(|| UnderlineStyle {
                color: Some(fg),
                thickness: Pixels::from(1.0),
                wavy: cell.has_undercurl(),
            });

        let strikethrough = cell.has_strikeout().then(|| StrikethroughStyle {
            color: Some(fg),
            thickness: Pixels::from(1.0),
        });

        let weight = if cell.is_bold() {
            FontWeight::BOLD
        } else {
            text_style.font_weight
        };

        let style = if cell.is_italic() {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        };

        TextRun {
            len: cell.character().len_utf8(),
            color: fg,
            background_color: None,
            font: Font {
                weight,
                style,
                ..text_style.font()
            },
            underline,
            strikethrough,
        }
    }

    fn generic_button_handler<E>(
        connection: Entity<Terminal>,
        focus_handle: FocusHandle,
        steal_focus: bool,
        f: impl Fn(&mut Terminal, &E, &mut Context<Terminal>),
    ) -> impl Fn(&E, &mut Window, &mut App) {
        move |event, window, cx| {
            if steal_focus {
                window.focus(&focus_handle, cx);
            } else if !focus_handle.is_focused(window) {
                return;
            }
            connection.update(cx, |terminal, cx| {
                f(terminal, event, cx);

                cx.notify();
            })
        }
    }

    fn right_button_handler(
        terminal: Entity<Terminal>,
        terminal_view: Entity<TerminalView>,
        focus_handle: FocusHandle,
    ) -> impl Fn(&MouseUpEvent, &mut Window, &mut App) {
        move |event, window, cx| {
            if !focus_handle.is_focused(window) {
                return;
            }

            let forward_to_terminal =
                terminal_view.update(cx, |terminal_view, _| terminal_view.take_right_mouse_down());
            if forward_to_terminal {
                terminal.update(cx, |terminal, terminal_cx| {
                    terminal.mouse_up(event, terminal_cx);
                    terminal_cx.notify();
                });
                cx.stop_propagation();
            }
        }
    }

    fn register_mouse_listeners(&mut self, mode: Modes, hitbox: &Hitbox, window: &mut Window) {
        let focus = self.focus.clone();
        let terminal = self.terminal.clone();
        let terminal_view = self.terminal_view.clone();

        self.interactivity.on_mouse_down(MouseButton::Left, {
            let terminal = terminal.clone();
            let focus = focus.clone();

            move |e, window, cx| {
                window.focus(&focus, cx);
                terminal.update(cx, |terminal, cx| {
                    terminal.mouse_down(e, cx);
                    cx.notify();
                })
            }
        });

        window.on_mouse_event({
            let terminal = self.terminal.clone();
            let hitbox = hitbox.clone();
            let focus = focus.clone();
            move |e: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }

                if e.pressed_button.is_some() && !cx.has_active_drag() && focus.is_focused(window) {
                    let hovered = hitbox.is_hovered(window);

                    terminal.update(cx, |terminal, cx| {
                        if terminal.selection_started() || hovered {
                            terminal.mouse_drag(e, hitbox.bounds, cx);
                            cx.notify();
                        }
                    })
                }

                if hitbox.is_hovered(window) {
                    terminal.update(cx, |terminal, cx| {
                        terminal.mouse_move(e, cx);
                    })
                }
            }
        });

        self.interactivity.on_mouse_up(
            MouseButton::Left,
            TerminalElement::generic_button_handler(
                terminal.clone(),
                focus.clone(),
                false,
                move |terminal, e, cx| {
                    terminal.mouse_up(e, cx);
                },
            ),
        );
        self.interactivity.on_mouse_down(
            MouseButton::Middle,
            TerminalElement::generic_button_handler(
                terminal.clone(),
                focus.clone(),
                true,
                move |terminal, e, cx| {
                    terminal.mouse_down(e, cx);
                },
            ),
        );

        let forwards_right_click = mode.intersects(Modes::MOUSE_MODE);
        self.interactivity.on_mouse_down(MouseButton::Right, {
            let terminal = terminal.clone();
            let terminal_view = terminal_view.clone();
            let focus = focus.clone();
            move |event, window, cx| {
                let forward_to_terminal = forwards_right_click && !event.modifiers.shift;
                if !forward_to_terminal {
                    // Mirror Zed: select the clicked word before opening the
                    // context menu so the Copy entry is available.
                    let had_selection = terminal.read(cx).last_content().selection.is_some();
                    if !had_selection {
                        terminal.update(cx, |terminal, _| {
                            terminal.select_word_at_event_position(event);
                        });
                    }
                }
                terminal_view.update(cx, |terminal_view, terminal_cx| {
                    terminal_view.begin_right_mouse_down(
                        event.position,
                        forward_to_terminal,
                        terminal_cx,
                    );
                });
                if forward_to_terminal {
                    window.focus(&focus, cx);
                    terminal.update(cx, |terminal, terminal_cx| {
                        terminal.mouse_down(event, terminal_cx);
                        terminal_cx.notify();
                    });
                }
                cx.stop_propagation();
            }
        });

        self.interactivity.on_mouse_up(
            MouseButton::Right,
            TerminalElement::right_button_handler(
                terminal.clone(),
                terminal_view.clone(),
                focus.clone(),
            ),
        );
        self.interactivity.on_mouse_up_out(
            MouseButton::Right,
            TerminalElement::right_button_handler(terminal.clone(), terminal_view, focus.clone()),
        );

        self.interactivity.on_scroll_wheel({
            let terminal = self.terminal.clone();
            move |event, _window, cx| {
                terminal.update(cx, |terminal, terminal_cx| {
                    let multiplier = TerminalSettings::get_global(terminal_cx)
                        .scroll_multiplier
                        .max(0.01);
                    terminal.scroll_wheel(event, multiplier);
                    terminal_cx.notify();
                });
            }
        });

        // Mouse mode handlers: middle-button release is only needed when the
        // terminal application is tracking mouse input.
        if mode.intersects(Modes::MOUSE_MODE) {
            self.interactivity.on_mouse_up(
                MouseButton::Middle,
                TerminalElement::generic_button_handler(
                    terminal,
                    focus,
                    false,
                    move |terminal, e, cx| {
                        terminal.mouse_up(e, cx);
                    },
                ),
            );
        }
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = LayoutState;

    fn id(&self) -> Option<ElementId> {
        self.interactivity.element_id.clone()
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let height: Length = relative(1.).into();

        let layout_id = self.interactivity.request_layout(
            global_id,
            inspector_id,
            window,
            cx,
            |mut style, window, cx| {
                style.size.width = relative(1.).into();
                style.size.height = height;

                window.request_layout(style, None, cx)
            },
        );
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.interactivity.prepaint(
            global_id,
            inspector_id,
            bounds,
            bounds.size,
            window,
            cx,
            |_, _, hitbox, window, cx| {
                let hitbox = hitbox.unwrap();
                let show_timestamps = self.terminal_view.read(cx).show_timestamps;
                let settings = ThemeSettings::get_global(cx).clone();

                let buffer_font_size = settings.buffer_font_size(cx);

                let terminal_settings = TerminalSettings::get_global(cx);
                let minimum_contrast = terminal_settings.minimum_contrast;

                let font_family = terminal_settings.font_family.as_ref().map_or_else(
                    || settings.buffer_font.family.clone(),
                    |font_family| font_family.0.clone().into(),
                );

                let font_fallbacks = terminal_settings
                    .font_fallbacks
                    .as_ref()
                    .or(settings.buffer_font.fallbacks.as_ref())
                    .cloned();

                let font_features = terminal_settings
                    .font_features
                    .as_ref()
                    .unwrap_or(&FontFeatures::disable_ligatures())
                    .clone();

                let font_weight = terminal_settings.font_weight.unwrap_or_default();

                let line_height = terminal_settings.line_height.value();

                let font_size = terminal_settings
                    .font_size
                    .map_or(buffer_font_size, |size| {
                        theme_settings::adjusted_font_size(size, cx)
                    });

                let theme = cx.theme().clone();

                let link_style = HighlightStyle {
                    color: Some(theme.colors().link_text_hover),
                    font_weight: Some(font_weight),
                    underline: Some(UnderlineStyle {
                        color: Some(theme.colors().link_text_hover),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..Default::default()
                };

                let text_style = TextStyle {
                    font_family,
                    font_features,
                    font_weight,
                    font_fallbacks,
                    font_size: font_size.into(),
                    font_style: FontStyle::Normal,
                    line_height: px(line_height).into(),
                    background_color: Some(theme.colors().terminal_ansi_background),
                    white_space: WhiteSpace::Normal,
                    // These are going to be overridden per-cell
                    color: theme.colors().terminal_foreground,
                    ..Default::default()
                };

                let text_system = cx.text_system();
                let (dimensions, line_height_px) = {
                    let rem_size = window.rem_size();
                    let font_pixels = text_style.font_size.to_pixels(rem_size);
                    let line_height = f32::from(font_pixels) * line_height;
                    let font_id = cx.text_system().resolve_font(&text_style.font());

                    let cell_width = text_system
                        .advance(font_id, font_pixels, 'm')
                        .unwrap()
                        .width;

                    let gutter = if show_timestamps {
                        let available =
                            (f32::from(bounds.size.width) - TIMESTAMP_GUTTER_GAP).max(0.0);
                        px((TIMESTAMP_GUTTER_WIDTH + TIMESTAMP_GUTTER_GAP).min(available))
                    } else {
                        px(0.)
                    };

                    let mut size = bounds.size;
                    size.width -= gutter;
                    let available_height = size.height;

                    // https://github.com/zed-industries/zed/issues/2750
                    // if the terminal is one column wide, rendering 🦀
                    // causes alacritty to misbehave.
                    if size.width < cell_width * 2.0 {
                        size.width = cell_width * 2.0;
                    }

                    let mut origin = bounds.origin;
                    origin.x += gutter;

                    let should_anchor_to_bottom = {
                        let content = self.terminal.read(cx).last_content();
                        content.mode.contains(Modes::ALT_SCREEN)
                            || (content.scrolled_to_bottom && content.bottom_row_occupied)
                    };
                    let scale_factor = window.scale_factor();
                    let line_height_pixels = px(line_height);
                    let line_height_device_px = (f32::from(line_height_pixels) * scale_factor)
                        .round()
                        .max(1.0) as i32;
                    let available_height_device_px = (f32::from(available_height) * scale_factor)
                        .floor()
                        .max(0.0) as i32;
                    let rows =
                        ((available_height_device_px / line_height_device_px) as usize).max(1);
                    let snapped_height_device_px = (rows as i32) * line_height_device_px;
                    let padding_device_px =
                        (available_height_device_px - snapped_height_device_px).max(0);
                    let snapped_height =
                        px(snapped_height_device_px as f32 / scale_factor.max(1.0));
                    let padding = px(padding_device_px as f32 / scale_factor.max(1.0));
                    size.height = snapped_height;
                    if should_anchor_to_bottom {
                        origin.y += padding;
                    }

                    // Snap to device pixels to avoid subpixel jitter while resizing.
                    // Terminal rendering is grid-based; allowing fractional origins can cause the
                    // glyph rasterization to shift between frames, which looks like flicker.
                    let scale_factor = window.scale_factor();
                    let snap_px = |value: Pixels| {
                        Pixels::from((f32::from(value) * scale_factor).floor() / scale_factor)
                    };
                    origin.x = snap_px(origin.x);
                    origin.y = snap_px(origin.y);

                    (
                        TerminalBounds::new(px(line_height), cell_width, Bounds { origin, size }),
                        line_height,
                    )
                };

                let background_color = theme.colors().terminal_background;

                let last_hovered_word = self.terminal.update(cx, |terminal, cx| {
                    terminal.set_size(dimensions);
                    terminal.sync(window, cx);

                    (window.modifiers().secondary()
                        && dimensions.bounds.contains(&window.mouse_position()))
                    .then(|| terminal.last_content.last_hovered_word.clone())
                    .flatten()
                });
                let hovered_link = last_hovered_word.is_some();

                let (mode, display_offset, cursor_char, selection, cursor, row_snapshots) = {
                    let content = &self.terminal.read(cx).last_content;
                    (
                        content.mode,
                        content.display_offset,
                        content.cursor_char,
                        content.selection,
                        content.cursor,
                        timestamp_rows(
                            &content.cells,
                            dimensions.num_lines(),
                            dimensions.num_columns(),
                        ),
                    )
                };
                let (show_timestamps, timestamp_rows) = self.terminal_view.update(cx, |view, _| {
                    view.update_timestamp_state(
                        &row_snapshots,
                        display_offset,
                        DisplayCursor::from(cursor.point, display_offset)
                            .line()
                            .try_into()
                            .ok(),
                        mode.contains(Modes::ALT_SCREEN),
                    )
                });
                let cells = &self.terminal.read(cx).last_content.cells;

                // Keep selection painting in the element so the terminal core
                // remains the single owner of selection coordinates.
                let mut relative_highlighted_ranges = Vec::new();
                if let Some(selection) = selection {
                    relative_highlighted_ranges
                        .push((selection.point_range(), selection_highlight_color()));
                }

                // Calculate the intersection of the terminal's bounds with the current
                // content mask (the visible viewport after all parent clipping).
                // This allows us to only render cells that are actually visible, which is
                // critical for performance when terminals are inside scrollable containers
                // like the Agent Panel thread view.
                //
                // This optimization is analogous to the editor optimization in PR #45077
                // which fixed performance issues with large AutoHeight editors inside Lists.
                let content_bounds = dimensions.bounds;
                let visible_bounds = window.content_mask().bounds;
                let intersection = visible_bounds.intersect(&content_bounds);

                // If the terminal is entirely outside the viewport, skip all cell processing.
                // This handles the case where the terminal has been scrolled past (above or
                // below the viewport), similar to the editor fix in PR #45077 where start_row
                // could exceed max_row when the editor was positioned above the viewport.
                let (rects, batched_text_runs, block_element_rects) = if intersection.size.height
                    <= px(0.)
                    || intersection.size.width <= px(0.)
                {
                    (Vec::new(), Vec::new(), Vec::new())
                } else if intersection == content_bounds {
                    // Fast path: terminal fully visible, no clipping needed.
                    // Avoid grouping/allocation overhead by streaming cells directly.
                    TerminalElement::layout_grid(
                        cells.iter(),
                        0,
                        &text_style,
                        last_hovered_word
                            .as_ref()
                            .map(|word| (link_style, &word.word_match)),
                        minimum_contrast,
                        dimensions.cell_width,
                        window.rem_size(),
                        cx,
                    )
                } else {
                    // Calculate which screen rows are visible based on pixel positions.
                    // This works for both Scrollable and Inline modes because we filter
                    // by screen position (enumerated line group index), not by the cell's
                    // internal line number (which can be negative in Scrollable mode for
                    // scrollback history).
                    let rows_above_viewport = f32::from(
                        (intersection.top() - content_bounds.top()).max(px(0.)) / line_height_px,
                    ) as usize;
                    let visible_row_count =
                        f32::from((intersection.size.height / line_height_px).ceil()) as usize + 1;

                    TerminalElement::layout_grid(
                        // Group cells by line and filter to only the visible screen rows.
                        // skip() and take() work on enumerated line groups (screen position),
                        // making this work regardless of the actual cell.point.line values.
                        cells
                            .iter()
                            .chunk_by(|c| c.point.line)
                            .into_iter()
                            .skip(rows_above_viewport)
                            .take(visible_row_count)
                            .flat_map(|(_, line_cells)| line_cells),
                        rows_above_viewport as i32,
                        &text_style,
                        last_hovered_word
                            .as_ref()
                            .map(|word| (link_style, &word.word_match)),
                        minimum_contrast,
                        dimensions.cell_width,
                        window.rem_size(),
                        cx,
                    )
                };

                // Layout cursor. Rectangle is used for IME, so we should lay it out even
                // if we don't end up showing it.
                let cursor_point = DisplayCursor::from(cursor.point, display_offset);
                let cursor_text = {
                    let str_trxt = cursor_char.to_string();
                    let len = str_trxt.len();
                    window.text_system().shape_line(
                        str_trxt.into(),
                        text_style.font_size.to_pixels(window.rem_size()),
                        &[TextRun {
                            len,
                            font: text_style.font(),
                            color: theme.colors().terminal_ansi_background,
                            ..Default::default()
                        }],
                        None,
                    )
                };

                // For whitespace, use cell width to avoid cursor stretching.
                // For other characters, use the larger of shaped width and cell width
                // to properly cover wide characters like emojis.
                let cursor_width = if cursor_char.is_whitespace() {
                    dimensions.cell_width()
                } else {
                    cursor_text.width.max(dimensions.cell_width())
                };

                let ime_cursor_bounds = TerminalElement::cursor_position(cursor_point, dimensions)
                    .map(|cursor_position| Bounds {
                        origin: cursor_position,
                        size: size(cursor_width.ceil(), dimensions.line_height),
                    });

                let cursor = if let CursorShape::Hidden = cursor.shape {
                    None
                } else {
                    let focused = self.focused;
                    ime_cursor_bounds.map(move |bounds| {
                        let (shape, text) = match cursor.shape {
                            CursorShape::Block if !focused => (EditorCursorShape::Hollow, None),
                            CursorShape::Block => (EditorCursorShape::Block, Some(cursor_text)),
                            CursorShape::Underline if !focused => (EditorCursorShape::Hollow, None),
                            CursorShape::Underline => (EditorCursorShape::Underline, None),
                            CursorShape::Bar if !focused => (EditorCursorShape::Hollow, None),
                            CursorShape::Bar => (EditorCursorShape::Bar, None),
                            CursorShape::HollowBlock => (EditorCursorShape::Hollow, None),
                            CursorShape::Hidden => unreachable!(),
                        };

                        CursorLayout::new(
                            bounds.origin,
                            bounds.size.width,
                            bounds.size.height,
                            theme.players().local().cursor,
                            shape,
                            text,
                        )
                    })
                };

                LayoutState {
                    hitbox,
                    batched_text_runs,
                    block_element_rects,
                    cursor,
                    ime_cursor_bounds,
                    background_color,
                    dimensions,
                    rects,
                    relative_highlighted_ranges,
                    mode,
                    display_offset,
                    hovered_link,
                    base_text_style: text_style,
                    timestamp_rows: if show_timestamps {
                        timestamp_rows
                    } else {
                        Vec::new()
                    },
                }
            },
        )
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        layout: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let paint_start = Instant::now();
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.paint_quad(fill(bounds, layout.background_color));
            let origin = layout.dimensions.bounds.origin;
            let scale_factor = window.scale_factor();
            let snap_px = |value: Pixels| {
                Pixels::from((f32::from(value) * scale_factor).floor() / scale_factor)
            };
            let origin = point(snap_px(origin.x), snap_px(origin.y));

            paint_timestamp_gutter(
                &layout.timestamp_rows,
                bounds,
                &layout.dimensions,
                layout.dimensions.line_height,
                &layout.base_text_style,
                window,
                cx,
            );

            let marked_text_cloned = {
                let marked_text = &self.terminal_view.read(cx).ime_marked_text;
                (!marked_text.is_empty()).then(|| marked_text.clone())
            };

            let terminal_input_handler = TerminalInputHandler {
                terminal_view: self.terminal_view.clone(),
                cursor_bounds: layout.ime_cursor_bounds.map(|bounds| bounds + origin),
            };

            self.register_mouse_listeners(layout.mode, &layout.hitbox, window);
            window.set_cursor_style(
                if layout.hovered_link {
                    gpui::CursorStyle::PointingHand
                } else {
                    gpui::CursorStyle::IBeam
                },
                &layout.hitbox,
            );

            let original_cursor = layout.cursor.take();
            self.interactivity.paint(
                global_id,
                inspector_id,
                bounds,
                Some(&layout.hitbox),
                window,
                cx,
                |_, window, cx| {
                    window.handle_input(&self.focus, terminal_input_handler, cx);

                    window.on_key_event({
                        let this = self.terminal.clone();
                        move |event: &ModifiersChangedEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble {
                                return;
                            }

                            this.update(cx, |term, cx| {
                                term.try_modifiers_change(&event.modifiers, window, cx)
                            });
                        }
                    });

                    for rect in &layout.rects {
                        rect.paint(origin, &layout.dimensions, window);
                    }

                    for (relative_highlighted_range, color) in &layout.relative_highlighted_ranges {
                        if let Some((start_y, highlighted_range_lines)) =
                            to_highlighted_range_lines(relative_highlighted_range, layout, origin)
                        {
                            let hr = HighlightedRange {
                                start_y,
                                line_height: layout.dimensions.line_height,
                                lines: highlighted_range_lines,
                                color: *color,
                            };
                            hr.paint(window);
                        }
                    }

                    // Paint batched text runs instead of individual cells
                    let text_paint_start = Instant::now();
                    for batch in &layout.batched_text_runs {
                        batch.paint(origin, &layout.dimensions, window, cx);
                    }
                    for block_element_rect in &layout.block_element_rects {
                        block_element_rect.paint(origin, &layout.dimensions, window);
                    }
                    let text_paint_time = text_paint_start.elapsed();

                    if let Some(text_to_mark) = &marked_text_cloned
                        && !text_to_mark.is_empty()
                        && let Some(ime_bounds) = layout.ime_cursor_bounds
                    {
                        let ime_position = (ime_bounds + origin).origin;
                        let mut ime_style = layout.base_text_style.clone();
                        ime_style.underline = Some(UnderlineStyle {
                            color: Some(ime_style.color),
                            thickness: px(1.0),
                            wavy: false,
                        });

                        let shaped_line = window.text_system().shape_line(
                            text_to_mark.clone().into(),
                            ime_style.font_size.to_pixels(window.rem_size()),
                            &[TextRun {
                                len: text_to_mark.len(),
                                font: ime_style.font(),
                                color: ime_style.color,
                                underline: ime_style.underline,
                                ..Default::default()
                            }],
                            None,
                        );

                        // Paint background to cover terminal text behind marked text
                        let ime_background_bounds = Bounds::new(
                            ime_position,
                            size(shaped_line.width, layout.dimensions.line_height),
                        );
                        window.paint_quad(fill(ime_background_bounds, layout.background_color));

                        shaped_line
                            .paint(
                                ime_position,
                                layout.dimensions.line_height,
                                gpui::TextAlign::Left,
                                None,
                                window,
                                cx,
                            )
                            .log_err();
                    }

                    if self.cursor_visible
                        && marked_text_cloned.is_none()
                        && let Some(mut cursor) = original_cursor
                    {
                        cursor.paint(origin, window, cx);
                    }

                    log::debug!(
                        "Terminal paint: {} text runs, {} rects, \
                        text paint took {:?}, total paint took {total_paint_time:?}",
                        layout.batched_text_runs.len(),
                        layout.rects.len(),
                        text_paint_time,
                        total_paint_time = paint_start.elapsed()
                    );
                },
            );
        });
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

struct TerminalInputHandler {
    terminal_view: Entity<TerminalView>,
    cursor_bounds: Option<Bounds<Pixels>>,
}

impl InputHandler for TerminalInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _cx: &mut App,
    ) -> Option<UTF16Selection> {
        // Always return a valid selection for IME positioning,
        // even in ALT_SCREEN mode (fullscreen TUI apps like opencode, vim, etc.)
        // The terminal still has a cursor position that should be used for IME candidate window placement.
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &mut self,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<std::ops::Range<usize>> {
        let marked_text = &self.terminal_view.read(cx).ime_marked_text;
        (!marked_text.is_empty()).then(|| 0..marked_text.encode_utf16().count())
    }

    fn text_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: &mut Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<std::ops::Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.terminal_view.update(cx, |view, view_cx| {
            view.ime_marked_text.clear();
            if !text.is_empty() {
                view.zed_terminal.update(view_cx, |terminal, _| {
                    terminal.input(text.as_bytes().to_vec())
                });
            }
            view_cx.notify();
        });

        window.invalidate_character_coordinates();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<std::ops::Range<usize>>,
        new_text: &str,
        _new_marked_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut App,
    ) {
        self.terminal_view.update(cx, |view, view_cx| {
            view.ime_marked_text.clear();
            view.ime_marked_text.push_str(new_text);
            view_cx.notify();
        });
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut App) {
        self.terminal_view.update(cx, |view, view_cx| {
            view.ime_marked_text.clear();
            view_cx.notify();
        });
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: std::ops::Range<usize>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let term_bounds = self
            .terminal_view
            .read(cx)
            .zed_terminal
            .read(cx)
            .last_content()
            .terminal_bounds;

        let mut bounds = self.cursor_bounds?;
        let offset_x = term_bounds.cell_width * range_utf16.start as f32;
        bounds.origin.x += offset_x;

        Some(bounds)
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }

    fn character_index_for_point(
        &mut self,
        _point: GpuiPoint<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        None
    }
}

pub fn is_blank(cell: &Cell) -> bool {
    if cell.character() != ' ' {
        return false;
    }

    if !is_default_background_color(cell.background()) {
        return false;
    }

    if cell.hyperlink().is_some() {
        return false;
    }

    if cell.has_visible_style_modifier() {
        return false;
    }

    true
}

fn to_highlighted_range_lines(
    range: &Range,
    layout: &LayoutState,
    origin: GpuiPoint<Pixels>,
) -> Option<(Pixels, Vec<HighlightedRangeLine>)> {
    // Step 1. Normalize the points to be viewport relative.
    // When display_offset = 1, here's how the grid is arranged:
    //-2,0 -2,1...
    //--- Viewport top
    //-1,0 -1,1...
    //--------- Terminal Top
    // 0,0  0,1...
    // 1,0  1,1...
    //--- Viewport Bottom
    // 2,0  2,1...
    //--------- Terminal Bottom

    // Normalize to viewport relative, from terminal relative.
    // lines are i32s, which are negative above the top left corner of the terminal
    // If the user has scrolled, we use the display_offset to tell us which offset
    // of the grid data we should be looking at. But for the rendering step, we don't
    // want negatives. We want things relative to the 'viewport' (the area of the grid
    // which is currently shown according to the display offset)
    let display_offset = i32::try_from(layout.display_offset).unwrap_or(i32::MAX);
    let unclamped_start_line = range.start().line.saturating_add(display_offset);
    let unclamped_start_column = range.start().column;
    let unclamped_end_line = range.end().line.saturating_add(display_offset);
    let unclamped_end_column = range.end().column;

    // Step 2. Clamp range to viewport, and return None if it doesn't overlap
    if unclamped_end_line < 0 || unclamped_start_line > layout.dimensions.num_lines() as i32 {
        return None;
    }

    let clamped_start_line = unclamped_start_line.max(0) as usize;

    let clamped_end_line = unclamped_end_line.min(layout.dimensions.num_lines() as i32) as usize;

    // Convert the start of the range to pixels
    let start_y = origin.y + clamped_start_line as f32 * layout.dimensions.line_height;

    // Step 3. Expand ranges that cross lines into a collection of single-line ranges.
    //  (also convert to pixels)
    let mut highlighted_range_lines = Vec::new();
    for line in clamped_start_line..=clamped_end_line {
        let mut line_start = 0;
        let mut line_end = layout.dimensions.num_columns();

        if line == clamped_start_line && unclamped_start_line >= 0 {
            line_start = unclamped_start_column;
        }
        if line == clamped_end_line && unclamped_end_line <= layout.dimensions.num_lines() as i32 {
            line_end = unclamped_end_column + 1; // +1 for inclusive
        }

        highlighted_range_lines.push(HighlightedRangeLine {
            start_x: origin.x + line_start as f32 * layout.dimensions.cell_width,
            end_x: origin.x + line_end as f32 * layout.dimensions.cell_width,
        });
    }

    Some((start_y, highlighted_range_lines))
}

/// Converts a 2, 8, or 24 bit color ANSI color to the GPUI equivalent.
pub fn convert_color(fg: &Color, theme: &Theme) -> Hsla {
    let colors = theme.colors();
    match fg {
        // Named and theme defined colors
        Color::Named(color) => match color {
            NamedColor::Black => colors.terminal_ansi_black,
            NamedColor::Red => colors.terminal_ansi_red,
            NamedColor::Green => colors.terminal_ansi_green,
            NamedColor::Yellow => colors.terminal_ansi_yellow,
            NamedColor::Blue => colors.terminal_ansi_blue,
            NamedColor::Magenta => colors.terminal_ansi_magenta,
            NamedColor::Cyan => colors.terminal_ansi_cyan,
            NamedColor::White => colors.terminal_ansi_white,
            NamedColor::BrightBlack => colors.terminal_ansi_bright_black,
            NamedColor::BrightRed => colors.terminal_ansi_bright_red,
            NamedColor::BrightGreen => colors.terminal_ansi_bright_green,
            NamedColor::BrightYellow => colors.terminal_ansi_bright_yellow,
            NamedColor::BrightBlue => colors.terminal_ansi_bright_blue,
            NamedColor::BrightMagenta => colors.terminal_ansi_bright_magenta,
            NamedColor::BrightCyan => colors.terminal_ansi_bright_cyan,
            NamedColor::BrightWhite => colors.terminal_ansi_bright_white,
            NamedColor::Foreground => colors.terminal_foreground,
            NamedColor::Background => colors.terminal_ansi_background,
            NamedColor::Cursor => theme.players().local().cursor,
            NamedColor::DimBlack => colors.terminal_ansi_dim_black,
            NamedColor::DimRed => colors.terminal_ansi_dim_red,
            NamedColor::DimGreen => colors.terminal_ansi_dim_green,
            NamedColor::DimYellow => colors.terminal_ansi_dim_yellow,
            NamedColor::DimBlue => colors.terminal_ansi_dim_blue,
            NamedColor::DimMagenta => colors.terminal_ansi_dim_magenta,
            NamedColor::DimCyan => colors.terminal_ansi_dim_cyan,
            NamedColor::DimWhite => colors.terminal_ansi_dim_white,
            NamedColor::BrightForeground => colors.terminal_bright_foreground,
            NamedColor::DimForeground => colors.terminal_dim_foreground,
        },
        // 'True' colors
        Color::Spec(rgb) => terminal::rgba_color(rgb.r, rgb.g, rgb.b),
        // 8 bit, indexed colors
        Color::Indexed(i) => terminal::get_color_at_index(*i as usize, theme),
    }
}

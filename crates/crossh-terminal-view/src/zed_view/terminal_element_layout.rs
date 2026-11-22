//! Layout and cell-style computation for the terminal element.
//! Positional split from `terminal_element.rs` (Zed revision 90d024b88abc91264d9a0ad260eb4f365fa695c3);
//! see that file's header for the split map.
// SPDX-License-Identifier: GPL-3.0-or-later

use gpui::{
    App, Font, FontStyle, FontWeight, HighlightStyle, Hsla, Pixels, StrikethroughStyle, TextRun,
    TextStyle, UnderlineStyle, px,
};
use itertools::Itertools;
use std::mem;
use std::time::Instant;
use terminal::theme::CrosshTheme;
use terminal::{
    Cell, Color, NamedColor, Point, Range,
    is_app_chosen_exact_color as terminal_is_app_chosen_exact_color, is_default_background_color,
};
use unicode_width::UnicodeWidthChar;

use super::apca_contrast::ensure_minimum_contrast;
use super::{
    BLOCK_SUBCELL_COLUMNS, BLOCK_SUBCELL_LINES, BackgroundRegion, BatchedTextRun,
    BlockElementLayoutRect, LayoutPoint, LayoutRect, TerminalElement, TerminalLayoutCell,
    merge_background_regions,
};

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

/// 第一遍收集所需的只读布局上下文，避免在辅助函数间重复传参。
struct GridLayoutContext<'a> {
    text_style: &'a TextStyle,
    hyperlink: Option<(HighlightStyle, &'a Range)>,
    minimum_contrast: f32,
    cell_width: Pixels,
    base_font_pixels: Pixels,
    theme: &'a CrosshTheme,
    cx: &'a App,
}

/// 第一遍收集的可变累积状态。
struct GridRunCollector {
    batched_runs: Vec<BatchedTextRun>,
    block_element_regions: Vec<BackgroundRegion>,
    current_batch: Option<BatchedTextRun>,
    cell_count: usize,
    shaped_cache: std::collections::HashMap<(char, gpui::FontId), Pixels>,
}

impl GridRunCollector {
    fn flush_current_batch(&mut self) {
        if let Some(batch) = self.current_batch.take() {
            self.batched_runs.push(batch);
        }
    }

    /// 歧义宽度字符是否应按 2 格处理（字形步进超过格宽）。
    fn is_overwide_cell(&mut self, ch: char, style: &TextRun, ctx: &GridLayoutContext) -> bool {
        if ch.is_ascii() || ch == ' ' || ch.width().unwrap_or(1) != 1 {
            return false;
        }
        let font_id = ctx.cx.text_system().resolve_font(&style.font);
        let shaped = *self.shaped_cache.entry((ch, font_id)).or_insert_with(|| {
            ctx.cx
                .text_system()
                .layout_width(font_id, ctx.base_font_pixels, ch)
        });
        ambiguous_shrink_factor(shaped, ctx.cell_width).is_some()
    }

    /// 追加一个非空白单元格的文本；必要时新建 batch 并更新 overwide 偏移。
    fn append_text_cell(
        &mut self,
        point: Point,
        cell: &Cell,
        display_line: i32,
        style: TextRun,
        extra_offset: &mut i32,
        ctx: &GridLayoutContext,
    ) {
        let cell_point = LayoutPoint::new(display_line, point.column as i32 + *extra_offset);
        if TerminalElement::collect_block_element_regions(
            cell_point,
            cell.character(),
            style.color,
            &mut self.block_element_regions,
        ) {
            self.flush_current_batch();
            return;
        }

        let ch = cell.character();
        let is_overwide = self.is_overwide_cell(ch, &style, ctx);
        let zero_width_chars = cell.zerowidth();
        let cell_font_size = ctx.text_style.font_size;

        // Overwide chars never batch with neighbours and occupy 2 cells
        let can_append = if is_overwide {
            false
        } else if let Some(batch) = &self.current_batch {
            batch.cell_count == 1
                && batch.can_append(&style, cell_font_size)
                && batch.start_point.line == cell_point.line
                && batch.start_point.column + batch.cell_count as i32 == cell_point.column
        } else {
            false
        };

        if can_append {
            let batch = self.current_batch.as_mut().unwrap();
            batch.append_char(ch);
            if let Some(chars) = zero_width_chars {
                batch.append_zero_width_chars(chars);
            }
        } else {
            self.flush_current_batch();
            let mut new_batch =
                BatchedTextRun::new_from_char(cell_point, ch, style, cell_font_size);
            if is_overwide {
                new_batch.cell_count = 2;
            }
            if let Some(chars) = zero_width_chars {
                new_batch.append_zero_width_chars(chars);
            }
            self.current_batch = Some(new_batch);
        }

        if is_overwide {
            *extra_offset += 1;
        }
    }
}

/// 追加/扩展一个背景区域：仅同 line、同色且相邻时才扩展。
fn push_background_region(
    regions: &mut Vec<BackgroundRegion>,
    display_line: i32,
    col: i32,
    color: Hsla,
) {
    if let Some(last_region) = regions.last_mut()
        && last_region.color == color
        && last_region.start_line == display_line
        && last_region.end_line == display_line
        && last_region.end_col + 1 == col
    {
        last_region.end_col = col;
        return;
    }

    regions.push(BackgroundRegion::new(display_line, col, color));
}

/// 第一遍：按行收集文本 batch、背景区域与 block element 区域。
fn collect_grid_runs<T: TerminalLayoutCell>(
    grid: impl Iterator<Item = T>,
    start_line_offset: i32,
    ctx: &GridLayoutContext,
) -> (
    Vec<BatchedTextRun>,
    Vec<BackgroundRegion>,
    Vec<BackgroundRegion>,
    usize,
) {
    // Pre-allocate with estimated capacity to reduce reallocations
    let estimated_cells = grid.size_hint().0;
    let estimated_runs = estimated_cells / 10; // Estimate ~10 cells per run
    let estimated_regions = estimated_cells / 20; // Estimate ~20 cells per background region

    let mut collector = GridRunCollector {
        batched_runs: Vec::with_capacity(estimated_runs),
        block_element_regions: Vec::new(),
        current_batch: None,
        cell_count: 0,
        shaped_cache: std::collections::HashMap::new(),
    };
    // Collect background regions for efficient merging
    let mut background_regions: Vec<BackgroundRegion> = Vec::with_capacity(estimated_regions);

    let linegroups = grid.into_iter().chunk_by(|cell| cell.point().line);
    for (line_index, (_, line)) in linegroups.into_iter().enumerate() {
        let display_line = start_line_offset + line_index as i32;

        // Flush any existing batch at line boundaries
        collector.flush_current_batch();

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
                push_background_region(
                    &mut background_regions,
                    display_line,
                    point.column as i32,
                    convert_color(&bg, ctx.theme),
                );
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
            previous_cell_had_extras = matches!(cell.zerowidth(), Some(chars) if !chars.is_empty());

            if is_blank(cell) {
                continue;
            }

            collector.cell_count += 1;
            let mut style = TerminalElement::cell_style(
                cell,
                fg,
                bg,
                ctx.theme,
                ctx.text_style,
                ctx.minimum_contrast,
            );
            apply_hovered_link_style(point, ctx.hyperlink, &mut style);
            collector.append_text_cell(point, cell, display_line, style, &mut extra_offset, ctx);
        }
    }

    // Flush any remaining batch
    collector.flush_current_batch();

    (
        collector.batched_runs,
        background_regions,
        collector.block_element_regions,
        collector.cell_count,
    )
}

/// 第二遍：合并背景区域，并按行拆分为单行 `LayoutRect`。
fn background_regions_to_rects(regions: Vec<BackgroundRegion>) -> (Vec<LayoutRect>, usize) {
    let region_count = regions.len();
    let merged_regions = merge_background_regions(regions);
    let mut rects = Vec::with_capacity(merged_regions.len() * 2); // Estimate 2 rects per merged region

    // LayoutRect only supports single-line rectangles, so split multi-line regions
    for region in merged_regions {
        for line in region.start_line..=region.end_line {
            rects.push(LayoutRect::new(
                LayoutPoint::new(line, region.start_col),
                (region.end_col - region.start_col + 1) as usize,
                region.color,
            ));
        }
    }

    (rects, region_count)
}

fn log_grid_layout(
    cell_count: usize,
    batched_run_count: usize,
    block_element_rect_count: usize,
    block_element_region_count: usize,
    rect_count: usize,
    region_count: usize,
    layout_time: std::time::Duration,
) {
    log::debug!(
        "Terminal layout_grid: {} cells processed, \
        {} batched runs created, {} block element rects (from {} regions), {} rects (from {} merged regions), \
        layout took {:?}",
        cell_count,
        batched_run_count,
        block_element_rect_count,
        block_element_region_count,
        rect_count,
        region_count,
        layout_time
    );
}

impl TerminalElement {
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
        let theme = CrosshTheme::get(cx);
        let ctx = GridLayoutContext {
            text_style,
            hyperlink,
            minimum_contrast,
            cell_width,
            base_font_pixels: text_style.font_size.to_pixels(rem_size),
            theme,
            cx,
        };

        let (batched_runs, background_regions, block_element_regions, cell_count) =
            collect_grid_runs(grid, start_line_offset, &ctx);
        let (rects, region_count) = background_regions_to_rects(background_regions);

        let block_element_region_count = block_element_regions.len();
        let block_element_rects = Self::block_element_regions_to_rects(block_element_regions);
        let layout_time = start_time.elapsed();

        log_grid_layout(
            cell_count,
            batched_runs.len(),
            block_element_rects.len(),
            block_element_region_count,
            rects.len(),
            region_count,
            layout_time,
        );

        (rects, batched_runs, block_element_rects)
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
        colors: &CrosshTheme,
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

/// Converts a 2, 8, or 24 bit color ANSI color to the GPUI equivalent.
pub fn convert_color(fg: &Color, theme: &CrosshTheme) -> Hsla {
    let colors = theme;
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
            NamedColor::Cursor => theme.cursor,
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

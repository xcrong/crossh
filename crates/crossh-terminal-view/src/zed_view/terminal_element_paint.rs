//! Painting for the terminal element.
//! Positional split from `terminal_element.rs` (Zed revision 90d024b88abc91264d9a0ad260eb4f365fa695c3);
//! see that file's header for the split map.
// SPDX-License-Identifier: GPL-3.0-or-later

use crossh_terminal::timestamps::TerminalRow;
use gpui::{
    App, Bounds, ContentMask, Corners, DispatchPhase, Edges, Element, ElementId, Entity,
    FontFeatures, FontStyle, FontWeight, GlobalElementId, HighlightStyle, Hsla, LayoutId, Length,
    ModifiersChangedEvent, Pixels, Point as GpuiPoint, TextRun, TextStyle, UnderlineStyle,
    WhiteSpace, Window, fill, point, px, quad, relative, size,
};
use itertools::Itertools;
use std::time::{Duration, Instant};
use terminal::result_ext::ResultExt;
use terminal::theme::CrosshTheme;
use terminal::{Cursor, CursorShape, IndexedCell, Modes, Range, Terminal, TerminalBounds};

use super::terminal_element_input::TerminalInputHandler;
use super::{
    BatchedTextRun, BlockElementLayoutRect, CursorLayout, DisplayCursor, EditorCursorShape,
    LayoutRect, LayoutState, TerminalElement, TerminalView,
};

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

impl TerminalElement {
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
}

/// Font settings and theme colors resolved once per `prepaint`.
struct TerminalPaintStyle {
    text_style: TextStyle,
    link_style: HighlightStyle,
    theme: CrosshTheme,
    minimum_contrast: f32,
    line_height: f32,
    show_timestamps: bool,
}

fn terminal_paint_style(terminal_view: &Entity<TerminalView>, cx: &App) -> TerminalPaintStyle {
    let show_timestamps = terminal_view.read(cx).show_timestamps;
    let terminal_settings = terminal::terminal_settings::TerminalSettings::get_global(cx).clone();
    let minimum_contrast = terminal_settings.minimum_contrast;
    // Crossh single source of truth — copied from Zed's buffer_font chain but now owned.
    // Do not read ThemeSettings/SettingsStore; fallback is Crossh's own mono.
    let font_family = terminal_settings
        .font_family
        .as_ref()
        .map_or_else(|| "Lilex".into(), |f| f.0.clone().into());
    let font_fallbacks = terminal_settings.font_fallbacks.clone();
    let font_features = terminal_settings
        .font_features
        .clone()
        .unwrap_or_else(FontFeatures::disable_ligatures);
    let font_weight = terminal_settings.font_weight.unwrap_or(FontWeight(400.0));
    let line_height = terminal_settings.line_height.value();
    let font_size = terminal_settings
        .font_size
        .map(|s| s.into())
        .unwrap_or_else(|| px(14.0).into());
    let theme = CrosshTheme::get(cx).clone();

    let link_style = HighlightStyle {
        color: Some(theme.link_text_hover),
        font_weight: Some(font_weight),
        underline: Some(UnderlineStyle {
            color: Some(theme.link_text_hover),
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
        font_size,
        font_style: FontStyle::Normal,
        line_height: px(line_height).into(),
        white_space: WhiteSpace::Normal,
        // These are going to be overridden per-cell
        color: theme.terminal_foreground,
        ..Default::default()
    };

    TerminalPaintStyle {
        text_style,
        link_style,
        theme,
        minimum_contrast,
        line_height,
        show_timestamps,
    }
}

/// Computes the snapped terminal bounds and the line height used for row math.
fn terminal_dimensions(
    bounds: Bounds<Pixels>,
    style: &TerminalPaintStyle,
    terminal: &Entity<Terminal>,
    window: &Window,
    cx: &App,
) -> (TerminalBounds, f32) {
    let text_style = &style.text_style;
    let line_height = style.line_height;
    let show_timestamps = style.show_timestamps;
    let text_system = cx.text_system();
    let rem_size = window.rem_size();
    let font_pixels = text_style.font_size.to_pixels(rem_size);
    let line_height = f32::from(font_pixels) * line_height;
    let font_id = text_system.resolve_font(&text_style.font());

    let cell_width = text_system
        .advance(font_id, font_pixels, 'm')
        .map(|advance| advance.width)
        .unwrap_or_else(|_| {
            log::warn!("failed to measure terminal cell width; using font size as fallback");
            font_pixels
        });

    let gutter = if show_timestamps {
        let available = (f32::from(bounds.size.width) - TIMESTAMP_GUTTER_GAP).max(0.0);
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
        let content = terminal.read(cx).last_content();
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
    let rows = ((available_height_device_px / line_height_device_px) as usize).max(1);
    let snapped_height_device_px = (rows as i32) * line_height_device_px;
    let padding_device_px = (available_height_device_px - snapped_height_device_px).max(0);
    let snapped_height = px(snapped_height_device_px as f32 / scale_factor.max(1.0));
    let padding = px(padding_device_px as f32 / scale_factor.max(1.0));
    size.height = snapped_height;
    if should_anchor_to_bottom {
        origin.y += padding;
    }

    // Snap to device pixels to avoid subpixel jitter while resizing.
    // Terminal rendering is grid-based; allowing fractional origins can cause the
    // glyph rasterization to shift between frames, which looks like flicker.
    let scale_factor = window.scale_factor();
    let snap_px =
        |value: Pixels| Pixels::from((f32::from(value) * scale_factor).floor() / scale_factor);
    origin.x = snap_px(origin.x);
    origin.y = snap_px(origin.y);

    (
        TerminalBounds::new(px(line_height), cell_width, Bounds { origin, size }),
        line_height,
    )
}

/// Metrics `layout_terminal_cells` needs beyond the cells themselves.
struct CellLayoutMetrics {
    minimum_contrast: f32,
    cell_width: Pixels,
    line_height_px: f32,
    rem_size: Pixels,
}

/// Computes the region of the terminal grid that survives parent clipping.
fn terminal_intersection(content_bounds: Bounds<Pixels>, window: &Window) -> Bounds<Pixels> {
    // Calculate the intersection of the terminal's bounds with the current
    // content mask (the visible viewport after all parent clipping).
    // This allows us to only render cells that are actually visible, which is
    // critical for performance when terminals are inside scrollable containers
    // like the Agent Panel thread view.
    //
    // This optimization is analogous to the editor optimization in PR #45077
    // which fixed performance issues with large AutoHeight editors inside Lists.
    let visible_bounds = window.content_mask().bounds;
    visible_bounds.intersect(&content_bounds)
}

/// Selection highlight ranges relative to the terminal content.
fn selection_highlight_ranges(selection: Option<terminal::SelectionRange>) -> Vec<(Range, Hsla)> {
    // Keep selection painting in the element so the terminal core
    // remains the single owner of selection coordinates.
    let mut ranges = Vec::new();
    if let Some(selection) = selection {
        ranges.push((selection.point_range(), selection_highlight_color()));
    }
    ranges
}

fn layout_terminal_cells(
    cells: &[IndexedCell],
    content_bounds: Bounds<Pixels>,
    intersection: Bounds<Pixels>,
    text_style: &TextStyle,
    hover: Option<(HighlightStyle, &Range)>,
    metrics: CellLayoutMetrics,
    cx: &App,
) -> (
    Vec<LayoutRect>,
    Vec<BatchedTextRun>,
    Vec<BlockElementLayoutRect>,
) {
    // If the terminal is entirely outside the viewport, skip all cell processing.
    // This handles the case where the terminal has been scrolled past (above or
    // below the viewport), similar to the editor fix in PR #45077 where start_row
    // could exceed max_row when the editor was positioned above the viewport.
    if intersection.size.height <= px(0.) || intersection.size.width <= px(0.) {
        return (Vec::new(), Vec::new(), Vec::new());
    }

    if intersection == content_bounds {
        // Fast path: terminal fully visible, no clipping needed.
        // Avoid grouping/allocation overhead by streaming cells directly.
        TerminalElement::layout_grid(
            cells.iter(),
            0,
            text_style,
            hover,
            metrics.minimum_contrast,
            metrics.cell_width,
            metrics.rem_size,
            cx,
        )
    } else {
        // Calculate which screen rows are visible based on pixel positions.
        // This works for both Scrollable and Inline modes because we filter
        // by screen position (enumerated line group index), not by the cell's
        // internal line number (which can be negative in Scrollable mode for
        // scrollback history).
        let rows_above_viewport = f32::from(
            (intersection.top() - content_bounds.top()).max(px(0.)) / metrics.line_height_px,
        ) as usize;
        let visible_row_count =
            f32::from((intersection.size.height / metrics.line_height_px).ceil()) as usize + 1;

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
            text_style,
            hover,
            metrics.minimum_contrast,
            metrics.cell_width,
            metrics.rem_size,
            cx,
        )
    }
}

/// Everything needed to lay out the cursor besides the base text style.
struct CursorPaintContext<'a> {
    cursor: Cursor,
    cursor_char: char,
    display_offset: usize,
    dimensions: TerminalBounds,
    focused: bool,
    theme: &'a CrosshTheme,
}

fn layout_terminal_cursor(
    context: CursorPaintContext<'_>,
    text_style: &TextStyle,
    window: &Window,
) -> (Option<CursorLayout>, Option<Bounds<Pixels>>) {
    let CursorPaintContext {
        cursor,
        cursor_char,
        display_offset,
        dimensions,
        focused,
        theme,
    } = context;
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
                color: theme.terminal_ansi_background,
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

    let ime_cursor_bounds =
        TerminalElement::cursor_position(cursor_point, dimensions).map(|cursor_position| Bounds {
            origin: cursor_position,
            size: size(cursor_width.ceil(), dimensions.line_height),
        });

    let cursor = if let CursorShape::Hidden = cursor.shape {
        None
    } else {
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
                theme.cursor,
                shape,
                text,
            )
        })
    };

    (cursor, ime_cursor_bounds)
}

/// Syncs the terminal to the new dimensions and reports the viewport to the daemon.
/// Returns the hovered word when the secondary modifier is held over the terminal.
fn sync_terminal_frame(
    terminal: &Entity<Terminal>,
    terminal_view: &Entity<TerminalView>,
    dimensions: TerminalBounds,
    window: &mut Window,
    cx: &mut App,
) -> Option<terminal::HoveredWord> {
    let last_hovered_word = terminal.update(cx, |terminal, cx| {
        terminal.set_size(dimensions);
        terminal.sync(window, cx);

        (window.modifiers().secondary(cx) && dimensions.bounds.contains(&window.mouse_position()))
            .then(|| terminal.last_content.last_hovered_word.clone())
            .flatten()
    });
    // daemon 远端：视口变化且持有输入权时上报服务端（D2 通道 4）。
    let cols = u16::try_from(dimensions.num_columns()).unwrap_or(u16::MAX);
    let rows = u16::try_from(dimensions.num_lines()).unwrap_or(u16::MAX);
    terminal_view.update(cx, |view, _| {
        view.note_viewport(cols, rows);
    });
    last_hovered_word
}

/// Terminal content sampled after the size sync, plus the timestamp gutter rows.
struct TerminalFrameSnapshot {
    mode: Modes,
    display_offset: usize,
    cursor_char: char,
    selection: Option<terminal::SelectionRange>,
    cursor: Cursor,
    show_timestamps: bool,
    timestamp_rows: Vec<Option<String>>,
}

fn terminal_frame_snapshot(
    terminal: &Entity<Terminal>,
    terminal_view: &Entity<TerminalView>,
    dimensions: TerminalBounds,
    cx: &mut App,
) -> TerminalFrameSnapshot {
    let (mode, display_offset, cursor_char, selection, cursor, row_snapshots) = {
        let content = &terminal.read(cx).last_content;
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
    let (show_timestamps, timestamp_rows) = terminal_view.update(cx, |view, _| {
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

    TerminalFrameSnapshot {
        mode,
        display_offset,
        cursor_char,
        selection,
        cursor,
        show_timestamps,
        timestamp_rows,
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<LayoutState>;

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
                let Some(hitbox) = hitbox else {
                    return None;
                };
                let style = terminal_paint_style(&self.terminal_view, cx);

                let (dimensions, line_height_px) =
                    terminal_dimensions(bounds, &style, &self.terminal, window, cx);

                let last_hovered_word = sync_terminal_frame(
                    &self.terminal,
                    &self.terminal_view,
                    dimensions,
                    window,
                    cx,
                );

                let TerminalFrameSnapshot {
                    mode,
                    display_offset,
                    cursor_char,
                    selection,
                    cursor,
                    show_timestamps,
                    timestamp_rows,
                } = terminal_frame_snapshot(&self.terminal, &self.terminal_view, dimensions, cx);
                let cells = &self.terminal.read(cx).last_content.cells;

                let relative_highlighted_ranges = selection_highlight_ranges(selection);
                let intersection = terminal_intersection(dimensions.bounds, window);

                let hover = last_hovered_word
                    .as_ref()
                    .map(|word| (style.link_style, &word.word_match));
                let (rects, batched_text_runs, block_element_rects) = layout_terminal_cells(
                    cells,
                    dimensions.bounds,
                    intersection,
                    &style.text_style,
                    hover,
                    CellLayoutMetrics {
                        minimum_contrast: style.minimum_contrast,
                        cell_width: dimensions.cell_width,
                        line_height_px,
                        rem_size: window.rem_size(),
                    },
                    cx,
                );

                // Layout cursor. Rectangle is used for IME, so we should lay it out even
                // if we don't end up showing it.
                let (cursor, ime_cursor_bounds) = layout_terminal_cursor(
                    CursorPaintContext {
                        cursor,
                        cursor_char,
                        display_offset,
                        dimensions,
                        focused: self.focused,
                        theme: &style.theme,
                    },
                    &style.text_style,
                    window,
                );

                Some(LayoutState {
                    hitbox,
                    batched_text_runs,
                    block_element_rects,
                    cursor,
                    ime_cursor_bounds,
                    background_color: style.theme.terminal_background,
                    dimensions,
                    rects,
                    relative_highlighted_ranges,
                    mode,
                    display_offset,
                    hovered_link: last_hovered_word.is_some(),
                    base_text_style: style.text_style,
                    timestamp_rows: if show_timestamps {
                        timestamp_rows
                    } else {
                        Vec::new()
                    },
                })
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
        let Some(layout) = layout.as_mut() else {
            return;
        };
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

                    let text_paint_time = paint_grid_content(layout, origin, window, cx);

                    if let Some(text_to_mark) = &marked_text_cloned {
                        paint_marked_text(layout, origin, text_to_mark, window, cx);
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

/// Paints cell backgrounds, selection highlights, batched text runs, and block
/// elements. Returns the time spent painting text runs and block elements.
fn paint_grid_content(
    layout: &LayoutState,
    origin: GpuiPoint<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> Duration {
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
    text_paint_start.elapsed()
}

/// Paints the IME marked text, including its underline and covering background.
fn paint_marked_text(
    layout: &LayoutState,
    origin: GpuiPoint<Pixels>,
    text_to_mark: &str,
    window: &mut Window,
    cx: &mut App,
) {
    if text_to_mark.is_empty() {
        return;
    }
    let Some(ime_bounds) = layout.ime_cursor_bounds else {
        return;
    };
    let ime_position = (ime_bounds + origin).origin;
    let mut ime_style = layout.base_text_style.clone();
    ime_style.underline = Some(UnderlineStyle {
        color: Some(ime_style.color),
        thickness: px(1.0),
        wavy: false,
    });

    let shaped_line = window.text_system().shape_line(
        text_to_mark.into(),
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

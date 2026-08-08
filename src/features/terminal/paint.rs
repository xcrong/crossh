//! GPUI paint-stage operations for terminal snapshots.

use gpui::*;
use vte::ansi::CursorShape;

use super::render::*;
use super::view::{
    KITTY_BACKGROUND_Z_INDEX, TIMESTAMP_GUTTER_GAP, TIMESTAMP_GUTTER_PADDING, TerminalImage,
    TerminalProgress,
};
use crate::shared::terminal::ImageDimension;
use crate::shared::ui::theme;

/// paint 阶段共享的绘制参数（收敛 paint_* 长参数列表）。
pub(crate) struct PaintContext<'a> {
    pub(crate) snapshot: &'a Snapshot,
    pub(crate) ime_marked_text: &'a str,
    pub(crate) shell_input: Option<&'a ShellInputRender>,
    pub(crate) canvas_bounds: Bounds<Pixels>,
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) cell_w: Pixels,
    pub(crate) line_h: Pixels,
    pub(crate) font_size: f32,
    pub(crate) show_timestamps: bool,
    pub(crate) font: &'a Font,
    pub(crate) default_fg: Hsla,
    pub(crate) default_bg: Hsla,
    pub(crate) images: &'a [TerminalImage],
    pub(crate) progress: Option<TerminalProgress>,
}

#[derive(Clone)]
pub(crate) struct ShellInputRender {
    pub(crate) text: String,
    pub(crate) cursor: usize,
    pub(crate) ime_marked_text: String,
}

/// 根据快照绘制。
pub(crate) fn paint_timestamp_gutter(ctx: &PaintContext, window: &mut Window, cx: &mut App) {
    let snapshot = ctx.snapshot;
    let canvas_bounds = ctx.canvas_bounds;
    let terminal_bounds = ctx.bounds;
    let line_h = ctx.line_h;
    let font_size = ctx.font_size;
    let font = ctx.font;
    let default_fg = ctx.default_fg;
    let default_bg = ctx.default_bg;
    window.paint_quad(quad(
        canvas_bounds,
        Corners::default(),
        default_bg,
        Edges::default(),
        hsla(0., 0., 0., 0.),
        gpui::BorderStyle::default(),
    ));

    let reserved_width = terminal_bounds.origin.x - canvas_bounds.origin.x;
    let gutter_width = reserved_width - px(TIMESTAMP_GUTTER_GAP);
    if gutter_width.as_f32() <= 1.0 {
        return;
    }

    let divider_color = Hsla::from(theme::border());
    window.paint_quad(quad(
        Bounds {
            origin: Point::new(
                canvas_bounds.origin.x + gutter_width - px(1.),
                canvas_bounds.origin.y,
            ),
            size: gpui::size(px(1.), canvas_bounds.size.height),
        },
        Corners::default(),
        divider_color,
        Edges::default(),
        hsla(0., 0., 0., 0.),
        gpui::BorderStyle::default(),
    ));

    let text_width = gutter_width.as_f32() - TIMESTAMP_GUTTER_PADDING;
    if text_width <= 0.0 {
        return;
    }
    let timestamp_color = Hsla {
        a: default_fg.a * 0.48,
        ..default_fg
    };

    for (row, timestamp) in snapshot.timestamps.iter().enumerate() {
        let Some(timestamp) = timestamp else {
            continue;
        };
        let text_run = TextRun {
            len: timestamp.len(),
            font: font.clone(),
            color: timestamp_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = window.text_system().shape_line(
            SharedString::from(timestamp.clone()),
            px((font_size - 2.0).max(8.0)),
            &[text_run],
            None,
        );
        let origin = Point::new(
            canvas_bounds.origin.x + px(TIMESTAMP_GUTTER_PADDING / 2.0),
            canvas_bounds.origin.y + px(row as f32 * line_h.as_f32()),
        );
        if let Err(error) = shaped.paint(
            origin,
            line_h,
            TextAlign::Right,
            Some(px(text_width)),
            window,
            cx,
        ) {
            log::warn!("paint timestamp row {row} failed: {error}");
        }
    }
}

/// Paint Crossh's timestamp gutter beside the official Zed terminal view.
/// `content_origin_y` is taken from Zed's terminal bounds so bottom-anchored
/// short terminals and the gutter use the same first-row position.
pub(crate) struct TimestampGutterOverlay<'a> {
    pub(crate) canvas_bounds: Bounds<Pixels>,
    pub(crate) content_origin_y: Pixels,
    pub(crate) timestamps: &'a [Option<String>],
    pub(crate) line_h: Pixels,
    pub(crate) font_size: f32,
    pub(crate) font: &'a Font,
    pub(crate) default_fg: Hsla,
    pub(crate) default_bg: Hsla,
}

impl TimestampGutterOverlay<'_> {
    pub(crate) fn paint(self, window: &mut Window, cx: &mut App) {
        let Self {
            canvas_bounds,
            content_origin_y,
            timestamps,
            line_h,
            font_size,
            font,
            default_fg,
            default_bg,
        } = self;
        let line_h = line_h.max(px(1.));
        window.paint_quad(quad(
            canvas_bounds,
            Corners::default(),
            default_bg,
            Edges::default(),
            hsla(0., 0., 0., 0.),
            gpui::BorderStyle::default(),
        ));

        if canvas_bounds.size.width.as_f32() <= 1.0 {
            return;
        }

        let divider_color = Hsla::from(theme::border());
        window.paint_quad(quad(
            Bounds {
                origin: Point::new(canvas_bounds.right() - px(1.), canvas_bounds.origin.y),
                size: gpui::size(px(1.), canvas_bounds.size.height),
            },
            Corners::default(),
            divider_color,
            Edges::default(),
            hsla(0., 0., 0., 0.),
            gpui::BorderStyle::default(),
        ));

        let text_width = canvas_bounds.size.width.as_f32() - TIMESTAMP_GUTTER_PADDING;
        if text_width <= 0.0 {
            return;
        }

        let timestamp_color = Hsla {
            a: default_fg.a * 0.48,
            ..default_fg
        };
        for (row, timestamp) in timestamps.iter().enumerate() {
            let Some(timestamp) = timestamp else {
                continue;
            };
            let text_run = TextRun {
                len: timestamp.len(),
                font: font.clone(),
                color: timestamp_color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped = window.text_system().shape_line(
                SharedString::from(timestamp.clone()),
                px((font_size - 2.0).max(8.0)),
                &[text_run],
                None,
            );
            let origin = Point::new(
                canvas_bounds.origin.x + px(TIMESTAMP_GUTTER_PADDING / 2.0),
                content_origin_y + px(row as f32 * line_h.as_f32()),
            );
            if let Err(error) = shaped.paint(
                origin,
                line_h,
                TextAlign::Right,
                Some(px(text_width)),
                window,
                cx,
            ) {
                log::warn!("paint timestamp overlay row {row} failed: {error}");
            }
        }
    }
}

pub(crate) fn paint_cell_backgrounds(
    snapshot: &Snapshot,
    bounds: Bounds<Pixels>,
    cell_w: f32,
    line_h: Pixels,
    default_bg: Hsla,
    window: &mut Window,
) {
    for (row_index, row) in snapshot.rows.iter().enumerate() {
        let mut start = 0usize;
        while start < row.len() {
            let color = row[start].bg;
            let mut end = start + 1;
            while end < row.len() && row[end].bg == color {
                end += 1;
            }

            if color != default_bg {
                window.paint_quad(quad(
                    Bounds {
                        origin: Point::new(
                            bounds.origin.x + px(start as f32 * cell_w),
                            bounds.origin.y + px(row_index as f32 * line_h.as_f32()),
                        ),
                        size: gpui::size(px((end - start) as f32 * cell_w), line_h),
                    },
                    Corners::default(),
                    color,
                    Edges::default(),
                    hsla(0., 0., 0., 0.),
                    gpui::BorderStyle::default(),
                ));
            }
            start = end;
        }
    }
}

pub(crate) fn paint_terminal_images(
    ctx: &PaintContext,
    window: &mut Window,
    cx: &mut App,
    under_text: bool,
    under_cell_background: bool,
) {
    if ctx.images.is_empty() {
        return;
    }
    let snapshot = ctx.snapshot;
    let top_line = snapshot.history_len.saturating_sub(snapshot.display_offset) as i64;
    let cell_w = ctx.cell_w.as_f32();
    let line_h = ctx.line_h.as_f32();
    let mut images = ctx
        .images
        .iter()
        .filter(|image| (image.z_index < 0) == under_text)
        .filter(|image| (image.z_index < KITTY_BACKGROUND_Z_INDEX) == under_cell_background)
        .collect::<Vec<_>>();
    images.sort_by_key(|image| {
        (
            image.z_index,
            image.kitty_id.unwrap_or(u32::MAX),
            image.placement_id.unwrap_or(u32::MAX),
        )
    });

    for image in images {
        let Some(render_image) = image.image.clone().get_render_image(window, cx) else {
            continue;
        };
        let natural = render_image.size(0);
        let natural_width = natural.width.0.max(1) as f32;
        let natural_height = natural.height.0.max(1) as f32;
        let mut width = image
            .width
            .map(|dimension| image_dimension_pixels(dimension, cell_w))
            .unwrap_or(natural_width);
        let mut height = image
            .height
            .map(|dimension| image_dimension_pixels(dimension, line_h))
            .unwrap_or(natural_height);

        if image.preserve_aspect_ratio {
            match (image.width, image.height) {
                (Some(_), None) => height = natural_height * width / natural_width,
                (None, Some(_)) => width = natural_width * height / natural_height,
                _ => {}
            }
        }
        if width <= 0.0 || height <= 0.0 {
            continue;
        }

        let origins = terminal_image_origins(image, ctx.images, snapshot, top_line, 0);
        for (row, column) in origins {
            let offset_x = if image.virtual_placement {
                0.
            } else {
                image.offset_x.min(cell_w.max(0.) as usize) as f32
            };
            let offset_y = if image.virtual_placement {
                0.
            } else {
                image.offset_y.min(line_h.max(0.) as usize) as f32
            };
            let image_bounds = Bounds {
                origin: Point::new(
                    ctx.bounds.origin.x + px(column as f32 * cell_w) + px(offset_x),
                    ctx.bounds.origin.y + px(row as f32 * line_h) + px(offset_y),
                ),
                size: gpui::size(px(width), px(height)),
            };
            if let Err(error) = window.paint_image(
                ctx.bounds,
                image_bounds,
                Corners::default(),
                render_image.clone(),
                0,
                false,
            ) {
                log::debug!("paint terminal image failed: {error}");
            }
        }
    }
}

pub(crate) fn paint_terminal_progress(ctx: &PaintContext, window: &mut Window) {
    let Some(progress) = ctx.progress else {
        return;
    };
    let height = px(2.0);
    let y = ctx.bounds.bottom() - height;
    let track = hsla(0.0, 0.0, 0.0, 0.32);
    let fill = match progress.state {
        2 => Hsla::from(theme::danger()),
        4 => Hsla::from(theme::warning()),
        _ => Hsla::from(theme::accent()),
    };
    let fraction = match progress.state {
        3 => 0.35,
        _ => progress.progress.unwrap_or(0) as f32 / 100.0,
    };
    window.paint_quad(quad(
        Bounds {
            origin: Point::new(ctx.bounds.origin.x, y),
            size: gpui::size(ctx.bounds.size.width, height),
        },
        Corners::default(),
        track,
        Edges::default(),
        hsla(0.0, 0.0, 0.0, 0.0),
        gpui::BorderStyle::default(),
    ));
    if fraction > 0.0 {
        window.paint_quad(quad(
            Bounds {
                origin: Point::new(ctx.bounds.origin.x, y),
                size: gpui::size(ctx.bounds.size.width * fraction.min(1.0), height),
            },
            Corners::default(),
            fill,
            Edges::default(),
            hsla(0.0, 0.0, 0.0, 0.0),
            gpui::BorderStyle::default(),
        ));
    }
}

pub(crate) fn image_dimension_pixels(dimension: ImageDimension, cell_size: f32) -> f32 {
    match dimension {
        ImageDimension::Cells(cells) => cells as f32 * cell_size,
        ImageDimension::Pixels(pixels) => pixels as f32,
    }
}

pub(crate) fn image_dimension_cells(dimension: Option<ImageDimension>) -> Option<usize> {
    match dimension {
        Some(ImageDimension::Cells(cells)) => Some(cells),
        Some(ImageDimension::Pixels(_)) | None => None,
    }
}

pub(crate) fn terminal_image_origins(
    image: &TerminalImage,
    images: &[TerminalImage],
    snapshot: &Snapshot,
    top_line: i64,
    depth: usize,
) -> Vec<(i64, i64)> {
    if depth >= 8 {
        return Vec::new();
    }
    if let Some(parent_id) = image.relative_image_id {
        let Some(parent) = images.iter().rev().find(|parent| {
            parent.kitty_id == Some(parent_id)
                && image
                    .relative_placement_id
                    .is_none_or(|placement| parent.placement_id == Some(placement))
        }) else {
            return Vec::new();
        };
        return terminal_image_origins(parent, images, snapshot, top_line, depth + 1)
            .into_iter()
            .map(|(row, column)| {
                (
                    row.saturating_add(i64::from(image.relative_offset_y)),
                    column.saturating_add(i64::from(image.relative_offset_x)),
                )
            })
            .collect();
    }

    if image.virtual_placement {
        let Some(image_id) = image.kitty_id else {
            return Vec::new();
        };
        let mut origins = Vec::new();
        for placeholder in snapshot
            .kitty_placeholders
            .iter()
            .filter(|placeholder| placeholder.image_id == image_id)
            .filter(|placeholder| {
                image.placement_id.is_none() || image.placement_id == placeholder.placement_id
            })
        {
            let origin = (
                placeholder.viewport_row as i64 - placeholder.row as i64,
                placeholder.viewport_column as i64 - placeholder.column as i64,
            );
            if !origins.contains(&origin) {
                origins.push(origin);
            }
        }
        origins
    } else {
        vec![(image.origin_line - top_line, image.origin_col as i64)]
    }
}

pub(crate) fn paint_shell_input(
    ctx: &PaintContext,
    input: &ShellInputRender,
    window: &mut Window,
    cx: &mut App,
) {
    let Some((col, row)) = ctx.snapshot.cursor else {
        return;
    };

    let text = &input.text;
    let cursor = input.cursor.min(text.len());
    let before = &text[..cursor];
    let after = &text[cursor..];
    let mut display = String::with_capacity(text.len() + input.ime_marked_text.len());
    display.push_str(before);
    display.push_str(&input.ime_marked_text);
    display.push_str(after);

    let regular_run = TextRun {
        len: 0,
        font: ctx.font.clone(),
        color: ctx.default_fg,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let ime_run = TextRun {
        len: input.ime_marked_text.len(),
        font: ctx.font.clone(),
        color: ctx.default_fg,
        background_color: None,
        underline: Some(UnderlineStyle {
            thickness: px(1.0),
            color: Some(ctx.default_fg),
            wavy: false,
        }),
        strikethrough: None,
    };
    let runs = if input.ime_marked_text.is_empty() {
        vec![TextRun {
            len: display.len(),
            ..regular_run.clone()
        }]
    } else {
        vec![
            TextRun {
                len: before.len(),
                ..regular_run.clone()
            },
            ime_run,
            TextRun {
                len: after.len(),
                ..regular_run
            },
        ]
    };
    let shaped = window.text_system().shape_line(
        SharedString::from(display),
        px(ctx.font_size),
        &runs,
        None,
    );
    let origin = Point::new(
        ctx.bounds.origin.x + px(col as f32 * ctx.cell_w.as_f32()),
        ctx.bounds.origin.y + px(row as f32 * ctx.line_h.as_f32()),
    );
    let width = shaped.width().max(ctx.cell_w);
    window.paint_quad(quad(
        Bounds {
            origin,
            size: gpui::size(width, ctx.line_h),
        },
        Corners::default(),
        ctx.default_bg,
        Edges::default(),
        hsla(0., 0., 0., 0.),
        gpui::BorderStyle::default(),
    ));
    if (!input.text.is_empty() || !input.ime_marked_text.is_empty())
        && let Err(error) = shaped.paint(origin, ctx.line_h, TextAlign::Left, None, window, cx)
    {
        log::warn!("paint shell input failed: {error}");
    }

    if ctx.snapshot.cursor_visible {
        let cursor_prefix = format!("{before}{}", input.ime_marked_text);
        let cursor_run = TextRun {
            len: cursor_prefix.len(),
            font: ctx.font.clone(),
            color: ctx.default_fg,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let cursor_width = if cursor_prefix.is_empty() {
            px(0.)
        } else {
            window
                .text_system()
                .shape_line(
                    SharedString::from(cursor_prefix),
                    px(ctx.font_size),
                    &[cursor_run],
                    None,
                )
                .width()
        };
        window.paint_quad(quad(
            Bounds {
                origin: Point::new(origin.x + cursor_width, origin.y),
                size: gpui::size(px(1.5).min(ctx.cell_w), ctx.line_h),
            },
            Corners::default(),
            ctx.default_fg,
            Edges::default(),
            hsla(0., 0., 0., 0.),
            gpui::BorderStyle::default(),
        ));
    }
}

pub(crate) fn paint_snapshot(ctx: &PaintContext, window: &mut Window, cx: &mut App) {
    let snapshot = ctx.snapshot;
    let ime_marked_text = ctx.ime_marked_text;
    let shell_input = ctx.shell_input;
    let bounds = ctx.bounds;
    let cell_w = ctx.cell_w;
    let line_h = ctx.line_h;
    let font_size = ctx.font_size;
    let show_timestamps = ctx.show_timestamps;
    let font = ctx.font;
    let default_fg = ctx.default_fg;
    let default_bg = ctx.default_bg;
    let cell_wf = cell_w.as_f32();
    let line_hf = line_h.as_f32();

    if show_timestamps {
        paint_timestamp_gutter(ctx, window, cx);
    }

    // 诊断：打印每帧实际拿到的 snapshot 内容（非空行），用于判断
    // 是「读不到内容」还是「画不出来」。trace 级别，避免 debug 下日志爆炸。
    if log::log_enabled!(log::Level::Trace) {
        log::trace!(
            "paint_snapshot: {} rows, bounds={}x{} cell_w={} line_h={}",
            snapshot.rows.len(),
            bounds.size.width.as_f32() as u32,
            bounds.size.height.as_f32() as u32,
            cell_wf,
            line_hf
        );
        for (r, row) in snapshot.rows.iter().enumerate() {
            let s: String = row
                .iter()
                .map(|c| {
                    if c.spacer || c.kitty_placeholder {
                        ' '
                    } else {
                        c.ch
                    }
                })
                .collect();
            let t = s.trim_end();
            if !t.is_empty() {
                log::trace!("  snapshot row {:2}: {:?}", r, t);
            }
        }
    }

    // 背景填充整个视口。
    window.paint_quad(quad(
        bounds,
        Corners::default(),
        default_bg,
        Edges::default(),
        hsla(0., 0., 0., 0.),
        gpui::BorderStyle::default(),
    ));

    // Kitty negative z-index placements sit below terminal text. They are
    // painted after the base fill but before non-default cell backgrounds.
    paint_terminal_images(ctx, window, cx, true, true);

    // Paint cell backgrounds separately from glyphs. GPUI text backgrounds are
    // glyph-run decorations and do not reliably cover blank cells or preserve
    // the terminal selection layer.
    paint_cell_backgrounds(snapshot, bounds, cell_wf, line_h, default_bg, window);

    // Ordinary negative z-index images sit above non-default cell backgrounds
    // but below text. Kitty reserves lower-than-INT32_MIN/2 z values for
    // images that should also be covered by cell backgrounds.
    paint_terminal_images(ctx, window, cx, true, false);

    // 选择高亮使用主题的 mint/teal 色，提高深色终端中的可见度。
    let sel_bg = hsla(0.43, 0.58, 0.42, 0.78);

    for (r, row) in snapshot.rows.iter().enumerate() {
        // 绘制选择高亮背景。
        if let Some(((ax, ay), (bx, by))) = snapshot.selection {
            let r0 = ay.min(by);
            let r1 = ay.max(by);
            if r >= r0 && r <= r1 {
                let cols = snapshot.cols;
                let (c0, c1) = if r == r0 && r == r1 {
                    (ax.min(bx), ax.max(bx))
                } else if r == r0 {
                    (ax.min(bx), cols.saturating_sub(1))
                } else if r == r1 {
                    (0, ax.max(bx))
                } else {
                    (0, cols.saturating_sub(1))
                };
                if c0 <= c1 {
                    let x = bounds.origin.x + px(c0 as f32 * cell_wf);
                    let w = px((c1 - c0 + 1) as f32 * cell_wf);
                    let y = bounds.origin.y + px(r as f32 * line_hf);
                    window.paint_quad(quad(
                        Bounds {
                            origin: Point::new(x, y),
                            size: gpui::size(w, line_h),
                        },
                        Corners::default(),
                        sel_bg,
                        Edges::default(),
                        hsla(0., 0., 0., 0.),
                        gpui::BorderStyle::default(),
                    ));
                }
            }
        }

        let row_y = bounds.origin.y + px(r as f32 * line_hf);
        for run in terminal_text_runs(row) {
            let underline = match run.underline {
                UnderlineKind::None if !run.is_url => None,
                UnderlineKind::Wavy => Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(run.underline_color),
                    wavy: true,
                }),
                UnderlineKind::Solid | UnderlineKind::None => Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(run.underline_color),
                    wavy: false,
                }),
            };
            let strikethrough = run.strikeout.then(|| StrikethroughStyle {
                thickness: px(1.0),
                color: Some(run.fg),
            });
            let text_len = run.text.len();
            let text_run = TextRun {
                len: text_len,
                font: Font {
                    weight: if run.bold {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    },
                    style: if run.italic {
                        gpui::FontStyle::Italic
                    } else {
                        gpui::FontStyle::Normal
                    },
                    family: font.family.clone(),
                    features: font.features.clone(),
                    fallbacks: font.fallbacks.clone(),
                },
                color: run.fg,
                background_color: None,
                underline,
                strikethrough,
            };
            let shaped = window.text_system().shape_line(
                SharedString::from(run.text),
                px(font_size),
                &[text_run],
                Some(px(cell_wf * run.force_width_cells as f32)),
            );
            let origin = Point::new(bounds.origin.x + px(run.start_col as f32 * cell_wf), row_y);
            if let Err(e) = shaped.paint(origin, line_h, TextAlign::Left, None, window, cx) {
                log::warn!("paint row {r} failed: {e}");
            }
        }
    }

    paint_terminal_images(ctx, window, cx, false, false);
    paint_terminal_progress(ctx, window);

    if let Some(shell_input) = shell_input {
        paint_shell_input(ctx, shell_input, window, cx);
    }

    // 光标形状由 DECSCUSR/OSC 50 控制；blink 已在快照阶段按终端状态处理。
    if shell_input.is_none()
        && snapshot.cursor_visible
        && ime_marked_text.is_empty()
        && let Some((col, row)) = snapshot.cursor
        && snapshot
            .rows
            .get(row)
            .and_then(|cells| cells.get(col))
            .is_some()
        && snapshot.cursor_shape != CursorShape::Hidden
    {
        let row_cells = &snapshot.rows[row];
        let (cursor_col, cursor_cell_count, glyph_col) = cursor_visual_span(row_cells, col);
        let cursor_cell = &row_cells[glyph_col];
        let cursor_width = px(cursor_cell_count as f32 * cell_wf);
        let x = bounds.origin.x + px(cursor_col as f32 * cell_wf);
        let y = bounds.origin.y + px(row as f32 * line_hf);
        let (cursor_origin, cursor_size, cursor_fill, cursor_edges) = match snapshot.cursor_shape {
            CursorShape::Beam => (
                Point::new(x, y),
                gpui::size(px(cell_wf.clamp(1.0, 2.0)), line_h),
                cursor_cell.fg,
                Edges::default(),
            ),
            CursorShape::Underline => {
                let height = px(2.0).min(line_h);
                (
                    Point::new(x, y + line_h - height),
                    gpui::size(cursor_width, height),
                    cursor_cell.fg,
                    Edges::default(),
                )
            }
            CursorShape::HollowBlock => (
                Point::new(x, y),
                gpui::size(cursor_width, line_h),
                hsla(0., 0., 0., 0.),
                Edges::all(px(1.)),
            ),
            CursorShape::Block | CursorShape::Hidden => (
                Point::new(x, y),
                gpui::size(cursor_width, line_h),
                cursor_cell.fg,
                Edges::default(),
            ),
        };
        let cb = Bounds {
            origin: cursor_origin,
            size: cursor_size,
        };
        window.paint_quad(quad(
            cb,
            Corners::default(),
            cursor_fill,
            cursor_edges,
            cursor_cell.fg,
            gpui::BorderStyle::default(),
        ));

        // The cursor quad is painted after the row text, so repaint the cell's
        // glyph with the effective background color to keep the character
        // readable instead of hiding it beneath the cursor block.
        if snapshot.cursor_shape == CursorShape::Block && !cursor_cell.spacer {
            let mut cursor_text =
                String::with_capacity(cursor_cell.ch.len_utf8() + cursor_cell.zero_width.len());
            cursor_text.push(cursor_cell.ch);
            cursor_text.push_str(&cursor_cell.zero_width);
            let underline = match cursor_cell.underline {
                UnderlineKind::None => None,
                UnderlineKind::Solid => Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(cursor_cell.underline_color),
                    wavy: false,
                }),
                UnderlineKind::Wavy => Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(cursor_cell.underline_color),
                    wavy: true,
                }),
            };
            let text_run = TextRun {
                len: cursor_text.len(),
                font: Font {
                    weight: if cursor_cell.bold {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    },
                    style: if cursor_cell.italic {
                        gpui::FontStyle::Italic
                    } else {
                        gpui::FontStyle::Normal
                    },
                    family: font.family.clone(),
                    features: font.features.clone(),
                    fallbacks: font.fallbacks.clone(),
                },
                color: if cursor_cell.bg == default_bg {
                    default_bg
                } else {
                    cursor_cell.bg
                },
                background_color: None,
                underline,
                strikethrough: cursor_cell.strikeout.then(|| StrikethroughStyle {
                    thickness: px(1.0),
                    color: Some(cursor_cell.bg),
                }),
            };
            let shaped = window.text_system().shape_line(
                SharedString::from(cursor_text),
                px(font_size),
                &[text_run],
                None,
            );
            if let Err(error) =
                shaped.paint(Point::new(x, y), line_h, TextAlign::Left, None, window, cx)
            {
                log::warn!("paint cursor glyph failed: {error}");
            }
        }
    }

    // 合成阶段的拼音由输入法暂存，不能提前写入 PTY；在光标处绘制出来，
    // 让用户能看到当前正在组合的文本，提交后才由 replace_text_in_range 发送。
    if shell_input.is_none()
        && !ime_marked_text.is_empty()
        && let Some((col, row)) = snapshot.cursor
    {
        let origin = Point::new(
            bounds.origin.x + px(col as f32 * cell_wf),
            bounds.origin.y + px(row as f32 * line_hf),
        );
        let marked_runs = [TextRun {
            len: ime_marked_text.len(),
            font: font.clone(),
            color: default_fg,
            background_color: Some(default_bg),
            underline: Some(gpui::UnderlineStyle {
                thickness: px(1.0),
                color: Some(default_fg),
                wavy: false,
            }),
            strikethrough: None,
        }];
        let shaped = window.text_system().shape_line(
            SharedString::from(ime_marked_text.to_string()),
            px(font_size),
            &marked_runs,
            None,
        );
        let width = shaped.width().max(cell_w);
        window.paint_quad(quad(
            Bounds {
                origin,
                size: gpui::size(width, line_h),
            },
            Corners::default(),
            default_bg,
            Edges::default(),
            hsla(0., 0., 0., 0.),
            gpui::BorderStyle::default(),
        ));
        if let Err(e) = shaped.paint(origin, line_h, TextAlign::Left, None, window, cx) {
            log::warn!("paint IME marked text failed: {e}");
        }
    }

    // 滚动条指示器（右侧窄条）。
    let display_offset = snapshot.display_offset;
    let history_len = snapshot.history_len;
    if history_len > 0 && display_offset > 0 {
        let sb_w = px(6.);
        let sb_x = bounds.right() - sb_w;
        let sb_h = bounds.size.height;
        let thumb_h =
            sb_h * (snapshot.rows.len() as f32 / (history_len + snapshot.rows.len()) as f32);
        let thumb_y = sb_h * ((history_len - display_offset) as f32 / history_len as f32);
        window.paint_quad(quad(
            Bounds {
                origin: Point::new(sb_x, bounds.origin.y),
                size: gpui::size(sb_w, sb_h),
            },
            Corners::default(),
            hsla(0., 0., 0.2, 0.15),
            Edges::default(),
            hsla(0., 0., 0., 0.),
            gpui::BorderStyle::default(),
        ));
        window.paint_quad(quad(
            Bounds {
                origin: Point::new(sb_x, bounds.origin.y + thumb_y),
                size: gpui::size(sb_w, thumb_h.min(sb_h)),
            },
            Corners::default(),
            hsla(0., 0., 0.5, 0.3),
            Edges::default(),
            hsla(0., 0., 0., 0.),
            gpui::BorderStyle::default(),
        ));
    }
}

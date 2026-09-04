// Copyright (c) 2026 Crossh contributors.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 弹窗定位几何：纯函数，无 Window 依赖，可单元测试。
//!
//! 调用方先读出视口尺寸（如 `window.viewport_size()`），组装
//! [`PopupRequest`] 后调用 [`place_popup`]；`gap` 由本模块统一施加，
//! 调用方只需传入未偏移的锚点。

use gpui::{Pixels, Point, Size, px};

/// 弹窗定位的纯输入。
pub struct PopupRequest {
    anchor: Point<Pixels>,
    popup: Size<Pixels>,
    viewport: Size<Pixels>,
    gap: Pixels,
    margin: Pixels,
}

impl PopupRequest {
    /// 以锚点（窗口坐标，未含 `gap` 偏移）、弹窗尺寸、视口尺寸创建。
    ///
    /// 默认 `gap` 为 0、`margin` 为 8px（弹窗不贴屏幕边缘）。
    pub fn new(anchor: Point<Pixels>, popup: Size<Pixels>, viewport: Size<Pixels>) -> Self {
        Self {
            anchor,
            popup,
            viewport,
            gap: px(0.),
            margin: px(8.),
        }
    }

    /// 锚点与弹窗展开方向上的间距。
    pub fn with_gap(mut self, gap: Pixels) -> Self {
        self.gap = gap;
        self
    }

    /// 弹窗与视口边缘的最小距离。
    pub fn with_margin(mut self, margin: Pixels) -> Self {
        self.margin = margin;
        self
    }

    /// 锚点。
    pub fn anchor(&self) -> Point<Pixels> {
        self.anchor
    }

    /// 弹窗尺寸。
    pub fn popup(&self) -> Size<Pixels> {
        self.popup
    }

    /// 视口尺寸。
    pub fn viewport(&self) -> Size<Pixels> {
        self.viewport
    }

    /// 间距。
    pub fn gap(&self) -> Pixels {
        self.gap
    }

    /// 边距。
    pub fn margin(&self) -> Pixels {
        self.margin
    }
}

/// 弹窗定位的纯输出。
pub struct PopupPlacement {
    origin: Point<Pixels>,
    placed_above: bool,
    placed_left: bool,
}

impl PopupPlacement {
    /// 钳制后的弹窗左上原点（窗口坐标，整像素）。
    pub fn origin(&self) -> Point<Pixels> {
        self.origin
    }

    /// 是否向上翻转展开。
    pub fn is_above(&self) -> bool {
        self.placed_above
    }

    /// 是否向左翻转展开。
    pub fn is_left(&self) -> bool {
        self.placed_left
    }
}

fn clamp_axis(value: f32, size: f32, total: f32, margin: f32) -> f32 {
    let margin = margin.max(0.0);
    let max = (total - size - margin).max(margin);
    value.clamp(margin, max)
}

/// 通用原点钳制：把弹窗完整保留在视口 `margin` 内；弹窗比视口还大时贴 `margin`。
pub fn clamp_origin(
    origin: Point<Pixels>,
    popup: Size<Pixels>,
    viewport: Size<Pixels>,
    margin: Pixels,
) -> Point<Pixels> {
    let margin = margin.as_f32();
    Point::new(
        px(clamp_axis(
            origin.x.as_f32(),
            popup.width.as_f32(),
            viewport.width.as_f32(),
            margin,
        )
        .round()),
        px(clamp_axis(
            origin.y.as_f32(),
            popup.height.as_f32(),
            viewport.height.as_f32(),
            margin,
        )
        .round()),
    )
}

/// 默认向右下展开，越界则向左 / 上翻转，翻转后仍越界则钳制到 `margin` 内。
///
/// `is_above` / `is_left` 报告翻转决策，最终位置以 [`PopupPlacement::origin`] 为准。
pub fn place_popup(request: &PopupRequest) -> PopupPlacement {
    let margin = request.margin.as_f32();
    let viewport_width = request.viewport.width.as_f32();
    let viewport_height = request.viewport.height.as_f32();
    let popup_width = request.popup.width.as_f32();
    let popup_height = request.popup.height.as_f32();
    let anchor_x = request.anchor.x.as_f32();
    let anchor_y = request.anchor.y.as_f32();
    let gap = request.gap.as_f32().max(0.0);

    let mut x = anchor_x + gap;
    let mut placed_left = false;
    if x + popup_width > viewport_width - margin {
        x = anchor_x - popup_width - gap;
        placed_left = true;
    }

    let mut y = anchor_y + gap;
    let mut placed_above = false;
    if y + popup_height > viewport_height - margin {
        y = anchor_y - popup_height - gap;
        placed_above = true;
    }

    let origin = clamp_origin(
        Point::new(px(x), px(y)),
        request.popup,
        request.viewport,
        request.margin,
    );
    PopupPlacement {
        origin,
        placed_above,
        placed_left,
    }
}

#[cfg(test)]
mod tests {
    use gpui::{px, size};

    use super::{PopupRequest, clamp_origin, place_popup};

    fn point(pair: (f32, f32)) -> gpui::Point<gpui::Pixels> {
        gpui::Point::new(px(pair.0), px(pair.1))
    }

    fn request(anchor: (f32, f32), popup: (f32, f32), viewport: (f32, f32)) -> PopupRequest {
        PopupRequest::new(
            point(anchor),
            size(px(popup.0), px(popup.1)),
            size(px(viewport.0), px(viewport.1)),
        )
        .with_margin(px(0.))
    }

    #[test]
    fn expands_down_right_by_default() {
        let placement = place_popup(&request((10., 20.), (100., 80.), (800., 600.)));
        assert_eq!(placement.origin(), point((10., 20.)));
        assert!(!placement.is_above());
        assert!(!placement.is_left());
    }

    #[test]
    fn flips_left_on_right_overflow() {
        let placement = place_popup(&request((750., 20.), (100., 80.), (800., 600.)));
        assert_eq!(placement.origin(), point((650., 20.)));
        assert!(placement.is_left());
        assert!(!placement.is_above());
    }

    #[test]
    fn clamps_when_flipped_origin_still_overflows() {
        // 弹窗比视口宽：左翻后仍溢出，钳制到 margin（此处为 0）。
        let placement = place_popup(&request((750., 20.), (900., 80.), (800., 600.)));
        assert_eq!(placement.origin(), point((0., 20.)));
        assert!(placement.is_left());
    }

    #[test]
    fn flips_up_on_bottom_overflow() {
        let placement = place_popup(&request((10., 550.), (100., 80.), (800., 600.)));
        assert_eq!(placement.origin(), point((10., 470.)));
        assert!(placement.is_above());
        assert!(!placement.is_left());
    }

    #[test]
    fn clamps_tiny_viewport_to_margin() {
        let origin = clamp_origin(
            point((-50., 900.)),
            size(px(100.), px(80.)),
            size(px(200.), px(100.)),
            px(8.),
        );
        assert_eq!(origin, point((8., 12.)));
    }

    #[test]
    fn gap_offsets_default_and_flipped_origins() {
        let request = PopupRequest::new(
            point((10., 20.)),
            size(px(100.), px(80.)),
            size(px(800.), px(600.)),
        )
        .with_gap(px(4.))
        .with_margin(px(0.));
        let placement = place_popup(&request);
        assert_eq!(placement.origin(), point((14., 24.)));

        let flipped = PopupRequest::new(
            point((750., 550.)),
            size(px(100.), px(80.)),
            size(px(800.), px(600.)),
        )
        .with_gap(px(4.))
        .with_margin(px(0.));
        let placement = place_popup(&flipped);
        assert_eq!(placement.origin(), point((646., 466.)));
        assert!(placement.is_above());
        assert!(placement.is_left());
    }
}

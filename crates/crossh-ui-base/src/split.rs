// Copyright (c) 2026 Crossh contributors.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 分栏拖拽几何：手柄方向、轴向与指针换算纯函数。
//!
//! 拖拽是否进行中、当前尺寸由调用方持有；地基层只做换算与钳制，不触碰应用状态。

use gpui::{Bounds, Pixels};

/// 拖拽手柄贴在面板的哪条边。
///
/// 两侧都是真实契约：`Right`（默认）给手柄贴右边缘的面板用，宽度从面板左边缘
/// 量到指针；`Left` 给手柄贴左边缘的面板用，宽度从指针量到面板右边缘。新增侧向
/// 面板时直接选取变体即可，无需改动调用方。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SplitHandleSide {
    /// 手柄贴右边缘：宽度 = 指针横坐标 − 面板左边缘。
    #[default]
    Right,
    /// 手柄贴左边缘：宽度 = 面板右边缘 − 指针横坐标。
    Left,
}

/// 拖拽调整的轴向。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SplitAxis {
    /// 水平调宽。
    #[default]
    Horizontal,
    /// 垂直调高。
    Vertical,
}

/// 从面板 bounds 与指针横坐标换算当前宽度（未钳制，调用方再过 [`clamp_size`]）。
pub fn drag_width(side: SplitHandleSide, bounds: &Bounds<Pixels>, pointer_x: Pixels) -> f32 {
    match side {
        SplitHandleSide::Left => bounds.right().as_f32() - pointer_x.as_f32(),
        SplitHandleSide::Right => pointer_x.as_f32() - bounds.origin.x.as_f32(),
    }
}

/// 从面板 bounds 与指针纵坐标换算当前高度（未钳制，调用方再过 [`clamp_size`]）。
pub fn drag_height(bounds: &Bounds<Pixels>, pointer_y: Pixels) -> f32 {
    pointer_y.as_f32() - bounds.origin.y.as_f32()
}

/// 把拖拽换算出的尺寸钳制到调用方给定的 `[min_size, max_size]`。
pub fn clamp_size(value: f32, min_size: f32, max_size: f32) -> f32 {
    value.clamp(min_size, max_size)
}

#[cfg(test)]
mod tests {
    use gpui::{Point, Size, px};

    use super::*;

    fn bounds() -> Bounds<Pixels> {
        Bounds::new(Point::new(px(40.), px(10.)), Size::new(px(200.), px(50.)))
    }

    #[test]
    fn right_side_measures_from_left_edge() {
        let bounds = bounds();
        assert_eq!(drag_width(SplitHandleSide::Right, &bounds, px(140.)), 100.0);
        assert_eq!(drag_width(SplitHandleSide::Right, &bounds, px(40.)), 0.0);
    }

    #[test]
    fn left_side_measures_from_right_edge() {
        let bounds = bounds();
        assert_eq!(drag_width(SplitHandleSide::Left, &bounds, px(140.)), 100.0);
        assert_eq!(drag_width(SplitHandleSide::Left, &bounds, px(240.)), 0.0);
    }

    #[test]
    fn height_measures_from_top_edge() {
        let bounds = bounds();
        assert_eq!(drag_height(&bounds, px(60.)), 50.0);
        assert_eq!(drag_height(&bounds, px(10.)), 0.0);
    }

    #[test]
    fn size_clamps_to_given_range() {
        assert_eq!(clamp_size(20.0, 80.0, 600.0), 80.0);
        assert_eq!(clamp_size(700.0, 80.0, 600.0), 600.0);
        assert_eq!(clamp_size(200.0, 80.0, 600.0), 200.0);
    }
}

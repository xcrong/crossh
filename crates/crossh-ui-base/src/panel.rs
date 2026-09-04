// Copyright (c) 2026 Crossh contributors.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 侧栏面板几何：宽度钳制、主区可用宽度、手柄方向推导、Rail 常量。
//!
//! 只算数、不渲染；边框、背景、手柄挂载由应用层决定。

use gpui::{Pixels, px};

use crate::split::SplitHandleSide;

/// 面板贴在窗口的哪一边：决定边框与拖拽手柄朝向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelSide {
    /// 左侧面板：手柄贴右边缘。
    Left,
    /// 右侧面板：手柄贴左边缘。
    Right,
}

/// Rail 头像项边长；与 [`RAIL_AVATAR_GAP`] 相加得纵向行距 34。
pub const RAIL_AVATAR_SIZE: f32 = 30.0;
/// Rail 头像项纵向间距。
pub const RAIL_AVATAR_GAP: f32 = 4.0;

/// 把原始宽度钳到 `[min_width, max_width]`；NaN 回落到 `min_width`，保证不产生
/// 负可用宽度。
pub fn clamp_panel_width(value: f32, min_width: f32, max_width: f32) -> f32 {
    if value.is_nan() {
        return min_width;
    }
    value.clamp(min_width, max_width)
}

/// 主区可用宽度：`max(视口 − 侧栏 − 其余, 0)`，永不为负。
pub fn available_main_width(
    viewport_width: Pixels,
    sidebar_width: f32,
    other_width: f32,
) -> Pixels {
    px((viewport_width.as_f32() - sidebar_width - other_width).max(0.0))
}

/// 手柄方向推导：左侧面板手柄贴右，右侧面板手柄贴左。
pub fn handle_side_for(side: PanelSide) -> SplitHandleSide {
    match side {
        PanelSide::Left => SplitHandleSide::Right,
        PanelSide::Right => SplitHandleSide::Left,
    }
}

#[cfg(test)]
mod tests {
    use gpui::px;

    use super::*;

    #[test]
    fn width_clamps_to_range() {
        assert_eq!(clamp_panel_width(100.0, 216.0, 360.0), 216.0);
        assert_eq!(clamp_panel_width(500.0, 216.0, 360.0), 360.0);
        assert_eq!(clamp_panel_width(300.0, 216.0, 360.0), 300.0);
    }

    #[test]
    fn width_absorbs_nan_negative_and_overflow() {
        assert_eq!(clamp_panel_width(f32::NAN, 216.0, 360.0), 216.0);
        assert_eq!(clamp_panel_width(f32::INFINITY, 216.0, 360.0), 360.0);
        assert_eq!(clamp_panel_width(f32::NEG_INFINITY, 216.0, 360.0), 216.0);
        assert_eq!(clamp_panel_width(-100.0, 216.0, 360.0), 216.0);
    }

    #[test]
    fn main_width_truncates_at_zero() {
        assert_eq!(available_main_width(px(700.), 216., 240.), px(244.));
        assert_eq!(available_main_width(px(700.), 44., 40.), px(616.));
        assert_eq!(available_main_width(px(400.), 216., 240.), px(0.));
    }

    #[test]
    fn handle_side_follows_panel_side() {
        assert_eq!(handle_side_for(PanelSide::Left), SplitHandleSide::Right);
        assert_eq!(handle_side_for(PanelSide::Right), SplitHandleSide::Left);
    }

    #[test]
    fn rail_pitch_is_avatar_plus_gap() {
        assert_eq!(RAIL_AVATAR_SIZE + RAIL_AVATAR_GAP, 34.0);
    }
}

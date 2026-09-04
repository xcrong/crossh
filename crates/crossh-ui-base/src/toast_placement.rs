// Copyright (c) 2026 Crossh contributors.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Toast 层底部偏移：状态栏高度与间隙相加的纯函数。
//!
//! 主题常量由调用方传入；地基层只做加法，不导入主题。

/// Toast 层距离底部的偏移（状态栏高度加间隙）。
pub fn toaster_bottom_offset(status_bar_height: f32, gap: f32) -> f32 {
    status_bar_height + gap
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_gap_returns_status_bar_height() {
        assert_eq!(toaster_bottom_offset(27.0, 0.0), 27.0);
    }

    #[test]
    fn typical_offset_adds_gap_above_status_bar() {
        assert_eq!(toaster_bottom_offset(27.0, 8.0), 35.0);
    }

    #[test]
    fn large_values_add_up() {
        assert_eq!(toaster_bottom_offset(10_000.0, 8.0), 10_008.0);
    }
}

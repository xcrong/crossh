// Copyright (c) 2026 Crossh contributors.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 无主题布局骨架：横向容器与纵向滚动容器。
//!
//! 只定结构（flex 方向、滚动跟踪），不管颜色、间距、字号；滚动偏移记入调用方
//! 持有的 [`ScrollHandle`](gpui::ScrollHandle)，地基层不持有任何应用状态。

use gpui::{
    Div, InteractiveElement, ScrollHandle, Stateful, StatefulInteractiveElement, Styled, div,
};

/// 横向排列、纵轴居中的容器。
pub fn h_flex() -> Div {
    div().flex().flex_row().items_center()
}

/// 纵向滚动容器：滚动偏移记入 `handle`。
///
/// 布局类修饰（如 `flex_1().min_h_0()`）留在调用方；紧接着链式 `.id(...)`，
/// 这里的占位 id 会被调用方自己的 id 替换（stateful 包装保留）。
pub fn scroll_y(handle: &ScrollHandle) -> Stateful<Div> {
    div()
        .id(SCROLL_PLACEHOLDER_ID)
        .overflow_y_scroll()
        .track_scroll(handle)
}

/// [`scroll_y`] 的内部占位元素 id，会被调用方的链式 `.id(...)` 覆盖。
const SCROLL_PLACEHOLDER_ID: usize = 0;

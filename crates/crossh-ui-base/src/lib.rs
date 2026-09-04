// Copyright (c) 2026 Crossh contributors.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 无样式行为 / 几何基础：行为归地基，表现归应用。
//!
//! - 只做语义、焦点、键盘、可访问性、几何钳制、受控状态机。
//! - 不做颜色、内外边距、圆角、阴影、字号（全部由 `crossh-ui-component`
//!   用 `crossh-ui` 的 token 适配）。
//! - 受控值：接受当前值，报告变化，永不静默改应用状态。

pub mod avatar_text;
pub mod button;
pub mod layout;
pub mod list_state;
pub mod panel;
pub mod positioner;
pub mod split;
pub mod text_selection;
pub mod text_state;
pub mod toast_placement;
pub mod toggle;

pub use avatar_text::abbreviation;
pub use button::{BaseButton, ButtonPress};
pub use layout::{h_flex, scroll_y};
pub use list_state::{ListCursor, ListStatus, StepDirection, next_index};
pub use panel::{
    PanelSide, RAIL_AVATAR_GAP, RAIL_AVATAR_SIZE, available_main_width, clamp_panel_width,
    handle_side_for,
};
pub use positioner::{PopupPlacement, PopupRequest, clamp_origin, place_popup};
pub use split::{SplitAxis, SplitHandleSide, clamp_size, drag_height, drag_width};
pub use text_selection::{
    clamp_to_char_boundary, is_valid_selection, normalize_selection, resolve_selection,
    selection_or_cursor, should_highlight_selection, use_cursor_split,
};
pub use text_state::SharedTextState;
pub use toast_placement::toaster_bottom_offset;
pub use toggle::next_state;

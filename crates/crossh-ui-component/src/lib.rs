//! Crossh's reusable GPUI component layer.
//!
//! This crate follows the useful part of `gpui-component`'s model: components
//! are stateless `RenderOnce` values with builder APIs, while application state
//! and event handlers remain owned by feature views. The visual tokens come
//! from Crossh's own theme and no third-party component runtime is required.

pub mod avatar;
pub mod badge;
pub mod banner;
pub mod button;
pub mod context_menu;
pub mod count_badge;
pub mod filter_bar;
pub mod hint;
pub mod labeled_field;
pub mod layout;
pub mod list_pane;
pub mod list_state;
pub mod modal;
pub mod modal_field;
pub mod pane_toolbar;
pub mod panel;
pub mod select;
pub mod selectable_row;
pub mod split_resizer;
pub mod status_bar;
pub mod status_dot;
pub mod status_metric;
pub mod stepper;
pub mod tab;
pub mod text_input;
pub mod toast;
pub mod toggle;
pub mod tooltip;

/// Crossh theme tokens exposed to component consumers.
pub mod theme {
    pub use crossh_ui::theme::*;
}

pub use avatar::{Avatar, AvatarKind};
pub use badge::{Badge, BadgeTone};
pub use banner::{Banner, BannerLayout, BannerTone};
pub use button::{Button, ButtonSize, ButtonVariant};
pub use context_menu::{
    CONTEXT_MENU_WIDTH, ContextMenuState, MenuEntry, MenuItem, clamp_menu_position,
    estimate_menu_height, render_context_menu,
};
pub use count_badge::CountBadge;
pub use filter_bar::{filter_row, filter_text_color, filter_text_input};
pub use hint::Hint;
pub use labeled_field::{LABELED_FIELD_LABEL_WIDTH, labeled_field};
pub use layout::{h_flex, scroll_y};
pub use list_pane::{list_pane, pane_operation_error};
pub use list_state::{ListStatus, list_state_body};
pub use modal::ModalDialog;
pub use modal_field::{ModalField, SharedTextState};
pub use pane_toolbar::{PaneToolbar, pane_toolbar};
pub use panel::{
    PanelSide, RAIL_AVATAR_GAP, RAIL_AVATAR_SIZE, Rail, SidePanel,
    available_main_width as panel_available_main_width, clamp_panel_width, rail_avatar,
    rail_avatar_wide, rail_status_badge,
};
pub use select::{Select, SelectOption};
pub use selectable_row::selectable_row;
pub use split_resizer::{SplitAxis, SplitHandleSide, SplitResizer};
pub use status_bar::StatusBar;
pub use status_dot::StatusDot;
pub use status_metric::StatusMetric;
pub use stepper::Stepper;
pub use tab::{TabItem, TabStrip};
pub use text_input::TextInput;
pub use toast::{Toast, ToastTone, Toaster};
pub use toggle::ToggleSwitch;
pub use tooltip::Tooltip;

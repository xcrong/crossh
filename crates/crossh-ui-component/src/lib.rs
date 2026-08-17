//! Crossh's reusable GPUI component layer.
//!
//! This crate follows the useful part of `gpui-component`'s model: components
//! are stateless `RenderOnce` values with builder APIs, while application state
//! and event handlers remain owned by feature views. The visual tokens come
//! from Crossh's own theme and no third-party component runtime is required.

pub mod avatar;
pub mod badge;
pub mod button;
pub mod count_badge;
pub mod hint;
pub mod layout;
pub mod modal;
pub mod separator;
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
pub use button::{Button, ButtonSize, ButtonVariant};
pub use count_badge::CountBadge;
pub use hint::Hint;
pub use layout::{h_flex, scroll_y};
pub use modal::ModalDialog;
pub use separator::{Separator, SeparatorOrientation};
pub use split_resizer::{SplitHandleSide, SplitResizer};
pub use status_bar::StatusBar;
pub use status_dot::StatusDot;
pub use status_metric::StatusMetric;
pub use stepper::Stepper;
pub use tab::{TabItem, TabStrip};
pub use text_input::TextInput;
pub use toast::{Toast, ToastTone, Toaster};
pub use toggle::ToggleSwitch;
pub use tooltip::Tooltip;

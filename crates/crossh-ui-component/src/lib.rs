//! Crossh's reusable GPUI component layer.
//!
//! This crate follows the useful part of `gpui-component`'s model: components
//! are stateless `RenderOnce` values with builder APIs, while application state
//! and event handlers remain owned by feature views. The visual tokens come
//! from Crossh's own theme and no third-party component runtime is required.

pub mod avatar;
pub mod badge;
pub mod button;
pub mod layout;
pub mod separator;
pub mod status_dot;
pub mod stepper;
pub mod toggle;
pub mod tooltip;

/// Crossh theme tokens exposed to component consumers.
pub mod theme {
    pub use crossh_ui::theme::*;
}

pub mod prelude {
    pub use crate::avatar::{Avatar, AvatarKind};
    pub use crate::badge::{Badge, BadgeTone};
    pub use crate::button::{Button, ButtonSize, ButtonVariant};
    pub use crate::layout::{h_flex, v_flex};
    pub use crate::separator::{Separator, SeparatorOrientation};
    pub use crate::status_dot::StatusDot;
    pub use crate::stepper::Stepper;
    pub use crate::toggle::ToggleSwitch;
    pub use crate::tooltip::Tooltip;
}

pub use avatar::{Avatar, AvatarKind};
pub use badge::{Badge, BadgeTone};
pub use button::{Button, ButtonSize, ButtonVariant};
pub use layout::{h_flex, v_flex};
pub use separator::{Separator, SeparatorOrientation};
pub use status_dot::StatusDot;
pub use stepper::Stepper;
pub use toggle::ToggleSwitch;
pub use tooltip::Tooltip;

//! Terminal feature contracts and settings.
//!
//! Rendering and GPUI entity ownership stay in the application crate. This
//! crate only exposes terminal state that can cross the UI boundary.

pub mod events;
pub mod settings;
pub mod timestamps;

pub use events::{ConnState, TerminalEvent};
pub use settings::TerminalSettings;

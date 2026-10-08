#![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction)]
//! Behavior and infrastructure foundations for GPUI applications.
//!
//! Primitives deliberately avoid presentation styles. Layout, positioning,
//! colors, sizing, and motion belong to applications or the
//! `gpui-component` façade.

pub mod actions;
pub mod animation;
#[doc(hidden)]
pub mod async_util;
mod auto_scroll;
mod button;
pub mod component_traits;
mod element_ext;
mod event;
mod geometry;
mod global_state;
mod history;
pub mod input;
pub mod motion;
mod number_input;
mod progress;
mod resizable;
mod scrollable_mask;
mod scrollbar;
mod state_style;
mod styled;
pub mod text;
mod text_boundary;
mod text_selection;
pub mod theme;
pub mod theme_tokens;

pub use auto_scroll::AutoScroll;
pub use button::{Button, ButtonStyles};
pub use component_traits::FocusableExt;
pub use component_traits::{Disableable, Selectable};
pub use element_ext::ElementExt;
pub use event::{InteractiveElementExt, OngoingScrollExt};
pub use geometry::*;
pub use global_state::{DeferredPopover, GlobalState};
pub use history::{History, HistoryItem};
pub use input::{Editor, Input, InputBase, InputStyles, Textarea};
pub use motion::{
    Discrete, DiscreteError, Easing, EasingError, Interpolate, IterationCount, Keyframe,
    KeyframeError, Keyframes, LinearStop, MotionPhase, MotionReveal, MotionStatus, MotionTransform,
    MotionValue, PlaybackDirection, Presence, PresencePhase, PresenceSample, SignedDuration,
    Spring, SpringError, Stagger, StaggerOrigin, StepPosition, Timing, TimingSample, Transition,
    TransitionId, animate_keyframes, spring, transition, transition_with_status,
};
pub use number_input::{
    Decrement, Increment, NumberInput, NumberInputEvent, NumberInputText, NumberStep, StepAction,
    step_value,
};
pub use progress::{Progress, ProgressIndicator, ProgressTrack};
#[doc(hidden)]
pub use resizable::{PANEL_MIN_SIZE, resize_handle};
pub use resizable::{
    ResizablePanel, ResizablePanelEvent, ResizablePanelGroup, ResizableState, ResizeHandleContext,
    ResizeHandleRenderer, h_resizable, resizable_panel, v_resizable,
};
pub use scrollable_mask::ScrollableMask;
pub use scrollbar::{
    Scrollbar, ScrollbarAxis, ScrollbarEntrance, ScrollbarHandle, ScrollbarMode, ScrollbarMotion,
    ScrollbarStyles, ScrollbarThumbStyle, ScrollbarTrackStyle,
};
pub use state_style::StateStyle;
#[cfg(any(feature = "inspector", debug_assertions))]
pub use styled::styled_ext_reflection_methods;
pub use styled::{RoleOverride, StyledExt, box_shadow, h_flex, v_flex};
pub use text::{
    MarkdownExtensions, MarkdownNode, MarkdownPlugin, SelectionFormat, TableData, Text, TextView,
    TextViewDefaults, TextViewPlugin, TextViewState, TextViewStyle, html, markdown,
};
pub use text_selection::{
    TextSelection, TextSelectionContentKey, TextSelectionCoverage, TextSelectionEndpoint,
    TextSelectionEvent, TextSelectionHandle, TextSelectionLayer, TextSelectionProjection,
    TextSelectionRegistration, TextSelectionRun, TextSelectionScopeId, TextSelectionSnapshot,
    TextSelectionWindowPoints,
};
pub use theme::{ResizableTheme, ScrollbarTheme, Theme, ThemeAppearance};
pub use theme_tokens::{
    ColorTokens, RadiusTokens, SemanticThemeTokens, ShadowTokens, SpacingTokens, TextStyleToken,
    TypographyTokens,
};

use gpui::App;

/// Initializes global infrastructure owned by the base layer.
pub fn init(cx: &mut App) {
    let _ = Theme::global_mut(cx);
    GlobalState::init(cx);
    number_input::init(cx);
    input::init(cx);
    text::init(cx);
}

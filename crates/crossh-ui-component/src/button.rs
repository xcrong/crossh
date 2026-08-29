use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext, ClickEvent, ElementId, FontWeight, InteractiveElement,
    IntoElement, ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, Styled,
    Window, div, prelude::FluentBuilder, px, transparent_black,
};

use crate::layout::h_flex;
use crate::theme;
use crate::tooltip::Tooltip;

/// Visual intent for a button.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonVariant {
    #[default]
    Default,
    Primary,
    Secondary,
    Ghost,
    Danger,
    Link,
}

impl ButtonVariant {
    fn no_container(&self) -> bool {
        matches!(self, Self::Ghost | Self::Link)
    }

    fn style(self, selected: bool) -> ButtonStyle {
        if selected && !matches!(self, Self::Primary) {
            return ButtonStyle {
                background: theme::accent_soft(),
                foreground: theme::accent(),
                border: theme::accent(),
                hover_background: theme::accent_soft(),
                active_background: theme::accent_soft(),
            };
        }

        match self {
            Self::Default => ButtonStyle {
                background: theme::raised(),
                foreground: theme::text(),
                border: theme::border_strong(),
                hover_background: theme::surface(),
                active_background: theme::border(),
            },
            Self::Primary => ButtonStyle {
                background: theme::accent(),
                foreground: theme::canvas(),
                border: theme::accent(),
                hover_background: theme::accent_hover(),
                active_background: theme::accent_hover(),
            },
            Self::Secondary => ButtonStyle {
                background: theme::surface(),
                foreground: theme::text(),
                border: theme::border(),
                hover_background: theme::raised(),
                active_background: theme::border(),
            },
            Self::Ghost => ButtonStyle {
                background: transparent_black().into(),
                foreground: theme::muted_text(),
                border: transparent_black().into(),
                hover_background: theme::raised(),
                active_background: theme::border(),
            },
            Self::Danger => ButtonStyle {
                background: theme::danger(),
                foreground: theme::canvas(),
                border: theme::danger(),
                hover_background: theme::danger_hover(),
                active_background: theme::danger_hover(),
            },
            Self::Link => ButtonStyle {
                background: transparent_black().into(),
                foreground: theme::accent(),
                border: transparent_black().into(),
                hover_background: transparent_black().into(),
                active_background: transparent_black().into(),
            },
        }
    }
}

/// Supported component sizes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ButtonSize {
    Small,
    #[default]
    Medium,
    Large,
    Icon(gpui::Pixels),
}

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A stateless, focusable button component.
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: Option<SharedString>,
    icon: Option<AnyElement>,
    variant: ButtonVariant,
    size: ButtonSize,
    selected: bool,
    disabled: bool,
    loading: bool,
    full_width: bool,
    tooltip: Option<SharedString>,
    hover_background: Option<gpui::Rgba>,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: None,
            icon: None,
            variant: ButtonVariant::default(),
            size: ButtonSize::default(),
            selected: false,
            disabled: false,
            loading: false,
            full_width: false,
            tooltip: None,
            hover_background: None,
            on_click: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.icon = Some(icon.into_any_element());
        self
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    pub fn hover_background(mut self, color: gpui::Rgba) -> Self {
        self.hover_background = Some(color);
        self
    }

    pub fn on_click(
        mut self,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(on_click));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let button_style = self.variant.style(self.selected);
        let hover_background = self
            .hover_background
            .unwrap_or(button_style.hover_background);
        let focus = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let focused = focus.is_focused(window);
        let disabled = self.disabled || self.loading;
        let has_label = self.label.is_some();

        let mut button = h_flex()
            .id(self.id)
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .gap_1()
            .rounded(px(theme::RADIUS_SM))
            .border_1()
            .border_color(if disabled {
                theme::border()
            } else {
                button_style.border
            })
            .bg(if disabled {
                theme::surface()
            } else {
                button_style.background
            })
            .text_color(if disabled {
                theme::faint_text()
            } else {
                button_style.foreground
            })
            .when(!disabled, |this| {
                this.hover(|element| element.bg(hover_background))
                    .active(|element| element.bg(button_style.active_background))
            })
            .when(!disabled, |this| {
                this.track_focus(&focus.tab_stop(true).tab_index(0))
            })
            .when(focused && !disabled, |this| {
                this.border_color(theme::focus_ring())
            })
            .when(!disabled && self.variant.no_container(), |this| {
                this.border_0()
            })
            .when(!disabled && !self.variant.no_container(), |this| {
                this.cursor_pointer()
            })
            .when(!disabled && self.variant.no_container(), |this| {
                this.cursor_pointer()
            });

        button = match self.size {
            ButtonSize::Small => button.h(px(28.)).px_2().text_xs(),
            ButtonSize::Medium => button.h(px(32.)).px_3().text_xs(),
            ButtonSize::Large => button.h(px(36.)).px_4().text_sm(),
            ButtonSize::Icon(size) => button.w(size).h(size).px_0().text_xs(),
        };
        if self.full_width {
            button = button.w_full();
        }
        if !has_label {
            button = button.gap_0();
        }
        if let Some(on_click) = self.on_click.filter(|_| !disabled) {
            button = button.on_click(move |event, window, cx| on_click(event, window, cx));
        }
        if let Some(tooltip) = self.tooltip {
            button =
                button.tooltip(move |_window, cx| cx.new(|_| Tooltip::new(tooltip.clone())).into());
        }

        if self.loading {
            button = button.child(SharedString::from("..."));
        } else {
            if let Some(icon) = self.icon {
                button = button.child(icon);
            }
            if let Some(label) = self.label {
                button = button.child(
                    div()
                        .flex_shrink_0()
                        .text_color(if disabled {
                            theme::faint_text()
                        } else {
                            button_style.foreground
                        })
                        .font_weight(FontWeight::MEDIUM)
                        .child(label),
                );
            }
        }
        button
    }
}

#[derive(Clone, Copy)]
struct ButtonStyle {
    background: gpui::Rgba,
    foreground: gpui::Rgba,
    border: gpui::Rgba,
    hover_background: gpui::Rgba,
    active_background: gpui::Rgba,
}


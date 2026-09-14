use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext, ClickEvent, Context, ElementId, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, Pixels, Render, RenderOnce, Rgba, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
};

use crate::layout::h_flex;
use crate::status_dot::StatusDot;
use crate::theme;

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type MouseDownHandler = Rc<dyn Fn(&MouseDownEvent, &mut Window, &mut App)>;

/// Shared horizontal strip used by workspace and standalone feature views.
#[derive(IntoElement)]
pub struct TabStrip {
    id: ElementId,
    children: Vec<AnyElement>,
    scroll: Option<gpui::ScrollHandle>,
    border_bottom: bool,
}

impl TabStrip {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            children: Vec::new(),
            scroll: None,
            border_bottom: false,
        }
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }

    pub fn track_scroll(mut self, scroll: &gpui::ScrollHandle) -> Self {
        self.scroll = Some(scroll.clone());
        self
    }

    pub fn border_bottom(mut self) -> Self {
        self.border_bottom = true;
        self
    }
}

impl RenderOnce for TabStrip {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut strip = div()
            .id(self.id)
            .flex()
            .flex_row()
            .h(px(theme::TAB_HEIGHT))
            .flex_shrink_0()
            .px_2()
            .gap_1()
            .items_center()
            .bg(theme::surface())
            .when(self.border_bottom, |strip| {
                strip.border_b_1().border_color(theme::border())
            })
            .children(self.children);
        if let Some(scroll) = self.scroll {
            strip = strip.track_scroll(&scroll).restrict_scroll_to_axis();
            strip.style().overflow.x = Some(gpui::Overflow::Scroll);
        }
        strip
    }
}

/// 标签拖拽负载：`session_id` 即工作区会话 id（用 `u64` 避免组件反向依赖工作区类型），
/// `pinned` 区分固定/普通两组（只允许组内排序），`label` 仅用于拖拽预览。
#[derive(Clone)]
pub struct DragLocalTab {
    session_id: u64,
    pinned: bool,
    label: SharedString,
}

impl DragLocalTab {
    pub fn new(session_id: u64, pinned: bool, label: impl Into<SharedString>) -> Self {
        Self {
            session_id,
            pinned,
            label: label.into(),
        }
    }

    pub fn session_id(&self) -> u64 {
        self.session_id
    }
}

type DropTabHandler = Rc<dyn Fn(&DragLocalTab, &mut Window, &mut App)>;

impl Render for DragLocalTab {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded(px(theme::RADIUS_SM))
            .bg(theme::raised())
            .text_xs()
            .text_color(theme::text())
            .child(self.label.clone())
    }
}

/// A tab item with shared active, hover, label, and optional status-dot styling.
#[derive(IntoElement)]
pub struct TabItem {
    id: ElementId,
    label_id: ElementId,
    label: SharedString,
    active: bool,
    dot_color: Option<Rgba>,
    leading_icon: Option<AnyElement>,
    max_label_width: Pixels,
    children: Vec<AnyElement>,
    on_select: Option<ClickHandler>,
    on_mouse_down: Option<(MouseButton, MouseDownHandler)>,
    drag: Option<DragLocalTab>,
    on_drop_tab: Option<DropTabHandler>,
}

impl TabItem {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        let id = id.into();
        Self {
            label_id: ElementId::NamedChild(Arc::new(id.clone()), "label".into()),
            id,
            label: label.into(),
            active: false,
            dot_color: None,
            leading_icon: None,
            max_label_width: px(220.),
            children: Vec::new(),
            on_select: None,
            on_mouse_down: None,
            drag: None,
            on_drop_tab: None,
        }
    }

    pub fn label_id(mut self, id: impl Into<ElementId>) -> Self {
        self.label_id = id.into();
        self
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn dot_color(mut self, color: impl Into<Rgba>) -> Self {
        self.dot_color = Some(color.into());
        self
    }

    /// 渲染在状态点之后、标签名之前的前置图标（如固定标签的图钉）。
    pub fn leading_icon(mut self, icon: impl IntoElement) -> Self {
        self.leading_icon = Some(icon.into_any_element());
        self
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }

    pub fn on_select(
        mut self,
        on_select: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(on_select));
        self
    }

    pub fn on_mouse_down(
        mut self,
        button: MouseButton,
        on_mouse_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_mouse_down = Some((button, Rc::new(on_mouse_down)));
        self
    }
    /// 拖拽源：按住拖动即带起该标签，预览为标题小 pill。
    pub fn drag_tab(mut self, drag: DragLocalTab) -> Self {
        self.drag = Some(drag);
        self
    }

    /// 拖拽落点：其他标签落到本标签上时触发，调用方做同组排序。
    pub fn on_drop_tab(
        mut self,
        on_drop: impl Fn(&DragLocalTab, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_drop_tab = Some(Rc::new(on_drop));
        self
    }
}

impl RenderOnce for TabItem {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let TabItem {
            id,
            label_id,
            label,
            active,
            dot_color,
            leading_icon,
            max_label_width,
            children,
            on_select,
            on_mouse_down,
            drag,
            on_drop_tab,
        } = self;

        let label_view = h_flex()
            .flex_shrink_0()
            .gap_1()
            .px_2()
            .py_1()
            .cursor_pointer()
            .text_xs()
            .text_color(if active {
                theme::text()
            } else {
                theme::muted_text()
            })
            .hover(|style| style.text_color(theme::accent()))
            .when_some(dot_color, |view, color| view.child(StatusDot::new(color)))
            .when_some(leading_icon, |view, icon| view.child(icon))
            .child(
                div()
                    .min_w_0()
                    .max_w(max_label_width)
                    .truncate()
                    .child(label),
            );
        let mut label_view = label_view.id(label_id);
        if let Some(on_select) = on_select {
            label_view = label_view.on_click(move |event, window, cx| {
                on_select(event, window, cx);
            });
        }

        let mut tab = h_flex()
            .id(id)
            .flex_none()
            .gap_1()
            .h(px(28.))
            .px_1()
            .rounded(px(theme::RADIUS_SM))
            .when(active, |tab| {
                tab.bg(theme::accent_soft())
                    .border_b_2()
                    .border_color(theme::accent())
            })
            .when(!active, |tab| tab.hover(|style| style.bg(theme::raised())))
            .child(label_view)
            .children(children);
        if let Some((button, on_mouse_down)) = on_mouse_down {
            tab = tab.on_mouse_down(button, move |event, window, cx| {
                on_mouse_down(event, window, cx);
            });
        }
        if let Some(drag) = drag {
            let pinned = drag.pinned;
            tab = tab
                .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()))
                .drag_over(move |style, incoming: &DragLocalTab, _, _| {
                    if incoming.pinned == pinned {
                        style.bg(theme::accent_soft())
                    } else {
                        style
                    }
                });
        }
        if let Some(on_drop) = on_drop_tab {
            tab = tab.on_drop(move |drag: &DragLocalTab, window, cx| {
                on_drop(drag, window, cx);
            });
        }
        tab
    }
}

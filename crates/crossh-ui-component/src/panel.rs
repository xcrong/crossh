//! 侧栏与 Rail 的无状态 GPUI 原语：统一宽度换算、边框与拖拽手柄。
//!
//! - `SidePanel`: 可拖拽展开态容器，收敛 `w(px(clamp)) + bg/border + SplitResizer` 的重复骨架。
//! - `Rail`: 收起态窄栏容器，收敛 `w(rail) + bg/border + flex_col items_center` 的骨架。
//! - `rail_avatar`: 30px 头像项，复用 `sidebar.rs:314` 的视觉规范。
//!
//! 组件保持无状态：宽度与拖拽状态由调用方（`AppShell`）持有，组件仅负责 clamp 与渲染。

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext, ClickEvent, Div, ElementId, InteractiveElement, IntoElement,
    ParentElement, RenderOnce, SharedString, Stateful, StatefulInteractiveElement, Styled, Window,
    div, px,
};

use crossh_ui_base::handle_side_for;
pub use crossh_ui_base::{
    PanelSide, RAIL_AVATAR_GAP, RAIL_AVATAR_SIZE, available_main_width, clamp_panel_width,
};

use crate::avatar::Avatar;
use crate::split_resizer::SplitResizer;
use crate::theme;
use crate::tooltip::Tooltip;

/// 透明占位色，用于未选中态的边框/背景。
const TRANSPARENT: gpui::Rgba = gpui::Rgba {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.0,
};

/// 可拖拽的展开态侧边面板。
///
/// 渲染为 `relative flex_shrink_0 w(px(clamped)) h_full flex flex_col bg/border + SplitResizer`。
/// `side` 决定边框（Left→border_r、Right→border_l）与默认手柄方向（Left→Right、Right→Left）。
#[derive(IntoElement)]
pub struct SidePanel {
    id: ElementId,
    side: PanelSide,
    width: Rc<Cell<f32>>,
    dragging: Rc<Cell<bool>>,
    min_width: f32,
    max_width: f32,
    bg: gpui::Rgba,
    border_color: gpui::Rgba,
    line: bool,
    children: Vec<AnyElement>,
}

impl SidePanel {
    fn new(
        id: impl Into<ElementId>,
        side: PanelSide,
        width: Rc<Cell<f32>>,
        dragging: Rc<Cell<bool>>,
    ) -> Self {
        let (bg, border_color) = match side {
            PanelSide::Left => (theme::sidebar(), theme::border()),
            PanelSide::Right => (theme::surface(), theme::border()),
        };
        Self {
            id: id.into(),
            side,
            width,
            dragging,
            min_width: 0.0,
            max_width: f32::MAX,
            bg,
            border_color,
            line: false,
            children: Vec::new(),
        }
    }

    /// 左侧面板（侧边栏）：默认 `bg(sidebar) + border`，手柄贴右边缘。
    pub fn left(id: impl Into<ElementId>, width: Rc<Cell<f32>>, dragging: Rc<Cell<bool>>) -> Self {
        Self::new(id, PanelSide::Left, width, dragging)
    }
    /// 右侧面板：默认 `bg(surface) + border`，手柄贴左边缘。
    pub fn right(id: impl Into<ElementId>, width: Rc<Cell<f32>>, dragging: Rc<Cell<bool>>) -> Self {
        Self::new(id, PanelSide::Right, width, dragging)
    }

    pub fn min_width(mut self, min_width: f32) -> Self {
        self.min_width = min_width;
        self
    }

    pub fn max_width(mut self, max_width: f32) -> Self {
        self.max_width = max_width;
        self
    }

    pub fn bg(mut self, color: gpui::Rgba) -> Self {
        self.bg = color;
        self
    }

    pub fn border_color(mut self, color: gpui::Rgba) -> Self {
        self.border_color = color;
        self
    }

    /// 在手柄内渲染 1px 视觉线。
    pub fn line(mut self) -> Self {
        self.line = true;
        self
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }

    pub fn children(mut self, children: impl IntoIterator<Item = AnyElement>) -> Self {
        self.children.extend(children);
        self
    }
}

impl RenderOnce for SidePanel {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let resolved = clamp_panel_width(self.width.get(), self.min_width, self.max_width);
        let handle_side = handle_side_for(self.side);
        let mut resizer =
            SplitResizer::new(self.id.clone(), self.dragging.clone(), self.width.clone())
                .min_width(self.min_width)
                .max_width(self.max_width)
                .handle_side(handle_side);
        if self.line {
            resizer = resizer.line();
        }

        let mut outer = div()
            .relative()
            .flex_shrink_0()
            .w(px(resolved))
            .h_full()
            .flex()
            .flex_col()
            .bg(self.bg)
            .border_color(self.border_color);
        outer = match self.side {
            PanelSide::Left => outer.border_r_1(),
            PanelSide::Right => outer.border_l_1(),
        };
        outer.children(self.children).child(resizer)
    }
}

/// 收起态窄栏容器。
///
/// 渲染为 `id w(px(width)) h_full flex_none flex flex_col items_center bg/border`。
#[derive(IntoElement)]
pub struct Rail {
    id: ElementId,
    width: f32,
    bg: gpui::Rgba,
    border_color: gpui::Rgba,
    side: PanelSide,
    children: Vec<AnyElement>,
}

impl Rail {
    fn new(id: impl Into<ElementId>, width: f32, side: PanelSide) -> Self {
        let (bg, border_color) = match side {
            PanelSide::Left => (theme::sidebar(), theme::border()),
            PanelSide::Right => (theme::surface(), theme::border()),
        };
        Self {
            id: id.into(),
            width,
            bg,
            border_color,
            side,
            children: Vec::new(),
        }
    }

    /// 左侧 Rail（侧边栏 Rail）：`border_r_1`，默认 `bg(sidebar)`。
    pub fn left(id: impl Into<ElementId>, width: f32) -> Self {
        Self::new(id, width, PanelSide::Left)
    }

    /// 右侧 Rail：`border_l_1`，默认 `bg(surface)`。
    pub fn right(id: impl Into<ElementId>, width: f32) -> Self {
        Self::new(id, width, PanelSide::Right)
    }

    pub fn bg(mut self, color: gpui::Rgba) -> Self {
        self.bg = color;
        self
    }

    pub fn border_color(mut self, color: gpui::Rgba) -> Self {
        self.border_color = color;
        self
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }

    pub fn children(mut self, children: impl IntoIterator<Item = AnyElement>) -> Self {
        self.children.extend(children);
        self
    }
}

impl RenderOnce for Rail {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut outer = div()
            .id(self.id)
            .w(px(self.width))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .bg(self.bg)
            .border_color(self.border_color);
        outer = match self.side {
            PanelSide::Left => outer.border_r_1().py_2(),
            PanelSide::Right => outer.border_l_1(),
        };
        outer.children(self.children)
    }
}

/// Rail 头像项：`w(30) h(30) rounded 4 border_1`，选中态 `accent/accent_soft`，未选中透明、悬停 `surface`。
///
/// 与 `src/features/workspace/sidebar.rs:314 rail_avatar_button` 像素一致；
/// 调用方如需在右上角叠加 `StatusDot`，可在返回的 `Stateful<Div>` 上 `.child(rail_status_badge(...))`
/// 且容器已为 `relative` 的父级按需包裹。
pub fn rail_avatar(
    id: impl Into<ElementId>,
    avatar: Avatar,
    tooltip: impl Into<SharedString>,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    rail_avatar_inner(id, avatar, tooltip.into(), selected, false, on_click)
}

/// `rail_avatar` 的宽版变体：tooltip 使用 `Tooltip::wide()`，用于展示完整命令等长文本。
pub fn rail_avatar_wide(
    id: impl Into<ElementId>,
    avatar: Avatar,
    tooltip: impl Into<SharedString>,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    rail_avatar_inner(id, avatar, tooltip.into(), selected, true, on_click)
}

fn rail_avatar_inner(
    id: impl Into<ElementId>,
    avatar: Avatar,
    tooltip: SharedString,
    selected: bool,
    wide: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .relative()
        .w(px(RAIL_AVATAR_SIZE))
        .h(px(RAIL_AVATAR_SIZE))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .border_1()
        .border_color(if selected {
            theme::accent()
        } else {
            TRANSPARENT
        })
        .bg(if selected {
            theme::accent_soft()
        } else {
            TRANSPARENT
        })
        .hover(move |style| {
            style.bg(if selected {
                theme::accent_soft()
            } else {
                theme::surface()
            })
        })
        .tooltip(move |_window, cx| {
            let tip = Tooltip::new(tooltip.clone());
            let tip = if wide { tip.wide() } else { tip };
            cx.new(|_| tip).into()
        })
        .child(avatar)
        .on_click(on_click)
}

/// Rail 头像右上角的状态徽标容器：`absolute top 1 right 1` + `StatusDot size 7 border`。
///
/// 用于在 `rail_avatar` 外层叠加未读/后台任务状态。
pub fn rail_status_badge(color: gpui::Rgba, border: gpui::Rgba) -> impl IntoElement {
    use crate::status_dot::StatusDot;
    div()
        .absolute()
        .top(px(1.))
        .right(px(1.))
        .child(StatusDot::new(color).size(px(7.)).border(border))
}

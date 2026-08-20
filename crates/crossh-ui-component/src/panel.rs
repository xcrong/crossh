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
    ParentElement, Pixels, RenderOnce, SharedString, Stateful, StatefulInteractiveElement, Styled,
    Window, div, px,
};

use crate::avatar::Avatar;
use crate::split_resizer::{SplitHandleSide, SplitResizer};
use crate::theme;
use crate::tooltip::Tooltip;

/// 侧栏所在窗口边缘：决定边框与拖拽手柄方向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelSide {
    Left,
    Right,
}

/// 可选的面板度量聚合，便于调用方复用同一组常量。
#[derive(Clone, Copy, Debug)]
pub struct PanelMetrics {
    pub min_width: f32,
    pub max_width: f32,
    pub rail_width: f32,
}

/// Rail 头像项尺寸与间距的公开常量，契约 8 要求 pitch = 30 + 4 = 34。
pub const RAIL_AVATAR_SIZE: f32 = 30.0;
pub const RAIL_AVATAR_GAP: f32 = 4.0;
pub const RAIL_AVATAR_PITCH: f32 = RAIL_AVATAR_SIZE + RAIL_AVATAR_GAP;
/// 透明占位色，用于未选中态的边框/背景。
const TRANSPARENT: gpui::Rgba = gpui::Rgba {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.0,
};

/// 将原始宽度钳制到合法区间；NaN 回退到 `min`，保证不 panic 且不产生负可用宽度。
pub fn clamp_panel_width(value: f32, min_width: f32, max_width: f32) -> f32 {
    if value.is_nan() {
        return min_width;
    }
    value.clamp(min_width, max_width)
}

/// 工作区主区可用宽度：`max(viewport - sidebar - quick, 0)`，与 `shell.rs:1652` 契约一致。
pub fn available_main_width(
    viewport_width: Pixels,
    sidebar_width: f32,
    quick_width: f32,
) -> Pixels {
    px((viewport_width.as_f32() - sidebar_width - quick_width).max(0.0))
}

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
    handle_side: Option<SplitHandleSide>,
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
            handle_side: None,
            children: Vec::new(),
        }
    }

    /// 左侧面板（侧边栏）：默认 `bg(sidebar) + border`，手柄贴右边缘。
    pub fn left(id: impl Into<ElementId>, width: Rc<Cell<f32>>, dragging: Rc<Cell<bool>>) -> Self {
        Self::new(id, PanelSide::Left, width, dragging)
    }

    /// 右侧面板（Quick Commands）：默认 `bg(surface) + border`，手柄贴左边缘。
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

    /// 显式指定手柄贴左边缘；通常由 `PanelSide::Right` 自动推导，保留为对称 API。
    pub fn handle_left(self) -> Self {
        self.handle_side(SplitHandleSide::Left)
    }

    pub fn handle_side(mut self, side: SplitHandleSide) -> Self {
        self.handle_side = Some(side);
        self
    }

    /// 推导有效手柄方向：显式设置优先，否则 Left→Right、Right→Left。
    pub fn effective_handle_side(&self) -> SplitHandleSide {
        if let Some(side) = self.handle_side {
            side
        } else {
            match self.side {
                PanelSide::Left => SplitHandleSide::Right,
                PanelSide::Right => SplitHandleSide::Left,
            }
        }
    }

    /// 当前宽度经 clamp 后的实际渲染宽度；NaN 回退到 `min_width`。
    pub fn resolved_width(&self) -> f32 {
        clamp_panel_width(self.width.get(), self.min_width, self.max_width)
    }

    /// 展开态的 resolved 宽度；`expanded == false` 时按隐藏态计 0（不渲染面板时可用）。
    /// 保留 `expanded` 参数以满足 spec 对 `resolved_width(&self, expanded: bool)` 的形态描述。
    pub fn resolved_width_expanded(&self, expanded: bool) -> f32 {
        if expanded { self.resolved_width() } else { 0.0 }
    }

    /// 静态 clamp 辅助：不依赖实例，常用于 `available_main_width` 之前的宽度换算。
    pub fn clamp_width(value: f32, min_width: f32, max_width: f32) -> f32 {
        clamp_panel_width(value, min_width, max_width)
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
        let handle_side = self.effective_handle_side();
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

    /// 右侧 Rail（Quick Commands Rail）：`border_l_1`，默认 `bg(surface)`。
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

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::px;

    use super::{
        PanelSide, RAIL_AVATAR_GAP, RAIL_AVATAR_PITCH, RAIL_AVATAR_SIZE, Rail, SidePanel,
        available_main_width, clamp_panel_width,
    };
    use crate::split_resizer::SplitHandleSide;

    fn panel(side: PanelSide, value: f32, min: f32, max: f32) -> SidePanel {
        let width = Rc::new(Cell::new(value));
        let dragging = Rc::new(Cell::new(false));
        let p = match side {
            PanelSide::Left => SidePanel::left("test", width, dragging),
            PanelSide::Right => SidePanel::right("test", width, dragging),
        };
        p.min_width(min).max_width(max)
    }

    #[test]
    fn spec_20260820_side_panel_rail__resolved_width_clamps_min_max() {
        assert_eq!(
            panel(PanelSide::Left, 100., 216., 360.).resolved_width(),
            216.
        );
        assert_eq!(
            panel(PanelSide::Left, 500., 216., 360.).resolved_width(),
            360.
        );
        assert_eq!(
            panel(PanelSide::Left, 300., 216., 360.).resolved_width(),
            300.
        );
        assert_eq!(SidePanel::clamp_width(300., 216., 360.), 300.);
        assert_eq!(clamp_panel_width(100., 216., 360.), 216.);
    }

    #[test]
    fn spec_20260820_side_panel_rail__resolved_width_handles_nan_negative_and_overflow() {
        assert_eq!(clamp_panel_width(f32::NAN, 216., 360.), 216.);
        assert_eq!(clamp_panel_width(f32::INFINITY, 216., 360.), 360.);
        assert_eq!(clamp_panel_width(f32::NEG_INFINITY, 216., 360.), 216.);
        assert_eq!(clamp_panel_width(-100., 216., 360.), 216.);
        assert_eq!(clamp_panel_width(720., 216., 360.), 360.);
        // 超出 max 2 倍仍被 clamp
        assert_eq!(
            panel(PanelSide::Left, 720., 216., 360.).resolved_width(),
            360.
        );
        assert_eq!(
            panel(PanelSide::Right, f32::NAN, 240., 460.).resolved_width(),
            240.
        );
    }

    #[test]
    fn spec_20260820_side_panel_rail__rail_pitch_is_34() {
        assert_eq!(RAIL_AVATAR_SIZE, 30.0);
        assert_eq!(RAIL_AVATAR_GAP, 4.0);
        assert_eq!(RAIL_AVATAR_PITCH, 34.0);
        assert_eq!(RAIL_AVATAR_SIZE + RAIL_AVATAR_GAP, RAIL_AVATAR_PITCH);
    }

    #[test]
    fn spec_20260820_side_panel_rail__drag_handle_side_left_defaults_to_right() {
        let p = panel(PanelSide::Left, 250., 216., 360.);
        assert_eq!(p.effective_handle_side(), SplitHandleSide::Right);
        assert_eq!(p.side, PanelSide::Left);
    }

    #[test]
    fn spec_20260820_side_panel_rail__drag_handle_side_right_defaults_to_left() {
        let p = panel(PanelSide::Right, 300., 240., 460.);
        assert_eq!(p.effective_handle_side(), SplitHandleSide::Left);
        assert_eq!(p.side, PanelSide::Right);
    }

    #[test]
    fn spec_20260820_side_panel_rail__handle_left_overrides_side() {
        let width = Rc::new(Cell::new(300.));
        let dragging = Rc::new(Cell::new(false));
        let p = SidePanel::left("p", width, dragging)
            .min_width(216.)
            .max_width(360.)
            .handle_left();
        assert_eq!(p.effective_handle_side(), SplitHandleSide::Left);
        let width = Rc::new(Cell::new(300.));
        let dragging = Rc::new(Cell::new(false));
        let p2 = SidePanel::right("p2", width, dragging)
            .min_width(240.)
            .max_width(460.)
            .handle_side(SplitHandleSide::Right);
        assert_eq!(p2.effective_handle_side(), SplitHandleSide::Right);
    }

    #[test]
    fn spec_20260820_side_panel_rail__available_main_width_truncates_at_zero() {
        assert_eq!(available_main_width(px(700.), 216., 240.), px(244.));
        assert_eq!(available_main_width(px(700.), 44., 40.), px(616.));
        assert_eq!(available_main_width(px(400.), 216., 240.), px(0.));
        assert_eq!(available_main_width(px(300.), 200., 200.), px(0.));
    }

    #[test]
    fn spec_20260820_side_panel_rail__rail_side_maps_to_border() {
        let rail_left = Rail::left("rail-left", 44.);
        assert_eq!(rail_left.side, PanelSide::Left);
        let rail_right = Rail::right("rail-right", 40.);
        assert_eq!(rail_right.side, PanelSide::Right);
    }

    #[test]
    fn spec_20260820_side_panel_rail__resolved_width_expanded_flag() {
        let p = panel(PanelSide::Left, 300., 216., 360.);
        assert_eq!(p.resolved_width_expanded(true), 300.);
        assert_eq!(p.resolved_width_expanded(false), 0.0);
        let p2 = panel(PanelSide::Left, 50., 216., 360.);
        assert_eq!(p2.resolved_width_expanded(true), 216.);
        assert_eq!(p2.resolved_width_expanded(false), 0.0);
    }
}

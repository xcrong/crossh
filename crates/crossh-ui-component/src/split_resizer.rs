//! 面板拖拽调宽组件，纯渲染逻辑，无业务依赖。

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, Bounds, DispatchPhase, ElementId, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, RenderOnce, Styled,
    Window, canvas, div, prelude::FluentBuilder, px,
};

use crate::theme;

/// 拖拽手柄所在的面板边缘。
/// 两侧都是真实契约：`Right`（默认）覆盖终端分栏、Git 变更/历史面板与
/// 侧边栏；`Left` 由 `SidePanel::handle_side(SplitHandleSide::Left)` 显式
/// 指定，用于手柄贴左边缘的面板。禁止按“单侧够用”收紧为单一
/// 方向——新增侧向面板时直接选取变体即可，无需改动组件。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SplitHandleSide {
    /// 手柄贴在面板右侧，宽度等于指针向左边缘的距离。
    #[default]
    Right,
    /// 手柄贴在面板左侧，宽度等于指针向右边缘的距离。
    Left,
}

/// 从面板 bounds 与指针位置换算当前宽度。
fn drag_width(side: SplitHandleSide, bounds: &Bounds<Pixels>, pointer_x: Pixels) -> f32 {
    match side {
        SplitHandleSide::Left => bounds.right().as_f32() - pointer_x.as_f32(),
        SplitHandleSide::Right => pointer_x.as_f32() - bounds.origin.x.as_f32(),
    }
}

fn drag_height(bounds: &Bounds<Pixels>, pointer_y: Pixels) -> f32 {
    pointer_y.as_f32() - bounds.origin.y.as_f32()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SplitAxis {
    #[default]
    Horizontal,
    Vertical,
}

/// 面板拖拽调宽组件：一个透明的 bounds 采集 canvas 加一个绝对定位的拖拽手柄。
///
/// 组件是无状态的：`dragging`/`value` 单元格由调用方持有，clamp 范围由调用方传入，
/// 组件只负责把指针移动换算成尺寸写入 `value` 单元格（水平为宽度，垂直为高度）。
#[derive(IntoElement)]
pub struct SplitResizer {
    id: ElementId,
    dragging: Rc<Cell<bool>>,
    value: Rc<Cell<f32>>,
    min_size: f32,
    max_size: f32,
    handle_side: SplitHandleSide,
    axis: SplitAxis,
    line: bool,
}
impl SplitResizer {
    pub fn new(id: impl Into<ElementId>, dragging: Rc<Cell<bool>>, value: Rc<Cell<f32>>) -> Self {
        Self {
            id: id.into(),
            dragging,
            value,
            min_size: 0.0,
            max_size: f32::MAX,
            handle_side: SplitHandleSide::default(),
            axis: SplitAxis::Horizontal,
            line: false,
        }
    }

    pub fn min_size(mut self, min_size: f32) -> Self {
        self.min_size = min_size;
        self
    }

    pub fn max_size(mut self, max_size: f32) -> Self {
        self.max_size = max_size;
        self
    }

    pub fn min_width(self, v: f32) -> Self {
        self.min_size(v)
    }

    pub fn max_width(self, v: f32) -> Self {
        self.max_size(v)
    }

    /// 显式指定手柄方向（仅水平轴生效，垂直轴固定为 bottom，设置无效）。
    pub fn handle_side(mut self, handle_side: SplitHandleSide) -> Self {
        self.handle_side = handle_side;
        self
    }

    pub fn vertical(mut self) -> Self {
        self.axis = SplitAxis::Vertical;
        self
    }

    /// 在手柄内渲染 1px 视觉线（拖拽中/悬停高亮）。
    ///
    /// 生产消费者：终端分栏、侧边栏两处（渲染视觉分隔线）；Git 变更/历史面板不调用（无视觉线）。勿删。
    pub fn line(mut self) -> Self {
        self.line = true;
        self
    }
}
impl RenderOnce for SplitResizer {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let bounds: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
        let is_vertical = self.axis == SplitAxis::Vertical;
        let backing = canvas(
            {
                let bounds = bounds.clone();
                move |canvas_bounds, _window, _cx| bounds.set(Some(canvas_bounds))
            },
            {
                let bounds = bounds.clone();
                let value_cell = self.value.clone();
                let dragging = self.dragging.clone();
                move |_canvas_bounds, _state, window, _cx| {
                    window.on_mouse_event({
                        let bounds = bounds.clone();
                        let value_cell = value_cell.clone();
                        let dragging = dragging.clone();
                        move |event: &MouseMoveEvent, phase, window, _cx| {
                            if !matches!(phase, DispatchPhase::Bubble) {
                                return;
                            }
                            if !dragging.get() {
                                return;
                            }
                            let Some(bounds) = bounds.get() else {
                                return;
                            };
                            let value = if is_vertical {
                                drag_height(&bounds, event.position.y)
                            } else {
                                drag_width(self.handle_side, &bounds, event.position.x)
                            }
                            .clamp(self.min_size, self.max_size);
                            value_cell.set(value);
                            window.refresh();
                        }
                    });
                    window.on_mouse_event({
                        let dragging = dragging.clone();
                        move |_event: &MouseUpEvent, phase, window, _cx| {
                            if !matches!(phase, DispatchPhase::Bubble) {
                                return;
                            }
                            if dragging.replace(false) {
                                window.refresh();
                            }
                        }
                    });
                }
            },
        )
        .absolute()
        .size_full();

        let resizing = self.dragging.get();
        let handle = if is_vertical {
            div()
                .id(self.id)
                .absolute()
                .left_0()
                .bottom(px(-4.))
                .w_full()
                .h(px(8.))
                .cursor_row_resize()
                .when(self.line, |this| {
                    this.flex().items_center().justify_center().child(
                        div()
                            .h(px(1.))
                            .w_full()
                            .bg(if resizing {
                                theme::accent()
                            } else {
                                theme::border()
                            })
                            .hover(|style| style.bg(theme::accent())),
                    )
                })
                .on_mouse_down(MouseButton::Left, {
                    let dragging = self.dragging.clone();
                    move |_event: &MouseDownEvent, window, _cx| {
                        dragging.set(true);
                        window.refresh();
                    }
                })
        } else {
            div()
                .id(self.id)
                .absolute()
                .top_0()
                .when(self.handle_side == SplitHandleSide::Left, |this| {
                    this.left(px(-4.))
                })
                .when(self.handle_side == SplitHandleSide::Right, |this| {
                    this.right(px(-4.))
                })
                .w(px(8.))
                .h_full()
                .cursor_col_resize()
                .when(self.line, |this| {
                    this.flex().items_center().justify_center().child(
                        div()
                            .w(px(1.))
                            .h_full()
                            .bg(if resizing {
                                theme::accent()
                            } else {
                                theme::border()
                            })
                            .hover(|style| style.bg(theme::accent())),
                    )
                })
                .on_mouse_down(MouseButton::Left, {
                    let dragging = self.dragging.clone();
                    move |_event: &MouseDownEvent, window, _cx| {
                        dragging.set(true);
                        window.refresh();
                    }
                })
        };

        div().absolute().size_full().child(backing).child(handle)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, Point, Size, px};

    use super::{SplitHandleSide, drag_height, drag_width};

    #[test]
    fn right_handled_width_measures_from_left_edge() {
        let bounds = Bounds::new(Point::new(px(100.), px(20.)), Size::new(px(300.), px(40.)));
        assert_eq!(drag_width(SplitHandleSide::Right, &bounds, px(180.)), 80.);
        assert_eq!(drag_width(SplitHandleSide::Right, &bounds, px(100.)), 0.);
    }

    #[test]
    fn left_handled_width_measures_from_right_edge() {
        let bounds = Bounds::new(Point::new(px(100.), px(20.)), Size::new(px(300.), px(40.)));
        assert_eq!(drag_width(SplitHandleSide::Left, &bounds, px(180.)), 220.);
        assert_eq!(drag_width(SplitHandleSide::Left, &bounds, px(400.)), 0.);
    }

    #[test]
    fn vertical_drag_height_measures_from_top_edge() {
        let bounds = Bounds::new(Point::new(px(10.), px(20.)), Size::new(px(100.), px(300.)));
        assert_eq!(drag_height(&bounds, px(50.)), 30.);
        assert_eq!(drag_height(&bounds, px(20.)), 0.);
        assert_eq!(drag_height(&bounds, px(320.)), 300.);
    }

    #[test]
    fn vertical_drag_height_clamps_within_min_max() {
        // 模拟 SplitResizer 垂直钳制：value.clamp(min, max)
        let min = 80.0;
        let max = 600.0;
        assert_eq!(30.0_f32.clamp(min, max), 80.0);
        assert_eq!(700.0_f32.clamp(min, max), 600.0);
        assert_eq!(200.0_f32.clamp(min, max), 200.0);
    }
}

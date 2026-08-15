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

/// 面板拖拽调宽组件：一个透明的 bounds 采集 canvas 加一个绝对定位的拖拽手柄。
///
/// 组件是无状态的：`dragging`/`width` 单元格由调用方持有，clamp 范围由调用方传入，
/// 组件只负责把指针移动换算成宽度写入 `width` 单元格。
#[derive(IntoElement)]
pub struct SplitResizer {
    id: ElementId,
    dragging: Rc<Cell<bool>>,
    width: Rc<Cell<f32>>,
    min_width: f32,
    max_width: f32,
    handle_side: SplitHandleSide,
    line: bool,
}

impl SplitResizer {
    pub fn new(id: impl Into<ElementId>, dragging: Rc<Cell<bool>>, width: Rc<Cell<f32>>) -> Self {
        Self {
            id: id.into(),
            dragging,
            width,
            min_width: 0.0,
            max_width: f32::MAX,
            handle_side: SplitHandleSide::default(),
            line: false,
        }
    }

    pub fn min_width(mut self, min_width: f32) -> Self {
        self.min_width = min_width;
        self
    }

    pub fn max_width(mut self, max_width: f32) -> Self {
        self.max_width = max_width;
        self
    }

    pub fn handle_side(mut self, handle_side: SplitHandleSide) -> Self {
        self.handle_side = handle_side;
        self
    }

    /// 手柄贴左边缘的便捷写法。
    pub fn handle_left(self) -> Self {
        self.handle_side(SplitHandleSide::Left)
    }

    /// 在手柄内渲染 1px 视觉线（拖拽中/悬停高亮）。
    pub fn line(mut self) -> Self {
        self.line = true;
        self
    }
}

impl RenderOnce for SplitResizer {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let bounds: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
        let backing = canvas(
            {
                let bounds = bounds.clone();
                move |canvas_bounds, _window, _cx| bounds.set(Some(canvas_bounds))
            },
            {
                let bounds = bounds.clone();
                let width_cell = self.width.clone();
                let dragging = self.dragging.clone();
                move |_canvas_bounds, _state, window, _cx| {
                    window.on_mouse_event({
                        let bounds = bounds.clone();
                        let width_cell = width_cell.clone();
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
                            let width = drag_width(self.handle_side, &bounds, event.position.x)
                                .clamp(self.min_width, self.max_width);
                            width_cell.set(width);
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
        let handle = div()
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
            });

        div().absolute().size_full().child(backing).child(handle)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{Bounds, ElementId, Point, Size, px};

    use super::{SplitHandleSide, SplitResizer, drag_width};

    fn state() -> (Rc<Cell<bool>>, Rc<Cell<f32>>) {
        (Rc::new(Cell::new(false)), Rc::new(Cell::new(100.)))
    }

    #[test]
    fn builder_defaults_to_right_side_without_line_or_range() {
        let (dragging, width) = state();
        let resizer = SplitResizer::new("panel-resize", dragging, width);
        assert_eq!(resizer.id, ElementId::Name("panel-resize".into()));
        assert_eq!(resizer.handle_side, SplitHandleSide::Right);
        assert!(!resizer.line);
        assert_eq!(resizer.min_width, 0.0);
        assert_eq!(resizer.max_width, f32::MAX);
        assert_eq!(resizer.width.get(), 100.);
        assert!(!resizer.dragging.get());
    }

    #[test]
    fn builder_sets_range_side_and_line() {
        let (dragging, width) = state();
        let resizer = SplitResizer::new("panel-resize", dragging, width)
            .min_width(10.)
            .max_width(420.)
            .handle_left()
            .line();
        assert_eq!(resizer.handle_side, SplitHandleSide::Left);
        assert!(resizer.line);
        assert_eq!(resizer.min_width, 10.);
        assert_eq!(resizer.max_width, 420.);
    }

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
}

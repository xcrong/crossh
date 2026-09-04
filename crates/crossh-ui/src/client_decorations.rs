//! Linux CSD 窗口外框：圆角 + 1px 边框 + 阴影 + 边缘 resize。
//!
//! 镜像 Zed `workspace::client_side_decorations` 的精简实现。服务端装饰
//! （`Decorations::Server`）下直通；客户端装饰（GNOME Wayland、无 SSD
//! 合成器）下绘制可见的窗口边缘，否则窗口会与深色桌面融为一体，看起来
//! “没有边框”。必须在 `Render::render` 中调用（内部写 `client_inset`）。

use gpui::InteractiveElement as _;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Bounds, CursorStyle, Decorations, Div, HitboxBehavior, Hsla, IntoElement, MouseButton,
    ParentElement, Pixels, ResizeEdge, Stateful, Styled, Tiling, Window, canvas, div, point, px,
    transparent_black,
};

use crate::theme;

/// CSD 阴影厚度，同时是边缘 resize 热区。
pub const CLIENT_SIDE_DECORATION_SHADOW: Pixels = px(10.0);
/// CSD 外框圆角半径。
pub const CLIENT_SIDE_DECORATION_ROUNDING: Pixels = px(10.0);

const BORDER_SIZE: Pixels = px(1.0);

/// 用客户端装饰包裹窗口根元素。服务端装饰下原样返回。
pub fn client_side_decorations(
    element: impl IntoElement,
    window: &mut Window,
    _cx: &mut App,
) -> Stateful<Div> {
    let decorations = window.window_decorations();
    let is_resizable = window.is_resizable();
    let tiling = match decorations {
        Decorations::Server => Tiling::default(),
        Decorations::Client { tiling } => tiling,
    };

    match decorations {
        Decorations::Client { .. } => window.set_client_inset(CLIENT_SIDE_DECORATION_SHADOW),
        Decorations::Server => window.set_client_inset(px(0.0)),
    }

    div()
        .id("window-backdrop")
        .bg(transparent_black())
        .map(|backdrop| match decorations {
            Decorations::Server => backdrop,
            Decorations::Client { .. } => backdrop
                .when(!tiling.top && !tiling.left, |this| {
                    this.rounded_tl(CLIENT_SIDE_DECORATION_ROUNDING)
                })
                .when(!tiling.top && !tiling.right, |this| {
                    this.rounded_tr(CLIENT_SIDE_DECORATION_ROUNDING)
                })
                .when(!tiling.bottom && !tiling.left, |this| {
                    this.rounded_bl(CLIENT_SIDE_DECORATION_ROUNDING)
                })
                .when(!tiling.bottom && !tiling.right, |this| {
                    this.rounded_br(CLIENT_SIDE_DECORATION_ROUNDING)
                })
                .when(!tiling.top, |this| this.pt(CLIENT_SIDE_DECORATION_SHADOW))
                .when(!tiling.bottom, |this| {
                    this.pb(CLIENT_SIDE_DECORATION_SHADOW)
                })
                .when(!tiling.left, |this| this.pl(CLIENT_SIDE_DECORATION_SHADOW))
                .when(!tiling.right, |this| this.pr(CLIENT_SIDE_DECORATION_SHADOW))
                .when(is_resizable, |this| {
                    this.on_mouse_move(|_event, window, _cx| window.refresh())
                        .on_mouse_down(MouseButton::Left, move |event, window, _cx| {
                            let size = window.window_bounds().get_bounds().size;
                            let Some(edge) = resize_edge(
                                event.position,
                                CLIENT_SIDE_DECORATION_SHADOW,
                                size,
                                tiling,
                            ) else {
                                return;
                            };
                            window.start_window_resize(edge);
                        })
                }),
        })
        .size_full()
        .child(
            div()
                .cursor(CursorStyle::Arrow)
                .map(|frame| match decorations {
                    Decorations::Server => frame,
                    Decorations::Client { .. } => frame
                        .border_color(theme::border())
                        .when(!tiling.top && !tiling.left, |this| {
                            this.rounded_tl(CLIENT_SIDE_DECORATION_ROUNDING)
                        })
                        .when(!tiling.top && !tiling.right, |this| {
                            this.rounded_tr(CLIENT_SIDE_DECORATION_ROUNDING)
                        })
                        .when(!tiling.bottom && !tiling.left, |this| {
                            this.rounded_bl(CLIENT_SIDE_DECORATION_ROUNDING)
                        })
                        .when(!tiling.bottom && !tiling.right, |this| {
                            this.rounded_br(CLIENT_SIDE_DECORATION_ROUNDING)
                        })
                        .when(!tiling.top, |this| this.border_t(BORDER_SIZE))
                        .when(!tiling.bottom, |this| this.border_b(BORDER_SIZE))
                        .when(!tiling.left, |this| this.border_l(BORDER_SIZE))
                        .when(!tiling.right, |this| this.border_r(BORDER_SIZE))
                        .when(!tiling.is_tiled(), |this| {
                            this.shadow(vec![
                                gpui::BoxShadow::new(
                                    px(0.),
                                    px(0.),
                                    Hsla {
                                        h: 0.,
                                        s: 0.,
                                        l: 0.,
                                        a: 0.4,
                                    },
                                )
                                .blur_radius(CLIENT_SIDE_DECORATION_SHADOW / 2.),
                            ])
                        })
                        // 方形内容（状态栏缺失的页面底部等）不露出圆角外。
                        .overflow_hidden(),
                })
                .on_mouse_move(|_event, _, cx| {
                    cx.stop_propagation();
                })
                .size_full()
                .child(element),
        )
        .map(|backdrop| match decorations {
            Decorations::Client { .. } if is_resizable => backdrop.child(
                canvas(
                    |_bounds, window, _cx| {
                        window.insert_hitbox(
                            Bounds::new(
                                point(px(0.0), px(0.0)),
                                window.window_bounds().get_bounds().size,
                            ),
                            HitboxBehavior::Normal,
                        )
                    },
                    move |_bounds, hitbox, window, _cx| {
                        let mouse = window.mouse_position();
                        let size = window.window_bounds().get_bounds().size;
                        let Some(edge) =
                            resize_edge(mouse, CLIENT_SIDE_DECORATION_SHADOW, size, tiling)
                        else {
                            return;
                        };
                        window.set_cursor_style(
                            match edge {
                                ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
                                ResizeEdge::Left | ResizeEdge::Right => {
                                    CursorStyle::ResizeLeftRight
                                }
                                ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
                                    CursorStyle::ResizeUpLeftDownRight
                                }
                                ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
                                    CursorStyle::ResizeUpRightDownLeft
                                }
                            },
                            &hitbox,
                        );
                    },
                )
                .size_full()
                .absolute(),
            ),
            _ => backdrop,
        })
}

fn resize_edge(
    pos: gpui::Point<Pixels>,
    shadow_size: Pixels,
    window_size: gpui::Size<Pixels>,
    tiling: Tiling,
) -> Option<ResizeEdge> {
    let bounds = Bounds::new(gpui::Point::default(), window_size).inset(shadow_size * 1.5);
    if bounds.contains(&pos) {
        return None;
    }

    let corner_size = gpui::size(shadow_size * 1.5, shadow_size * 1.5);
    let top_left_bounds = Bounds::new(gpui::Point::new(px(0.), px(0.)), corner_size);
    if !tiling.top && top_left_bounds.contains(&pos) {
        return Some(ResizeEdge::TopLeft);
    }

    let top_right_bounds = Bounds::new(
        gpui::Point::new(window_size.width - corner_size.width, px(0.)),
        corner_size,
    );
    if !tiling.top && top_right_bounds.contains(&pos) {
        return Some(ResizeEdge::TopRight);
    }

    let bottom_left_bounds = Bounds::new(
        gpui::Point::new(px(0.), window_size.height - corner_size.height),
        corner_size,
    );
    if !tiling.bottom && bottom_left_bounds.contains(&pos) {
        return Some(ResizeEdge::BottomLeft);
    }

    let bottom_right_bounds = Bounds::new(
        gpui::Point::new(
            window_size.width - corner_size.width,
            window_size.height - corner_size.height,
        ),
        corner_size,
    );
    if !tiling.bottom && bottom_right_bounds.contains(&pos) {
        return Some(ResizeEdge::BottomRight);
    }

    if !tiling.top && pos.y < shadow_size {
        Some(ResizeEdge::Top)
    } else if !tiling.bottom && pos.y > window_size.height - shadow_size {
        Some(ResizeEdge::Bottom)
    } else if !tiling.left && pos.x < shadow_size {
        Some(ResizeEdge::Left)
    } else if !tiling.right && pos.x > window_size.width - shadow_size {
        Some(ResizeEdge::Right)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window_size() -> gpui::Size<Pixels> {
        gpui::size(px(800.), px(600.))
    }

    #[test]
    fn center_hit_is_not_a_resize_edge() {
        assert_eq!(
            resize_edge(
                point(px(400.), px(300.)),
                CLIENT_SIDE_DECORATION_SHADOW,
                window_size(),
                Tiling::default(),
            ),
            None,
        );
    }

    #[test]
    fn each_side_maps_to_its_edge() {
        let shadow = CLIENT_SIDE_DECORATION_SHADOW;
        let size = window_size();
        let tiling = Tiling::default();
        assert_eq!(
            resize_edge(point(px(400.), px(5.)), shadow, size, tiling),
            Some(ResizeEdge::Top),
        );
        assert_eq!(
            resize_edge(point(px(400.), px(595.)), shadow, size, tiling),
            Some(ResizeEdge::Bottom),
        );
        assert_eq!(
            resize_edge(point(px(5.), px(300.)), shadow, size, tiling),
            Some(ResizeEdge::Left),
        );
        assert_eq!(
            resize_edge(point(px(795.), px(300.)), shadow, size, tiling),
            Some(ResizeEdge::Right),
        );
        assert_eq!(
            resize_edge(point(px(795.), px(595.)), shadow, size, tiling),
            Some(ResizeEdge::BottomRight),
        );
    }

    #[test]
    fn tiled_edges_are_not_resizable() {
        let shadow = CLIENT_SIDE_DECORATION_SHADOW;
        let size = window_size();
        let tiling = Tiling {
            top: true,
            ..Tiling::default()
        };
        assert_eq!(
            resize_edge(point(px(400.), px(5.)), shadow, size, tiling),
            None,
        );
        assert_eq!(
            resize_edge(point(px(5.), px(300.)), shadow, size, tiling),
            Some(ResizeEdge::Left),
        );
    }
}

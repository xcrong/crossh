use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Bounds, Context, DispatchPhase, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, SharedString, Styled, Window, canvas, div,
    px,
};

use crossh_ui::{icons, theme};
use crossh_ui_component::{Button, ButtonSize, ButtonVariant};

use crate::features::workspace::shell::AppShell;
use crate::features::workspace::shell::scratch::{
    SCRATCH_MAX_HEIGHT, SCRATCH_MIN_HEIGHT, clamp_scratch_height,
};
use crate::shared::i18n;

/// 渲染 Scratch 抽屉：顶部可拖拽边框 + 标题栏 + 终端内容。
pub(crate) fn render_scratch_panel(
    shell: &mut AppShell,
    _window: &Window,
    cx: &mut Context<AppShell>,
) -> gpui::AnyElement {
    let Some(terminal) = shell.scratch_terminal.clone() else {
        return div().into_any_element();
    };
    let height = shell.scratch_height_value();
    let height_cell = shell.scratch_height.clone();
    let dragging = shell.scratch_dragging.clone();

    let header = div()
        .h(px(28.))
        .flex_shrink_0()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .px_2()
        .bg(theme::surface())
        .border_b_1()
        .border_color(theme::border())
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .text_color(theme::muted_text())
                .child(icons::icon(icons::IconName::Terminal, 12.).text_color(theme::accent()))
                .child(SharedString::from(i18n::text("scratch.title"))),
        )
        .child(
            div().flex().items_center().gap_1().child(
                Button::new("scratch-close")
                    .size(ButtonSize::Icon(px(22.)))
                    .variant(ButtonVariant::Ghost)
                    .icon(icons::icon(icons::IconName::X, 12.).text_color(theme::muted_text()))
                    .tooltip(i18n::text("scratch.close"))
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.hide_scratch_terminal(cx);
                    })),
            ),
        );

    let resizer = render_vertical_resizer(height_cell, dragging);

    // resizer 置于最后绘制，确保顶部 8px 拖拽区在标题栏之上可命中；
    // 原先作为首个子元素时，header（h28）会以绘制顺序覆盖 0-4px 重叠区，
    // 仅余 -4-0 的 4px 可抓取，体感即“拖不动”。
    div()
        .id("scratch-panel")
        .w_full()
        .h(px(height))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .bg(theme::canvas())
        .border_t_1()
        .border_color(theme::border_strong())
        .relative()
        .child(header)
        .child(
            div()
                .flex_1()
                .min_h_0()
                .p(px(4.))
                .child(terminal.into_any_element()),
        )
        .child(resizer)
        .into_any_element()
}

fn render_vertical_resizer(height: Rc<Cell<f32>>, dragging: Rc<Cell<bool>>) -> impl IntoElement {
    // 复用 SplitResizer 的经验证模式，但针对底部抽屉语义：
    // - 背景 canvas 覆盖整个抽屉面板（absolute size_full），bounds.bottom() 为面板底边（固定贴底）
    // - 拖拽时高度 = bottom - pointer_y，直观且无需 start_y/start_height 中间态
    // - 失败根因是此前 backing 仅 h=8 的细条且分配全新 Rc/start_y 每帧，导致旧 handler 的
    //   start_y 仍为 None，叠加 header 遮挡使命中区仅 4px，体感“拖不动”
    let bounds: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
    let backing = canvas(
        {
            let bounds = bounds.clone();
            move |canvas_bounds, _window, _cx| bounds.set(Some(canvas_bounds))
        },
        {
            let bounds = bounds.clone();
            let height = height.clone();
            let dragging = dragging.clone();
            move |_canvas_bounds, _state, window, _cx| {
                window.on_mouse_event({
                    let bounds = bounds.clone();
                    let height = height.clone();
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
                        let new_height = (bounds.bottom().as_f32() - event.position.y.as_f32())
                            .clamp(SCRATCH_MIN_HEIGHT, SCRATCH_MAX_HEIGHT);
                        height.set(new_height);
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

    let resizing = dragging.get();
    let handle = div()
        .id("scratch-resizer")
        .absolute()
        .top(px(-4.))
        .left_0()
        .w_full()
        .h(px(8.))
        .cursor_row_resize()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(40.))
                .h(px(3.))
                .rounded(px(2.))
                .bg(if resizing {
                    theme::accent()
                } else {
                    theme::border()
                })
                .hover(|s| s.bg(theme::accent())),
        )
        .on_mouse_down(MouseButton::Left, {
            let dragging = dragging.clone();
            move |_event: &MouseDownEvent, window, _cx| {
                dragging.set(true);
                window.refresh();
            }
        });

    let _ = clamp_scratch_height(height.get());
    // 外层覆盖整个抽屉面板以提供稳定 bounds；视觉手柄仅在顶部 8px
    div().absolute().size_full().child(backing).child(handle)
}

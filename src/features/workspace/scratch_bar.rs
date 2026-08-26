use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Context, DispatchPhase, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, SharedString, Styled, Window, canvas, div, px,
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
        .child(resizer)
        .child(header)
        .child(
            div()
                .flex_1()
                .min_h_0()
                .p(px(4.))
                .child(terminal.into_any_element()),
        )
        .into_any_element()
}

fn render_vertical_resizer(height: Rc<Cell<f32>>, dragging: Rc<Cell<bool>>) -> impl IntoElement {
    let start_y: Rc<Cell<Option<f32>>> = Rc::new(Cell::new(None));
    let start_height: Rc<Cell<f32>> = Rc::new(Cell::new(0.));
    let backing = canvas(|_bounds, _window, _cx| {}, {
        let height = height.clone();
        let dragging = dragging.clone();
        let start_y = start_y.clone();
        let start_height = start_height.clone();
        move |_canvas_bounds, _state, window, _cx| {
            window.on_mouse_event({
                let height = height.clone();
                let dragging = dragging.clone();
                let start_y = start_y.clone();
                let start_height = start_height.clone();
                move |event: &MouseMoveEvent, phase, window, _cx| {
                    if !matches!(phase, DispatchPhase::Bubble) {
                        return;
                    }
                    if !dragging.get() {
                        return;
                    }
                    let Some(sy) = start_y.get() else {
                        return;
                    };
                    let delta = sy - event.position.y.as_f32();
                    let new_height =
                        (start_height.get() + delta).clamp(SCRATCH_MIN_HEIGHT, SCRATCH_MAX_HEIGHT);
                    height.set(new_height);
                    window.refresh();
                }
            });
            window.on_mouse_event({
                let dragging = dragging.clone();
                let start_y = start_y.clone();
                move |_event: &MouseUpEvent, phase, window, _cx| {
                    if !matches!(phase, DispatchPhase::Bubble) {
                        return;
                    }
                    if dragging.replace(false) {
                        start_y.set(None);
                        window.refresh();
                    }
                }
            });
        }
    })
    .absolute()
    .top_0()
    .left_0()
    .w_full()
    .h(px(8.));

    let handle = {
        let start_y_handle = start_y.clone();
        let start_height_handle = start_height.clone();
        let height_handle = height.clone();
        div()
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
                    .bg(theme::border())
                    .hover(|s| s.bg(theme::accent())),
            )
            .on_mouse_down(
                MouseButton::Left,
                move |event: &MouseDownEvent, window, _cx| {
                    dragging.set(true);
                    start_y_handle.set(Some(event.position.y.as_f32()));
                    start_height_handle.set(height_handle.get());
                    window.refresh();
                },
            )
    };

    // clamp 辅助在逻辑层已测试，此处仅渲染
    let _ = clamp_scratch_height(height.get());
    div()
        .absolute()
        .top_0()
        .left_0()
        .w_full()
        .h(px(8.))
        .child(backing)
        .child(handle)
}

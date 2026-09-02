use gpui::{
    Context, InteractiveElement, IntoElement, MouseButton, ParentElement, SharedString, Styled,
    Window, div, px,
};

use crate::features::workspace::AppShell;
use crate::features::workspace::shell::scratch::{SCRATCH_MIN_HEIGHT, clamp_scratch_height};
use crate::shared::i18n;
use crossh_ui::{icons, theme};
use crossh_ui_component::{Button, ButtonSize, ButtonVariant, SplitResizer};

/// 渲染 Scratch 悬浮层：复用 Command Palette 的 `absolute inset_0 + scrim + 居中卡片` 机制。
/// 卡片复用 `SplitResizer(vertical)` 在底部可拖拽调高，尺寸较 Palette 更大以便执行任务。
pub(crate) fn render_scratch_panel(
    shell: &mut AppShell,
    window: &Window,
    cx: &mut Context<AppShell>,
) -> gpui::AnyElement {
    let Some(term) = shell.scratch_terminal.clone() else {
        return div().into_any_element();
    };
    let height = shell.scratch_height_value();
    let height_cell = shell.scratch_height.clone();
    let dragging = shell.scratch_dragging.clone();

    let viewport = window.viewport_size();
    // 窗口自适应：无固定 MAX，默认 80% 视口，拖拽上下界随视口动态计算
    let max_h = (viewport.height.as_f32() - 96.0).max(SCRATCH_MIN_HEIGHT);
    let min_h = SCRATCH_MIN_HEIGHT;
    let raw = shell.scratch_height.get();
    let is_default = raw <= 0.0;
    let display_height = if is_default {
        (viewport.height.as_f32() * 0.8).clamp(min_h, max_h)
    } else {
        height.clamp(min_h, max_h)
    };
    let card_width = viewport.width.as_f32() * 0.8;
    let card_height = display_height;
    let _ = clamp_scratch_height(height);

    let header = div()
        .h(px(36.))
        .flex_shrink_0()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .px_3()
        .bg(theme::surface())
        .border_b_1()
        .border_color(theme::border())
        .rounded_t(px(theme::RADIUS_MD))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .text_color(theme::muted_text())
                .child(icons::icon(icons::IconName::Terminal, 14.).text_color(theme::accent()))
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

    // 卡片：默认 80% 视口，高度可拖拽
    let card = div()
        .id("scratch-card")
        .w(px(card_width))
        .h(px(card_height))
        .relative()
        .flex()
        .flex_col()
        .bg(theme::surface())
        .border_1()
        .border_color(theme::border_strong())
        .rounded(px(theme::RADIUS_MD))
        .shadow_lg()
        .overflow_hidden()
        .child(header)
        .child(
            div()
                .flex_1()
                .min_h_0()
                .p(px(6.))
                .bg(theme::canvas())
                .child(term.into_any_element()),
        )
        .child(
            SplitResizer::new("scratch-resizer", dragging.clone(), height_cell.clone())
                .vertical()
                .min_size(min_h)
                .max_size(max_h)
                .line(),
        );

    div()
        .id("scratch-scrim")
        .absolute()
        .inset_0()
        .flex()
        .items_start()
        .justify_center()
        .pt(px(72.))
        .bg(theme::scrim())
        .occlude()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _ev, _window, cx| {
                this.hide_scratch_terminal(cx);
            }),
        )
        .child(
            div()
                .id("scratch-card-wrapper")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _ev, _window, cx| {
                        cx.stop_propagation();
                    }),
                )
                .child(card),
        )
        .into_any_element()
}

//！ Linux 客户端装饰（CSD）通用标题栏。
//!
//! 主窗口与所有次级窗口（Git / Note / Settings / 独立二进制）共享同一套 CSD
//! 实现：在 `Decorations::Client` 时自绘标题栏，提供拖动、`gtk-decoration-layout`
//! 控制按钮、双击缩放与窗口菜单；在 `Server` 装饰下返回 `None` 由合成器绘制。

#[cfg(target_os = "linux")]
mod imp {
    use gpui::prelude::FluentBuilder as _;
    use gpui::{
        App, Decorations, IntoElement, ParentElement, SharedString, Styled, Window, WindowButton,
        WindowButtonLayout, WindowControlArea, div, px,
    };
    use gpui::{InteractiveElement as _, StatefulInteractiveElement as _};

    use crate::{icons, theme};

    pub fn render_linux_titlebar(
        window: &mut Window,
        cx: &App,
        title: SharedString,
    ) -> Option<gpui::AnyElement> {
        if !matches!(window.window_decorations(), Decorations::Client { .. }) {
            return None;
        }

        let decorations = window.window_decorations();
        let is_maximized = window.is_maximized();
        let window_controls = window.window_controls();
        let button_layout = cx
            .button_layout()
            .unwrap_or_else(WindowButtonLayout::linux_default);
        let left_controls = render_window_controls(
            &button_layout.left,
            is_maximized,
            window_controls,
            window,
            cx,
        );
        let right_controls = render_window_controls(
            &button_layout.right,
            is_maximized,
            window_controls,
            window,
            cx,
        );

        let height = px(36.);

        let titlebar = div()
            .id("linux-titlebar")
            .window_control_area(WindowControlArea::Drag)
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(height)
            .w_full()
            .px(px(8.))
            .gap(px(8.))
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .map(|el| match decorations {
                Decorations::Client { tiling } => el
                    .when(!tiling.top && !tiling.right, |e| e.rounded_tr(px(8.)))
                    .when(!tiling.top && !tiling.left, |e| e.rounded_tl(px(8.))),
                _ => el,
            })
            .on_mouse_down(gpui::MouseButton::Left, move |_, window: &mut Window, _| {
                window.start_window_move();
            })
            .on_click({
                let is_resizable = window.is_resizable();
                let maximize_supported = window_controls.maximize;
                move |event, window, _| {
                    if event.click_count() == 2 && maximize_supported && is_resizable {
                        window.zoom_window();
                    }
                }
            })
            .on_mouse_down(gpui::MouseButton::Right, move |event, window, _| {
                if window_controls.window_menu {
                    window.show_window_menu(event.position);
                }
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .children(left_controls)
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme::text())
                            .child(title),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.))
                    .children(right_controls),
            );

        Some(titlebar.into_any_element())
    }

    fn render_window_controls(
        buttons: &[Option<WindowButton>; 3],
        is_maximized: bool,
        supported: gpui::WindowControls,
        _window: &Window,
        cx: &App,
    ) -> Option<gpui::AnyElement> {
        let elements: Vec<gpui::AnyElement> = buttons
            .iter()
            .filter_map(|b| *b)
            .filter(|button| match button {
                WindowButton::Minimize => supported.minimize,
                WindowButton::Maximize => supported.maximize,
                WindowButton::Close => true,
            })
            .map(|button| render_single_button(button, is_maximized, cx).into_any_element())
            .collect();

        if elements.is_empty() {
            None
        } else {
            Some(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .children(elements)
                    .into_any_element(),
            )
        }
    }

    fn render_single_button(
        button: WindowButton,
        _is_maximized: bool,
        _cx: &App,
    ) -> impl IntoElement {
        let (icon_name, id, action) = match button {
            WindowButton::Minimize => (icons::IconName::Minus, "minimize", ButtonAction::Minimize),
            WindowButton::Maximize => (icons::IconName::Square, "maximize", ButtonAction::Maximize),
            WindowButton::Close => (icons::IconName::X, "close", ButtonAction::Close),
        };

        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(22.))
            .h(px(22.))
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|s| s.bg(theme::raised()))
            .active(|s| s.bg(theme::raised()))
            .child(icons::icon(icon_name, 12.).text_color(theme::text()))
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                match action {
                    ButtonAction::Minimize => window.minimize_window(),
                    ButtonAction::Maximize => window.zoom_window(),
                    ButtonAction::Close => {
                        window.remove_window();
                    }
                }
            })
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
    }

    #[derive(Clone, Copy)]
    enum ButtonAction {
        Minimize,
        Maximize,
        Close,
    }
}

#[cfg(target_os = "linux")]
pub fn render_linux_titlebar(
    window: &mut gpui::Window,
    cx: &gpui::App,
    title: gpui::SharedString,
) -> Option<gpui::AnyElement> {
    imp::render_linux_titlebar(window, cx, title)
}

/// 非 Linux 平台桩：CSD 标题栏不存在，恒返回 `None`。
#[cfg(not(target_os = "linux"))]
pub fn render_linux_titlebar(
    _window: &mut gpui::Window,
    _cx: &gpui::App,
    _title: gpui::SharedString,
) -> Option<gpui::AnyElement> {
    None
}

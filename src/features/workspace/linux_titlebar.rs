//! 渲染 Linux 客户端装饰（CSD）时的自定义标题栏。
//!
//! 平台隔离约定：本文件的全部实现仅在 `target_os = "linux"` 下编译；
//! 其他平台使用文件尾的同签名空桩（恒返回 `None`），使 Linux 特定代码
//! 与其依赖的 GPUI API 不进入其它平台的编译单元——调用点无需任何门控。

use crate::features::workspace::shell::AppShell;

use gpui::{Context, Window};

#[cfg(target_os = "linux")]
mod imp {
    use gpui::prelude::FluentBuilder as _;
    use gpui::{
        App, Context, Decorations, IntoElement, ParentElement, Styled, Window, WindowButton,
        WindowButtonLayout, WindowControlArea, div, px,
    };
    use gpui::{InteractiveElement as _, StatefulInteractiveElement as _};

    use crossh_ui::{icons, theme};

    use crate::features::workspace::shell::AppShell;

    /// 渲染 Linux 客户端装饰（CSD）时的自定义标题栏。
    ///
    /// - 仅在 `Decorations::Client` 时显示，Server 装饰由合成器提供。
    /// - 提供拖动区域（`window_control_area(Drag)` + `start_window_move`）与
    ///   最小化/最大化/关闭按钮，布局来自 `cx.button_layout()`（GNOME `gtk-decoration-layout`）
    ///   并回退到 `linux_default()`（右置 close,minimize,maximize）。
    /// - 双击标题栏触发最大化/还原，右键弹出窗口菜单。
    pub(super) fn render_linux_titlebar(
        window: &mut Window,
        cx: &mut Context<AppShell>,
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

        // 高度与 Zed 的 platform_title_bar_height 保持接近（1.75*rem, min 34px），
        // Crossh 侧用固定 36px 简化，避免 rem 波动导致的标题栏跳动。
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
            // 圆角仅在非贴边时显示，避免贴顶/贴边时出现透明缝隙（参考 Zed 的 tiling 处理）
            .map(|el| match decorations {
                Decorations::Client { tiling } => el
                    .when(!tiling.top && !tiling.right, |e| e.rounded_tr(px(8.)))
                    .when(!tiling.top && !tiling.left, |e| e.rounded_tl(px(8.))),
                _ => el,
            })
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|_, _, window, _| {
                    window.start_window_move();
                }),
            )
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
                            .child("crossh"),
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
        is_maximized: bool,
        _cx: &App,
    ) -> impl IntoElement {
        let (icon_name, id, action) = match button {
            WindowButton::Minimize => (icons::IconName::Minus, "minimize", ButtonAction::Minimize),
            WindowButton::Maximize => {
                #[allow(clippy::if_same_then_else)]
                let icon = if is_maximized {
                    icons::IconName::Square // restore 用同一图标，Zed 亦如此区分文字
                } else {
                    icons::IconName::Square
                };
                (icon, "maximize", ButtonAction::Maximize)
            }
            WindowButton::Close => (icons::IconName::X, "close", ButtonAction::Close),
        };

        let enabled = true; // 已在外层按 supported 过滤

        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(22.))
            .h(px(22.))
            .rounded(px(6.))
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|s| s.bg(theme::raised()))
                    .active(|s| s.bg(theme::raised()))
            })
            .child(icons::icon(icon_name, 12.).text_color(theme::text()))
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                match action {
                    ButtonAction::Minimize => window.minimize_window(),
                    ButtonAction::Maximize => window.zoom_window(),
                    ButtonAction::Close => {
                        // 通过 action 系统关闭，保证走 `on_window_should_close` 的确认逻辑
                        window.dispatch_action(Box::new(crate::CloseWindow), cx);
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
    window: &mut Window,
    cx: &mut Context<AppShell>,
) -> Option<gpui::AnyElement> {
    imp::render_linux_titlebar(window, cx)
}

/// 非 Linux 平台桩：CSD 标题栏不存在，恒返回 `None`。
#[cfg(not(target_os = "linux"))]
pub fn render_linux_titlebar(
    _window: &mut Window,
    _cx: &mut Context<AppShell>,
) -> Option<gpui::AnyElement> {
    None
}

use crate::features::workspace::command_palette::render_command_palette;

use super::*;

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // compose 展开时自动聚焦输入框（终端级，仅在对应终端可见性翻转时触发）
        if let Some(view) = self.workspace.focused_view() {
            let entry = self.workspace.compose_entry_mut(view);
            if entry.visible && !entry.last_visible {
                self.compose_focus.focus(window, cx);
            }
            entry.last_visible = entry.visible;
        }

        // 保持根焦点：没有终端或输入框聚焦时，按键/动作仍能沿 dispatch
        // 路径到达 #app-shell 的动作处理器（否则 Cmd+Q 等会被静默丢弃）。
        if window.focused(cx).is_none() {
            self.shell_focus.focus(window, cx);
        }

        // Materialize opaque elements before attaching the root listener so Rust 2024 does not
        // keep `cx` borrowed through the render helpers.
        let sidebar_width = if self.workspace_settings.show_host_sidebar {
            crossh_ui_component::clamp_panel_width(
                self.sidebar_width.get(),
                theme::SIDEBAR_MIN_WIDTH,
                theme::SIDEBAR_MAX_WIDTH,
            )
        } else {
            theme::SIDEBAR_RAIL_WIDTH
        };
        let available_main_width = crossh_ui_component::panel_available_main_width(
            window.viewport_size().width,
            sidebar_width,
            0.,
        );
        let main = render_main(self, window, available_main_width, cx);
        let sidebar = if self.workspace_settings.show_host_sidebar {
            render_sidebar(self, window, cx)
        } else {
            render_sidebar_rail(self, cx)
        };
        let compose_bar = self
            .workspace
            .compose_visible_for_focused()
            .then(|| crate::features::workspace::compose_bar::render_compose_bar(self, window, cx));
        let scratch_panel = self.scratch_visible.then(|| {
            crate::features::workspace::scratch_bar::render_scratch_panel(self, window, cx)
        });
        let main_column = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .relative()
            .child(main)
            .children(scratch_panel)
            .children(compose_bar);
        let workspace = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_row()
            .child(sidebar)
            .child(main_column);
        let status_bar = render_workspace_status_bar(self, available_main_width, cx);
        let linux_titlebar =
            crate::features::workspace::linux_titlebar::render_linux_titlebar(window, cx);

        let mut root =
            div()
                .id("app-shell")
                .key_context("AppShell")
                .track_focus(&self.shell_focus)
                .flex()
                .flex_col()
                .relative()
                .size_full()
                .bg(theme::canvas())
                .text_color(theme::text())
                .on_action(cx.listener(AppShell::handle_new_terminal))
                .on_action(cx.listener(AppShell::handle_close_active_tab))
                .on_action(cx.listener(|this, _: &crate::OpenProject, _, cx| {
                    this.choose_project_directory(cx)
                }))
                .on_action(cx.listener(|this, _: &crate::ToggleHostSidebar, _, cx| {
                    this.toggle_host_sidebar(cx)
                }))
                .on_action(
                    cx.listener(|this, _: &crate::ToggleScratchTerminal, window, cx| {
                        this.toggle_scratch_terminal(window, cx)
                    }),
                )
                .on_action(cx.listener(|this, _: &crate::ToggleTimestamps, _, cx| {
                    this.toggle_timestamps(cx)
                }))
                .on_action(cx.listener(AppShell::handle_quit))
                .on_key_down(cx.listener(AppShell::handle_shell_key_down))
                .children(linux_titlebar)
                .child(workspace)
                .child(status_bar);

        let system_monitor_card =
            crate::features::workspace::system_monitor::render_system_monitor_card(
                self, window, cx,
            );
        root = root.children(system_monitor_card);
        root = root.children(self.render_toaster());
        if self.command_palette.is_some() {
            root = root.child(render_command_palette(self, window, cx));
        }
        if self.rename_editor.is_some() {
            root = root.child(render_rename_editor(self, window, cx));
        }
        if self.default_command_editor.is_some() {
            root = root.child(render_default_command_editor(self, window, cx));
        }
        if let Some(menu) = self.context_menu.clone() {
            root = root.child(render_context_menu(
                &menu,
                Point::new(px(0.), px(0.)),
                window,
                cx,
                |this, action, window, cx| {
                    this.dispatch_shell_menu_action(action, window, cx);
                },
                |this, cx| this.close_context_menu(cx),
            ));
        }
        root
    }
}

use super::*;

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prompt = self.current_prompt(cx);
        let has_prompt = !matches!(prompt, PromptDisplay::None);

        if has_prompt && !self.last_had_prompt {
            self.modal_focus.focus(window, cx);
        }
        if !has_prompt {
            self.prompt_input.clear();
            self.prompt_ime_marked_text.clear();
        }
        self.last_had_prompt = has_prompt;

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

        // 快捷命令是 workspace 级面板，使用当前活动视图的上下文；没有活动视图时不显示。
        let quick_context = match self.workspace.focused_view() {
            Some(ActiveView::LocalSession(session_id)) => self
                .workspace
                .sessions
                .local_sessions
                .get(&session_id)
                .map(|session| {
                    let cwd = session
                        .terminal
                        .read(cx)
                        .cwd
                        .clone()
                        .unwrap_or_else(|| session.cwd.to_string_lossy().to_string());
                    let cwd = PathBuf::from(cwd);
                    (local_scope(&cwd), cwd.to_string_lossy().to_string())
                }),
            Some(ActiveView::RemoteTab(index)) => self
                .workspace
                .sessions
                .remote_tabs
                .get(index)
                .and_then(|tab| {
                    tab.pane
                        .cwd(cx)
                        .map(|cwd| (remote_scope(&tab.host_key, &cwd), cwd))
                }),
            None => None,
        };
        let quick_commands_panel_mode = quick_commands_panel_mode(
            quick_context.is_some(),
            self.workspace_settings.show_quick_commands,
        );
        let quick_commands = match quick_commands_panel_mode {
            Some(QuickCommandsPanelMode::Expanded) => {
                let (scope, cwd) =
                    quick_context.expect("expanded panel requires a command context");
                Some(render_quick_commands(self, scope, cwd, cx))
            }
            Some(QuickCommandsPanelMode::Rail) => {
                let (scope, _) = quick_context.expect("rail requires a command context");
                Some(render_quick_commands_rail(self, &scope, cx))
            }
            None => None,
        };

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
        let quick_commands_width = match quick_commands_panel_mode {
            Some(QuickCommandsPanelMode::Expanded) => crossh_ui_component::clamp_panel_width(
                self.quick_commands_width.get(),
                theme::QUICK_COMMANDS_MIN_WIDTH,
                theme::QUICK_COMMANDS_MAX_WIDTH,
            ),
            Some(QuickCommandsPanelMode::Rail) => theme::QUICK_COMMANDS_RAIL_WIDTH,
            None => 0.,
        };
        let available_main_width = crossh_ui_component::panel_available_main_width(
            window.viewport_size().width,
            sidebar_width,
            quick_commands_width,
        );
        let main = render_main(self, available_main_width, cx);
        let sidebar = if self.workspace_settings.show_host_sidebar {
            render_sidebar(self, window, cx)
        } else {
            render_sidebar_rail(self, cx)
        };
        let compose_bar = self
            .workspace
            .compose_visible_for_focused()
            .then(|| crate::features::workspace::compose_bar::render_compose_bar(self, window, cx));
        let main_column = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(main)
            .children(compose_bar);
        let workspace = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_row()
            .child(sidebar)
            .child(main_column)
            .children(quick_commands);
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
                .on_action(cx.listener(|this, _: &crate::ToggleQuickCommands, _, cx| {
                    this.toggle_quick_commands(cx)
                }))
                .on_action(cx.listener(|this, _: &crate::ToggleTimestamps, _, cx| {
                    this.toggle_timestamps(cx)
                }))
                .on_action(cx.listener(AppShell::handle_quit))
                .on_key_down(cx.listener(AppShell::handle_shell_key_down))
                .children(linux_titlebar)
                .child(workspace)
                .child(status_bar);

        root = root.children(self.render_toaster());

        if matches!(
            prompt,
            PromptDisplay::HostKey { .. } | PromptDisplay::Credential { .. }
        ) {
            root = root.child(render_prompt_modal(self, prompt, window, cx));
        }
        if self.quick_command_editor.is_some() {
            root = root.child(render_quick_command_editor(self, window, cx));
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

use super::*;

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.compact_layout = uses_compact_settings_layout(window.viewport_size().width);
        let section = self.section;
        let settings = self.shell_settings(cx);
        self.updates
            .update(cx, |updates, cx| updates.start_startup_check(cx));

        let general = nav_button(
            "settings-section-general",
            icons::IconName::Settings,
            i18n::text("settings.general"),
            section == SettingsSection::General,
            cx.listener(|this, _ev, _window, cx| {
                this.select_section(SettingsSection::General, cx);
            }),
        );
        let terminal = nav_button(
            "settings-section-terminal",
            icons::IconName::Terminal,
            i18n::text("settings.terminal"),
            section == SettingsSection::Terminal,
            cx.listener(|this, _ev, _window, cx| {
                this.select_section(SettingsSection::Terminal, cx);
            }),
        );
        let updates = nav_button(
            "settings-section-updates",
            icons::IconName::RefreshCw,
            i18n::text("settings.updates"),
            section == SettingsSection::Updates,
            cx.listener(|this, _ev, _window, cx| {
                this.select_section(SettingsSection::Updates, cx);
            }),
        );
        let providers = nav_button(
            "settings-section-providers",
            icons::IconName::Server,
            i18n::text("settings.providers"),
            section == SettingsSection::Providers,
            cx.listener(|this, _ev, _window, cx| {
                this.select_section(SettingsSection::Providers, cx);
            }),
        );
        let agent = nav_button(
            "settings-section-agent",
            icons::IconName::Bot,
            i18n::text("settings.agent"),
            section == SettingsSection::Agent,
            cx.listener(|this, _ev, _window, cx| {
                this.select_section(SettingsSection::Agent, cx);
            }),
        );
        let about = nav_button(
            "settings-section-about",
            icons::IconName::Info,
            i18n::text("settings.about"),
            section == SettingsSection::About,
            cx.listener(|this, _ev, _window, cx| {
                this.select_section(SettingsSection::About, cx);
            }),
        );

        let section_content = match section {
            SettingsSection::General => self.render_general_settings(&settings, cx),
            SettingsSection::Terminal => self.render_terminal_settings(&settings, cx),
            SettingsSection::Providers => self.render_provider_settings(&settings, window, cx),
            SettingsSection::Agent => self.render_agent_settings(&settings, cx),
            SettingsSection::Updates => self.render_updates_settings(&settings, cx),
            SettingsSection::About => self.render_about_settings(),
        };

        let navigation = if self.compact_layout {
            div()
                .w_full()
                .flex_shrink_0()
                .p_3()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap_1()
                .bg(theme::sidebar())
                .border_b_1()
                .border_color(theme::border_strong())
                .child(general)
                .child(terminal)
                .child(providers)
                .child(agent)
                .child(updates)
                .child(about)
        } else {
            div()
                .w(px(180.))
                .flex_shrink_0()
                .p_3()
                .flex()
                .flex_col()
                .gap_1()
                .bg(theme::sidebar())
                .border_r_1()
                .border_color(theme::border_strong())
                .child(general)
                .child(terminal)
                .child(providers)
                .child(agent)
                .child(updates)
                .child(div().flex_1())
                .child(about)
        };

        let content_container = scroll_y(&self.scroll)
            .id("settings-content")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .py_4()
            .child(section_content);
        let content_container = if self.compact_layout {
            content_container.px_4()
        } else {
            content_container.px_5()
        };
        let main = if self.compact_layout {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(navigation)
                .child(content_container)
        } else {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(navigation)
                .child(content_container)
        };

        div()
            .id("settings-window")
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .child(main)
    }
}

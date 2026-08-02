//! 设置页：General（语言）/ Terminal（时间戳、字号、滚动回退）两节。

use gpui::{
    AnyElement, App, AppContext, Context, FontWeight, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::i18n::{self, LanguagePreference};
use crate::ui::app_shell::AppShell;
use crate::ui::widgets::LocalPathTooltip;
use crate::ui::{icons, theme};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsSection {
    General,
    Terminal,
}

pub fn render_settings_page(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let section = shell.settings_section;
    let general = div()
        .id("settings-section-general")
        .h(px(32.))
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .text_xs()
        .text_color(if section == SettingsSection::General {
            theme::text()
        } else {
            theme::muted_text()
        })
        .bg(if section == SettingsSection::General {
            theme::accent_soft()
        } else {
            theme::canvas()
        })
        .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
        .child(icons::icon(icons::IconName::Settings, 14.).text_color(if section == SettingsSection::General {
            theme::text()
        } else {
            theme::muted_text()
        }).hover(|s| s.text_color(theme::text())))
        .child(SharedString::from(i18n::text("settings.general")))
        .on_click(cx.listener(|this, _ev, _window, cx| {
            this.select_settings_section(SettingsSection::General, cx);
        }));
    let terminal = div()
        .id("settings-section-terminal")
        .h(px(32.))
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .text_xs()
        .text_color(if section == SettingsSection::Terminal {
            theme::text()
        } else {
            theme::muted_text()
        })
        .bg(if section == SettingsSection::Terminal {
            theme::accent_soft()
        } else {
            theme::canvas()
        })
        .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
        .child(icons::icon(icons::IconName::Terminal, 14.).text_color(if section == SettingsSection::Terminal {
            theme::text()
        } else {
            theme::muted_text()
        }).hover(|s| s.text_color(theme::text())))
        .child(SharedString::from(i18n::text("settings.terminal")))
        .on_click(cx.listener(|this, _ev, _window, cx| {
            this.select_settings_section(SettingsSection::Terminal, cx);
        }));

    let content = match section {
        SettingsSection::General => render_general_settings(shell, cx),
        SettingsSection::Terminal => render_terminal_settings(shell, cx),
    };

    div()
        .id("settings-page")
        .size_full()
        .flex()
        .flex_col()
        .bg(theme::canvas())
        .child(
            div()
                .h(px(theme::TAB_HEIGHT))
                .flex_shrink_0()
                .px_5()
                .flex()
                .items_center()
                .gap_2()
                .bg(theme::surface())
                .border_b_1()
                .border_color(theme::border())
                .child(icons::icon(icons::IconName::Settings, 16.).text_color(theme::accent()))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(SharedString::from(i18n::text("settings.title"))),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .id("settings-close")
                        .w(px(28.))
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(theme::RADIUS_SM))
                        .cursor_pointer()
                        .text_color(theme::muted_text())
                        .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
                        .tooltip(|_window, cx| {
                            cx.new(|_| LocalPathTooltip {
                                path: SharedString::from(i18n::text("settings.close")),
                            })
                            .into()
                        })
                        .child(icons::icon(icons::IconName::X, 14.).text_color(theme::muted_text()).hover(|s| s.text_color(theme::text())))
                        .on_click(cx.listener(|this, _ev, _window, cx| {
                            this.close_settings(cx);
                        })),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(
                    div()
                        .w(px(180.))
                        .flex_shrink_0()
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .border_r_1()
                        .border_color(theme::border())
                        .child(general)
                        .child(terminal),
                )
                .child(
                    div()
                        .id("settings-content")
                        .track_scroll(&shell.settings_scroll)
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px_5()
                        .py_4()
                        .child(content),
                ),
        )
        .into_any_element()
}

fn render_general_settings(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let mut languages = div().flex().flex_row().gap_1().flex_wrap().justify_end();
    for preference in LanguagePreference::ALL {
        let selected = preference == shell.language_preference;
        let option = div()
            .id(format!("settings-language-{preference:?}"))
            .h(px(30.))
            .px_2()
            .flex()
            .items_center()
            .rounded(px(theme::RADIUS_SM))
            .cursor_pointer()
            .text_xs()
            .text_color(if selected {
                theme::canvas()
            } else {
                theme::muted_text()
            })
            .bg(if selected {
                theme::accent()
            } else {
                theme::raised()
            })
            .hover(|s| s.bg(theme::accent()).text_color(theme::canvas()))
            .child(SharedString::from(i18n::preference_label(preference)))
            .on_click(cx.listener(move |this, _ev, _window, cx| {
                this.set_language(preference, cx);
            }));
        languages = languages.child(option);
    }

    div()
        .id("settings-general")
        .max_w(px(760.))
        .flex()
        .flex_col()
        .child(settings_heading("settings.general"))
        .child(settings_row(
            i18n::text("settings.language"),
            i18n::text("settings.language_description"),
            languages.into_any_element(),
        ))
        .into_any_element()
}

fn render_terminal_settings(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let mut timestamps = div()
        .id("settings-timestamps-toggle")
        .w(px(42.))
        .h(px(24.))
        .p_1()
        .flex()
        .items_center()
        .rounded_full()
        .cursor_pointer()
        .bg(if shell.settings.show_timestamps {
            theme::accent()
        } else {
            theme::border_strong()
        });
    timestamps = if shell.settings.show_timestamps {
        timestamps.justify_end()
    } else {
        timestamps.justify_start()
    };
    timestamps = timestamps.child(
        div()
            .w(px(18.))
            .h(px(18.))
            .rounded_full()
            .bg(theme::canvas()),
    );
    timestamps = timestamps.on_click(cx.listener(|this, _ev, _window, cx| {
        this.toggle_timestamps(cx);
    }));

    let font_size = shell.settings.terminal_font_size.round() as u32;
    let font_control = div()
        .flex()
        .items_center()
        .gap_1()
        .child(settings_icon_button(
            "settings-font-decrease",
            icons::IconName::Minus,
            i18n::text("settings.font_size"),
            cx.listener(|this, _ev, _window, cx| this.adjust_font_size(-1.0, cx)),
        ))
        .child(
            div()
                .w(px(64.))
                .h(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::RADIUS_SM))
                .bg(theme::raised())
                .text_xs()
                .text_color(theme::text())
                .child(SharedString::from(
                    rust_i18n::t!("settings.pixels", value = font_size).to_string(),
                )),
        )
        .child(settings_icon_button(
            "settings-font-increase",
            icons::IconName::Plus,
            i18n::text("settings.font_size"),
            cx.listener(|this, _ev, _window, cx| this.adjust_font_size(1.0, cx)),
        ));

    let scrollback_values = [500usize, 1000, 5000, 10000];
    let mut scrollback = div().flex().flex_row().gap_1().flex_wrap().justify_end();
    for value in scrollback_values {
        let selected = value == shell.settings.terminal_scrollback;
        let option = div()
            .id(format!("settings-scrollback-{value}"))
            .h(px(30.))
            .px_2()
            .flex()
            .items_center()
            .rounded(px(theme::RADIUS_SM))
            .cursor_pointer()
            .text_xs()
            .text_color(if selected {
                theme::canvas()
            } else {
                theme::muted_text()
            })
            .bg(if selected {
                theme::accent()
            } else {
                theme::raised()
            })
            .hover(|s| s.bg(theme::accent()).text_color(theme::canvas()))
            .child(SharedString::from(
                rust_i18n::t!("settings.lines", value = value).to_string(),
            ))
            .on_click(cx.listener(move |this, _ev, _window, cx| {
                this.set_scrollback(value, cx);
            }));
        scrollback = scrollback.child(option);
    }

    div()
        .id("settings-terminal")
        .max_w(px(760.))
        .flex()
        .flex_col()
        .child(settings_heading("settings.terminal"))
        .child(settings_row(
            i18n::text("settings.timestamps"),
            i18n::text("settings.timestamps_description"),
            timestamps.into_any_element(),
        ))
        .child(settings_row(
            i18n::text("settings.font_size"),
            i18n::text("settings.font_size_description"),
            font_control.into_any_element(),
        ))
        .child(settings_row(
            i18n::text("settings.scrollback"),
            i18n::text("settings.scrollback_description"),
            scrollback.into_any_element(),
        ))
        .into_any_element()
}

fn settings_heading(key: &str) -> AnyElement {
    div()
        .pb_2()
        .text_lg()
        .font_weight(FontWeight::MEDIUM)
        .child(SharedString::from(i18n::text(key)))
        .into_any_element()
}

fn settings_row(label: String, description: String, control: AnyElement) -> AnyElement {
    div()
        .w_full()
        .py_4()
        .flex()
        .items_center()
        .gap_4()
        .border_b_1()
        .border_color(theme::border())
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .text_color(theme::text())
                        .child(SharedString::from(label)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::muted_text())
                        .child(SharedString::from(description)),
                ),
        )
        .child(control)
        .into_any_element()
}

fn settings_icon_button(
    id: &'static str,
    icon: icons::IconName,
    tooltip: String,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .w(px(30.))
        .h(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .bg(theme::raised())
        .text_color(theme::muted_text())
        .hover(|s| s.bg(theme::accent()).text_color(theme::canvas()))
        .tooltip(move |_window, cx| {
            cx.new(|_| LocalPathTooltip {
                path: SharedString::from(tooltip.clone()),
            })
            .into()
        })
        .child(icons::icon(icon, 14.).text_color(theme::muted_text()).hover(|s| s.text_color(theme::canvas())))
        .on_click(on_click)
        .into_any_element()
}

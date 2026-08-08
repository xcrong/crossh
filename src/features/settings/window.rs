//! 设置窗口：独立的 gpui 窗口（借鉴 Zed 的 `SettingsWindow`）。
//!
//! 设置值以主窗口的 `AppShell` 为准（唯一真源），本窗口只持有其弱引用：
//! 渲染时从 `AppShell` 读取，用户改动通过 `AppShell` 的既有 setter 应用并持久化。
//! 这样终端重放、i18n 全局同步、最近目录同步等副作用都仍由主窗口统一处理。

use gpui::{
    AnyElement, App, AppContext, Bounds, ClickEvent, Context, FontWeight, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, Size, StatefulInteractiveElement, Styled,
    TitlebarOptions, WeakEntity, Window, WindowBounds, WindowOptions, div, px,
};

use crate::features::settings::{self, SettingsSnapshot};
use crate::features::workspace::AppShell;
use crate::shared::i18n::{self, LanguagePreference};
use crate::shared::ui::widgets::LocalPathTooltip;
use crate::shared::ui::{icons, theme};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsSection {
    General,
    Terminal,
}

/// 设置窗口的根视图。窗口关闭即释放。
pub struct SettingsWindow {
    /// 主窗口 AppShell 的弱引用：设置值读写都委托给它。
    shell: WeakEntity<AppShell>,
    section: SettingsSection,
    scroll: gpui::ScrollHandle,
}

impl SettingsWindow {
    fn new(shell: WeakEntity<AppShell>) -> Self {
        Self {
            shell,
            section: SettingsSection::General,
            scroll: gpui::ScrollHandle::new(),
        }
    }

    fn shell_settings(&self, cx: &App) -> SettingsSnapshot {
        match self.shell.upgrade() {
            Some(shell) => {
                let shell = shell.read(cx);
                SettingsSnapshot {
                    language: shell.language_preference,
                    terminal: shell.terminal_settings.clone(),
                    workspace: shell.workspace_settings.clone(),
                }
            }
            None => settings::load(),
        }
    }

    fn write_to_shell(
        &self,
        cx: &mut Context<Self>,
        update: impl FnOnce(&mut AppShell, &mut Context<AppShell>),
    ) {
        let _ = self.shell.update(cx, update);
    }

    fn select_section(&mut self, section: SettingsSection, cx: &mut Context<Self>) {
        self.section = section;
        self.scroll.set_offset(gpui::Point::new(px(0.), px(0.)));
        cx.notify();
    }

    fn render_general_settings(
        &self,
        settings: &SettingsSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut languages = div().flex().flex_row().gap_1().flex_wrap().justify_end();
        for preference in LanguagePreference::ALL {
            let selected = preference == settings.language;
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
                    this.write_to_shell(cx, |shell, cx| shell.set_language(preference, cx));
                }));
            languages = languages.child(option);
        }

        let recent_dirs_max = settings.workspace.recent_dirs_max;
        let recent_dirs_control = div()
            .flex()
            .items_center()
            .gap_1()
            .child(settings_icon_button(
                "settings-recent-dirs-decrease",
                icons::IconName::Minus,
                i18n::text("settings.recent_dirs"),
                cx.listener(|this, _ev, _window, cx| {
                    let max = this
                        .shell_settings(cx)
                        .workspace
                        .recent_dirs_max
                        .saturating_sub(1);
                    this.write_to_shell(cx, |shell, cx| shell.set_recent_dirs_max(max, cx));
                }),
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
                        rust_i18n::t!("settings.dirs", value = recent_dirs_max).to_string(),
                    )),
            )
            .child(settings_icon_button(
                "settings-recent-dirs-increase",
                icons::IconName::Plus,
                i18n::text("settings.recent_dirs"),
                cx.listener(|this, _ev, _window, cx| {
                    let max = this.shell_settings(cx).workspace.recent_dirs_max + 1;
                    this.write_to_shell(cx, |shell, cx| shell.set_recent_dirs_max(max, cx));
                }),
            ))
            .child(settings_icon_button(
                "settings-recent-dirs-clear",
                icons::IconName::X,
                i18n::text("settings.recent_dirs_clear"),
                cx.listener(|this, _ev, _window, cx| {
                    this.write_to_shell(cx, |shell, cx| shell.clear_recent_dirs(cx));
                }),
            ));

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
            .child(settings_row(
                i18n::text("settings.recent_dirs"),
                i18n::text("settings.recent_dirs_description"),
                recent_dirs_control.into_any_element(),
            ))
            .into_any_element()
    }

    fn render_terminal_settings(
        &self,
        settings: &SettingsSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut timestamps = div()
            .id("settings-timestamps-toggle")
            .w(px(42.))
            .h(px(24.))
            .p_1()
            .flex()
            .items_center()
            .rounded_full()
            .cursor_pointer()
            .bg(if settings.terminal.show_timestamps {
                theme::accent()
            } else {
                theme::border_strong()
            });
        timestamps = if settings.terminal.show_timestamps {
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
            this.write_to_shell(cx, |shell, cx| shell.toggle_timestamps(cx));
        }));

        let mut notifications = div()
            .id("settings-terminal-notifications-toggle")
            .w(px(42.))
            .h(px(24.))
            .p_1()
            .flex()
            .items_center()
            .rounded_full()
            .cursor_pointer()
            .bg(if settings.terminal.notifications_enabled {
                theme::accent()
            } else {
                theme::border_strong()
            });
        notifications = if settings.terminal.notifications_enabled {
            notifications.justify_end()
        } else {
            notifications.justify_start()
        };
        notifications = notifications.child(
            div()
                .w(px(18.))
                .h(px(18.))
                .rounded_full()
                .bg(theme::canvas()),
        );
        notifications = notifications.on_click(cx.listener(|this, _ev, _window, cx| {
            this.write_to_shell(cx, |shell, cx| shell.toggle_terminal_notifications(cx));
        }));

        let font_size = settings.terminal.font_size.round() as u32;
        let font_control = div()
            .flex()
            .items_center()
            .gap_1()
            .child(settings_icon_button(
                "settings-font-decrease",
                icons::IconName::Minus,
                i18n::text("settings.font_size"),
                cx.listener(|this, _ev, _window, cx| {
                    this.write_to_shell(cx, |shell, cx| shell.adjust_font_size(-1.0, cx));
                }),
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
                cx.listener(|this, _ev, _window, cx| {
                    this.write_to_shell(cx, |shell, cx| shell.adjust_font_size(1.0, cx));
                }),
            ));

        let scrollback_values = [500usize, 1000, 5000, 10000];
        let mut scrollback = div().flex().flex_row().gap_1().flex_wrap().justify_end();
        for value in scrollback_values {
            let selected = value == settings.terminal.scrollback;
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
                    this.write_to_shell(cx, |shell, cx| shell.set_scrollback(value, cx));
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
                i18n::text("settings.notifications"),
                i18n::text("settings.notifications_description"),
                notifications.into_any_element(),
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
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let section = self.section;
        let settings = self.shell_settings(cx);

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

        let content = match section {
            SettingsSection::General => self.render_general_settings(&settings, cx),
            SettingsSection::Terminal => self.render_terminal_settings(&settings, cx),
        };

        div()
            .id("settings-window")
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::canvas())
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
                            .track_scroll(&self.scroll)
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .px_5()
                            .py_4()
                            .child(content),
                    ),
            )
    }
}

fn nav_button(
    id: &'static str,
    icon: icons::IconName,
    label: String,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .h(px(32.))
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .text_xs()
        .text_color(if selected {
            theme::text()
        } else {
            theme::muted_text()
        })
        .bg(if selected {
            theme::accent_soft()
        } else {
            theme::canvas()
        })
        .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
        .child(
            icons::icon(icon, 14.)
                .text_color(if selected {
                    theme::text()
                } else {
                    theme::muted_text()
                })
                .hover(|s| s.text_color(theme::text())),
        )
        .child(SharedString::from(label))
        .on_click(on_click)
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
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
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
        .child(
            icons::icon(icon, 14.)
                .text_color(theme::muted_text())
                .hover(|s| s.text_color(theme::canvas())),
        )
        .on_click(on_click)
        .into_any_element()
}

/// 设置窗口当前是否打开（供侧栏按钮高亮）。
pub fn is_settings_window_open(cx: &App) -> bool {
    cx.windows()
        .iter()
        .any(|handle| handle.downcast::<SettingsWindow>().is_some())
}

/// 切换设置窗口：已存在则关闭，否则打开。
pub fn toggle_settings(shell: WeakEntity<AppShell>, cx: &mut Context<AppShell>) {
    if let Some(window) = cx
        .windows()
        .iter()
        .find_map(|handle| handle.downcast::<SettingsWindow>())
    {
        let _ = window.update(cx, |_, window, _| window.remove_window());
        cx.notify();
        return;
    }
    open_settings_window(shell, cx);
}

/// 打开设置窗口（借鉴 Zed：复用已有窗口，否则新开独立窗口）。
/// 延迟到当前帧结束再开窗口，避免在渲染/输入分发中途操作窗口列表。
pub fn open_settings_window(shell: WeakEntity<AppShell>, cx: &mut App) {
    if let Some(window) = cx
        .windows()
        .iter()
        .find_map(|handle| handle.downcast::<SettingsWindow>())
    {
        let _ = window.update(cx, |_, window, _| window.activate_window());
        return;
    }

    cx.defer(move |cx| {
        let bounds = Bounds::centered(
            None,
            Size {
                width: px(720.),
                height: px(540.),
            },
            cx,
        );
        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::from(i18n::text("settings.title"))),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(Size {
                    width: px(560.),
                    height: px(400.),
                }),
                ..Default::default()
            },
            |window, cx| {
                let notify_shell = shell.clone();
                // 平台侧关闭（Cmd+W / 红按钮）时通知主窗口刷新按钮高亮。
                window.on_window_should_close(cx, move |_window, cx| {
                    let _ = notify_shell.update(cx, |_shell, cx| cx.notify());
                    true
                });
                cx.new(|_| SettingsWindow::new(shell))
            },
        )
        .ok();
    });
}

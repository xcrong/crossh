//! 设置窗口：独立的 gpui 窗口（借鉴 Zed 的 `SettingsWindow`）。
//!
//! 设置值以主窗口的 `AppShell` 为准（唯一真源），本窗口只持有其弱引用：
//! 渲染时从 `AppShell` 读取，用户改动通过 `AppShell` 的既有 setter 应用并持久化。
//! 这样终端重放、i18n 全局同步、最近目录同步等副作用都仍由主窗口统一处理。

use gpui::{
    AnyElement, App, AppContext, Bounds, ClickEvent, Context, Entity, FontWeight,
    InteractiveElement, IntoElement, ParentElement, Pixels, Render, SharedString, Size,
    StatefulInteractiveElement, Styled, Subscription, TitlebarOptions, WeakEntity, Window,
    WindowBounds, WindowOptions, div, px,
};

use crate::features::editor_launcher;
use crate::features::settings::{self, SettingsSnapshot};
use crate::features::updates::{UpdateController, UpdateStatus};
use crate::features::workspace::AppShell;
use crate::shared::i18n::{self, LanguagePreference};
use crossh_ui::{icons, theme};
use crossh_ui_component::{
    Button, ButtonSize, ButtonVariant, Select, SelectOption, Stepper, ToggleSwitch, scroll_y,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsSection {
    General,
    Terminal,
    Updates,
    About,
}

// The sidebar is 180px wide; below this point it leaves too little room for form rows.
const SETTINGS_COMPACT_WIDTH: f32 = 640.;

fn uses_compact_settings_layout(width: Pixels) -> bool {
    width < px(SETTINGS_COMPACT_WIDTH)
}

fn observe_update_status<T>(updates: &Entity<UpdateController>, cx: &mut Context<T>) -> Subscription
where
    T: 'static,
{
    cx.observe(updates, |_, _, cx| cx.notify())
}

/// 设置窗口的根视图。窗口关闭即释放。
pub struct SettingsWindow {
    /// 主窗口 AppShell 的弱引用：设置值读写都委托给它。
    shell: WeakEntity<AppShell>,
    section: SettingsSection,
    scroll: gpui::ScrollHandle,
    updates: Entity<UpdateController>,
    _updates_subscription: Subscription,
    /// 外部编辑器下拉：是否展开（受控）
    editor_select_open: bool,
    compact_layout: bool,
}

impl SettingsWindow {
    fn new(shell: WeakEntity<AppShell>, cx: &mut Context<Self>) -> Self {
        let loaded = settings::load();
        let updates = shell
            .upgrade()
            .map(|shell| shell.read(cx).updates.clone())
            .unwrap_or_else(|| cx.new(|_| UpdateController::new(loaded.updates.clone())));
        let updates_subscription = observe_update_status(&updates, cx);
        Self {
            shell,
            section: SettingsSection::General,
            scroll: gpui::ScrollHandle::new(),
            updates,
            _updates_subscription: updates_subscription,
            editor_select_open: false,
            compact_layout: false,
        }
    }

    fn shell_settings(&self, cx: &App) -> SettingsSnapshot {
        match self.shell.upgrade() {
            Some(shell) => {
                let shell = shell.read(cx);
                SettingsSnapshot {
                    language: shell.language_preference,
                    terminal: shell.terminal_settings.clone(),
                    updates: shell.update_settings.clone(),
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
        let compact_layout = self.compact_layout;
        let mut languages = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_row()
            .gap_1()
            .flex_wrap()
            .justify_end();
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
            .child(
                Stepper::new("settings-recent-dirs")
                    .value(rust_i18n::t!("settings.dirs", value = recent_dirs_max))
                    .tooltips(
                        i18n::text("settings.recent_dirs"),
                        i18n::text("settings.recent_dirs"),
                    )
                    .on_decrease(cx.listener(|this, _ev, _window, cx| {
                        let max = this
                            .shell_settings(cx)
                            .workspace
                            .recent_dirs_max
                            .saturating_sub(1);
                        this.write_to_shell(cx, |shell, cx| shell.set_recent_dirs_max(max, cx));
                    }))
                    .on_increase(cx.listener(|this, _ev, _window, cx| {
                        let max = this.shell_settings(cx).workspace.recent_dirs_max + 1;
                        this.write_to_shell(cx, |shell, cx| shell.set_recent_dirs_max(max, cx));
                    })),
            )
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
            .w_full()
            .flex()
            .flex_col()
            .child(settings_heading("settings.general"))
            .child(responsive_settings_row(
                i18n::text("settings.language"),
                i18n::text("settings.language_description"),
                languages.into_any_element(),
                compact_layout,
            ))
            .child(responsive_settings_row(
                i18n::text("settings.recent_dirs"),
                i18n::text("settings.recent_dirs_description"),
                recent_dirs_control.into_any_element(),
                compact_layout,
            ))
            .child(self.render_editor_settings_rows(settings, cx))
            .into_any_element()
    }

    /// 外部编辑器设置：下拉选择框，选项来自自动检测的已安装编辑器。
    /// 点击展开时实时检测并生成菜单；「自动检测」清除覆盖，其余选项写入
    /// `editor_command`。
    fn render_editor_settings_rows(
        &self,
        settings: &SettingsSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let compact_layout = self.compact_layout;
        let configured = settings.workspace.editor_command.clone();
        let path_env = editor_launcher::effective_path();
        let detected =
            editor_launcher::detect_editors(&path_env, editor_launcher::executable_exists);

        let mut ids: Vec<String> = Vec::new();
        let mut options: Vec<SelectOption> = Vec::new();
        ids.push("auto".into());
        options.push(SelectOption::new(
            "auto",
            i18n::text("settings.editor_auto"),
        ));
        let mut seen = std::collections::HashSet::new();
        for path in detected {
            seen.insert(path.clone());
            ids.push(path.clone());
            options.push(SelectOption::new(
                path.clone(),
                editor_launcher::command_display_name(&path),
            ));
        }
        if let Some(cfg) = &configured
            && !seen.contains(cfg)
        {
            ids.push(cfg.clone());
            options.push(SelectOption::new(
                cfg.clone(),
                editor_launcher::command_display_name(cfg),
            ));
        }
        let selected_index = if configured.is_none() {
            Some(0)
        } else {
            configured
                .as_deref()
                .and_then(|c| ids.iter().position(|id| id == c))
        };

        let is_open = self.editor_select_open;
        let entity = cx.entity();
        let ids_for_select = ids.clone();
        let select = Select::new("settings-editor-select")
            .options(options)
            .selected(selected_index)
            .placeholder(i18n::text("settings.editor_auto"))
            .is_open(is_open)
            .on_toggle(cx.listener(|this, _ev, _window, cx| {
                this.editor_select_open = !this.editor_select_open;
                cx.notify();
            }))
            .on_select(move |idx, _window, cx| {
                entity.update(cx, |this, cx| {
                    this.editor_select_open = false;
                    if let Some(id) = ids_for_select.get(idx) {
                        if id == "auto" {
                            this.write_to_shell(cx, |shell, cx| shell.set_editor_command(None, cx));
                        } else {
                            let cmd = id.clone();
                            this.write_to_shell(cx, move |shell, cx| {
                                shell.set_editor_command(Some(cmd), cx);
                            });
                        }
                    }
                    cx.notify();
                });
            })
            .into_any_element();

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(responsive_settings_row(
                i18n::text("settings.editor_command"),
                i18n::text("settings.editor_command_description"),
                select,
                compact_layout,
            ))
            .into_any_element()
    }

    fn render_terminal_settings(
        &self,
        settings: &SettingsSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let compact_layout = self.compact_layout;
        let entity = cx.entity();
        let notifications = ToggleSwitch::new("settings-terminal-notifications-toggle")
            .on(settings.terminal.notifications_enabled)
            .on_toggle(move |_on, _ev, _window, cx| {
                entity.update(cx, |this, cx| {
                    this.write_to_shell(cx, |shell, cx| shell.toggle_terminal_notifications(cx));
                });
            });

        let font_size = settings.terminal.font_size.round() as u32;
        let font_control = Stepper::new("settings-font")
            .value(rust_i18n::t!("settings.pixels", value = font_size))
            .tooltips(
                i18n::text("settings.font_size"),
                i18n::text("settings.font_size"),
            )
            .on_decrease(cx.listener(|this, _ev, _window, cx| {
                this.write_to_shell(cx, |shell, cx| shell.adjust_font_size(-1.0, cx));
            }))
            .on_increase(cx.listener(|this, _ev, _window, cx| {
                this.write_to_shell(cx, |shell, cx| shell.adjust_font_size(1.0, cx));
            }));

        let scrollback_values = [500usize, 1000, 5000, 10000];
        let mut scrollback = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_row()
            .gap_1()
            .flex_wrap()
            .justify_end();
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
            .w_full()
            .flex()
            .flex_col()
            .child(settings_heading("settings.terminal"))
            .child(responsive_settings_row(
                i18n::text("settings.notifications"),
                i18n::text("settings.notifications_description"),
                notifications.into_any_element(),
                compact_layout,
            ))
            .child(responsive_settings_row(
                i18n::text("settings.font_size"),
                i18n::text("settings.font_size_description"),
                font_control.into_any_element(),
                compact_layout,
            ))
            .child(responsive_settings_row(
                i18n::text("settings.scrollback"),
                i18n::text("settings.scrollback_description"),
                scrollback.into_any_element(),
                compact_layout,
            ))
            .into_any_element()
    }

    fn render_updates_settings(
        &mut self,
        settings: &SettingsSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let compact_layout = self.compact_layout;
        self.updates.update(cx, |updates, _cx| {
            updates.set_settings(settings.updates.clone())
        });
        let status = self
            .updates
            .read_with(cx, |updates, _app| updates.status().clone());

        let entity = cx.entity();
        let startup_toggle = ToggleSwitch::new("settings-updates-startup-toggle")
            .on(settings.updates.check_on_startup)
            .on_toggle(move |on, _ev, _window, cx| {
                // 原实现读取 `!this.shell_settings(cx).updates.check_on_startup`;
                // 快照与 shell 状态在渲染时一致,新状态 `on` 与之等价,直接使用即可。
                entity.update(cx, |this, cx| {
                    this.write_to_shell(cx, |shell, cx| shell.set_update_check_on_startup(on, cx));
                });
            });

        let (status_text, status_color) = update_status_presentation(&status);
        let status_control = div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .max_w(px(300.))
                    .text_xs()
                    .text_color(status_color)
                    .child(SharedString::from(status_text)),
            )
            .child(settings_icon_button(
                "settings-updates-check",
                icons::IconName::RefreshCw,
                i18n::text("settings.updates_check_now"),
                cx.listener(|this, _ev, _window, cx| {
                    this.updates.update(cx, |updates, cx| updates.check(cx));
                }),
            ));

        let mut content = div()
            .id("settings-updates")
            .w_full()
            .flex()
            .flex_col()
            .child(settings_heading("settings.updates"))
            .child(responsive_settings_row(
                i18n::text("settings.updates_check_on_startup"),
                i18n::text("settings.updates_check_on_startup_description"),
                startup_toggle.into_any_element(),
                compact_layout,
            ))
            .child(responsive_settings_row(
                i18n::text("settings.updates_status"),
                i18n::text("settings.updates_status_description"),
                status_control.into_any_element(),
                compact_layout,
            ));

        match status {
            UpdateStatus::Available(candidate) => {
                let version = candidate.version.to_string();
                let download = settings_icon_button(
                    "settings-updates-download",
                    icons::IconName::Download,
                    i18n::text("settings.updates_download"),
                    cx.listener(|this, _ev, _window, cx| {
                        this.updates.update(cx, |updates, cx| updates.download(cx));
                    }),
                );
                let mut actions = div().flex().items_center().gap_1().child(download);
                if let Some(release_url) = candidate.release_url.clone() {
                    actions = actions.child(settings_icon_button(
                        "settings-updates-release",
                        icons::IconName::Link,
                        i18n::text("settings.updates_release"),
                        move |_ev, _window, cx| cx.open_url(&release_url),
                    ));
                }
                content = content.child(responsive_settings_row(
                    rust_i18n::t!("settings.updates_available", version = version).to_string(),
                    if candidate.notes.is_empty() {
                        i18n::text("settings.updates_available_description")
                    } else {
                        candidate.notes
                    },
                    actions.into_any_element(),
                    compact_layout,
                ));
            }
            UpdateStatus::Downloading(candidate) => {
                content = content.child(responsive_settings_row(
                    rust_i18n::t!(
                        "settings.updates_downloading",
                        version = candidate.version.to_string()
                    )
                    .to_string(),
                    i18n::text("settings.updates_downloading_description"),
                    div().into_any_element(),
                    compact_layout,
                ));
            }
            UpdateStatus::Ready { candidate, package } => {
                let package_text = package.display().to_string();
                let install = settings_icon_button(
                    "settings-updates-install",
                    icons::IconName::RefreshCw,
                    i18n::text("settings.updates_install"),
                    cx.listener(|this, _ev, _window, cx| this.install_update(cx)),
                );
                content = content.child(responsive_settings_row(
                    rust_i18n::t!(
                        "settings.updates_ready",
                        version = candidate.version.to_string()
                    )
                    .to_string(),
                    package_text,
                    install,
                    compact_layout,
                ));
            }
            _ => {}
        }

        content.into_any_element()
    }

    fn install_update(&mut self, cx: &mut Context<Self>) {
        match self.updates.update(cx, |updates, _cx| updates.install()) {
            Ok(()) => self.write_to_shell(cx, |shell, cx| shell.quit_for_update(cx)),
            Err(error) => {
                self.updates
                    .update(cx, |updates, _cx| updates.set_failed(error));
                cx.notify();
            }
        }
    }

    fn render_about_settings(&self) -> AnyElement {
        let compact_layout = self.compact_layout;
        let version = div()
            .text_sm()
            .text_color(theme::text())
            .child(SharedString::from(format!(
                "v{}",
                env!("CARGO_PKG_VERSION")
            )));
        let source = settings_link_button(
            "settings-about-source",
            i18n::text("settings.about_source_open"),
            |_, _window, cx| cx.open_url("https://github.com/xcrong/crossh"),
        );
        let license = settings_link_button(
            "settings-about-license",
            i18n::text("settings.about_license_open"),
            |_, _window, cx| cx.open_url("https://github.com/xcrong/crossh/blob/main/LICENSE"),
        );

        div()
            .id("settings-about")
            .w_full()
            .flex()
            .flex_col()
            .child(settings_heading("settings.about"))
            .child(
                div()
                    .w_full()
                    .py_4()
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .w(px(56.))
                            .h(px(56.))
                            .flex_shrink_0()
                            .rounded(px(theme::RADIUS_MD))
                            .overflow_hidden()
                            .child(icons::logo(56.)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::text())
                                    .child(SharedString::from("Crossh")),
                            )
                            .child(div().text_sm().text_color(theme::muted_text()).child(
                                SharedString::from(i18n::text("settings.about_description")),
                            )),
                    ),
            )
            .child(responsive_settings_row(
                i18n::text("settings.about_version"),
                i18n::text("settings.about_version_description"),
                version.into_any_element(),
                compact_layout,
            ))
            .child(responsive_settings_row(
                i18n::text("settings.about_source"),
                i18n::text("settings.about_source_description"),
                source,
                compact_layout,
            ))
            .child(responsive_settings_row(
                i18n::text("settings.about_license"),
                i18n::text("settings.about_license_description"),
                license,
                compact_layout,
            ))
            .into_any_element()
    }
}

#[path = "render.rs"]
mod render;

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
        .border_l_2()
        .border_color(if selected {
            theme::accent()
        } else {
            theme::sidebar()
        })
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
            theme::sidebar()
        })
        .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
        .child(
            icons::icon(icon, 14.)
                .text_color(if selected {
                    theme::accent()
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
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::text())
        .child(SharedString::from(i18n::text(key)))
        .into_any_element()
}

fn responsive_settings_row(
    label: String,
    description: String,
    control: AnyElement,
    compact: bool,
) -> AnyElement {
    let mut label = div()
        .min_w_0()
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
        );
    let mut control = div()
        .min_w_0()
        .flex()
        .flex_wrap()
        .items_center()
        .child(control);

    let row = if compact {
        label = label.w_full();
        control = control.w_full().justify_start();
        div()
            .w_full()
            .py_4()
            .flex()
            .flex_col()
            .items_start()
            .gap_2()
            .child(label)
            .child(control)
    } else {
        label = label.flex_basis(px(200.)).flex_grow_1().flex_shrink_1();
        control = control.flex_basis(px(220.)).flex_shrink_1().justify_end();
        div()
            .w_full()
            .py_4()
            .flex()
            .flex_wrap()
            .items_start()
            .gap_4()
            .child(label)
            .child(control)
    };

    row.border_b_1()
        .border_color(theme::border())
        .into_any_element()
}

fn settings_icon_button(
    id: impl Into<SharedString>,
    icon: icons::IconName,
    tooltip: String,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let id: SharedString = id.into();
    Button::new(id)
        .size(ButtonSize::Icon(px(30.)))
        .variant(ButtonVariant::Secondary)
        .icon(
            icons::icon(icon, 14.)
                .text_color(theme::muted_text())
                .hover(|style| style.text_color(theme::text())),
        )
        .tooltip(tooltip)
        .on_click(on_click)
        .into_any_element()
}

fn settings_link_button(
    id: &'static str,
    label: String,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    Button::new(id)
        .size(ButtonSize::Small)
        .variant(ButtonVariant::Link)
        .icon(icons::icon(icons::IconName::Link, 14.).text_color(theme::muted_text()))
        .label(label)
        .on_click(on_click)
        .into_any_element()
}

fn update_status_presentation(status: &UpdateStatus) -> (String, gpui::Rgba) {
    match status {
        UpdateStatus::Idle => (
            i18n::text("settings.updates_not_checked"),
            theme::muted_text(),
        ),
        UpdateStatus::Checking => (i18n::text("settings.updates_checking"), theme::info()),
        UpdateStatus::UpToDate => (i18n::text("settings.updates_up_to_date"), theme::accent()),
        UpdateStatus::Available(candidate) => (
            rust_i18n::t!(
                "settings.updates_available_short",
                version = candidate.version.to_string()
            )
            .to_string(),
            theme::info(),
        ),
        UpdateStatus::Downloading(candidate) => (
            rust_i18n::t!(
                "settings.updates_downloading_short",
                version = candidate.version.to_string()
            )
            .to_string(),
            theme::warning(),
        ),
        UpdateStatus::Ready { candidate, .. } => (
            rust_i18n::t!(
                "settings.updates_ready_short",
                version = candidate.version.to_string()
            )
            .to_string(),
            theme::accent(),
        ),
        UpdateStatus::Failed(error) => (
            rust_i18n::t!("settings.updates_failed", error = error).to_string(),
            theme::danger(),
        ),
    }
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
    open_settings_section(shell, SettingsSection::General, cx);
}

/// 打开设置窗口并定位到指定页面（借鉴 Zed：复用已有窗口，否则新开独立窗口）。
/// 延迟到当前帧结束再开窗口，避免在渲染/输入分发中途操作窗口列表。
pub fn open_settings_section(shell: WeakEntity<AppShell>, section: SettingsSection, cx: &mut App) {
    if let Some(window) = cx
        .windows()
        .iter()
        .find_map(|handle| handle.downcast::<SettingsWindow>())
    {
        let _ = window.update(cx, |settings, window, cx| {
            if let Some(shell) = shell.upgrade() {
                settings.updates = shell.read(cx).updates.clone();
            }
            settings.shell = shell;
            settings.select_section(section, cx);
            window.activate_window();
        });
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
                cx.new(|cx| {
                    let mut settings = SettingsWindow::new(shell, cx);
                    settings.section = section;
                    settings
                })
            },
        )
        .ok();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    use crate::features::updates::UpdateSettings;

    struct UpdateStatusObserver {
        _subscription: Subscription,
    }

    #[test]
    fn settings_layout_switches_at_compact_width() {
        assert!(uses_compact_settings_layout(px(639.)));
        assert!(!uses_compact_settings_layout(px(640.)));
    }

    #[gpui::test]
    fn update_notifications_redraw_the_observing_view(cx: &mut gpui::TestAppContext) {
        let redraws = Rc::new(Cell::new(0));
        let (updates, observer) = cx.update(|cx| {
            let updates = cx.new(|_| UpdateController::new(UpdateSettings::default()));
            let observer = cx.new(|cx| UpdateStatusObserver {
                _subscription: observe_update_status(&updates, cx),
            });
            let redraws_for_observer = redraws.clone();
            cx.observe(&observer, move |_, _| {
                redraws_for_observer.set(redraws_for_observer.get() + 1);
            })
            .detach();

            (updates, observer)
        });
        cx.update(|cx| {
            updates.update(cx, |_, cx| cx.notify());
        });

        assert_eq!(redraws.get(), 1);
        drop(observer);
    }
}

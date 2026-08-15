//! 设置窗口：独立的 gpui 窗口（借鉴 Zed 的 `SettingsWindow`）。
//!
//! 设置值以主窗口的 `AppShell` 为准（唯一真源），本窗口只持有其弱引用：
//! 渲染时从 `AppShell` 读取，用户改动通过 `AppShell` 的既有 setter 应用并持久化。
//! 这样终端重放、i18n 全局同步、最近目录同步等副作用都仍由主窗口统一处理。

use gpui::{
    AnyElement, App, AppContext, Bounds, ClickEvent, ClipboardEntry, Context, Entity, FocusHandle,
    FontWeight, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render, SharedString, Size,
    StatefulInteractiveElement, Styled, Subscription, TitlebarOptions, WeakEntity, Window,
    WindowBounds, WindowOptions, canvas, div, px,
};
use std::cell::Cell;
use std::rc::Rc;

use crate::features::settings::{self, SettingsSnapshot};
use crate::features::updates::{UpdateController, UpdateStatus};
use crate::features::workspace::AppShell;
use crate::shared::i18n::{self, LanguagePreference};
use crossh_agent::{AgentModel, AgentModelRef, AgentProtocol, AgentProvider, AgentSettings};
use crossh_ui::widgets::{ime_input_canvas, printable_char, text_caret, text_width};
use crossh_ui::{icons, theme};
use crossh_ui_component::{Button, ButtonSize, ButtonVariant, Stepper, ToggleSwitch};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsSection {
    General,
    Terminal,
    Providers,
    Agent,
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
    agent_draft: AgentSettings,
    agent_draft_initialized: bool,
    agent_provider_index: usize,
    agent_model_index: usize,
    agent_model_editor_open: bool,
    agent_provider_id_focus: FocusHandle,
    agent_provider_name_focus: FocusHandle,
    agent_url_focus: FocusHandle,
    agent_model_focus: FocusHandle,
    agent_model_name_focus: FocusHandle,
    agent_api_key_focus: FocusHandle,
    agent_key_env_focus: FocusHandle,
    agent_context_focus: FocusHandle,
    agent_max_tokens_focus: FocusHandle,
    agent_edit_field: Option<AgentInputField>,
    agent_cursor: usize,
    agent_anchor: Option<usize>,
    agent_ime_marked_text: String,
    agent_ime_replacement: Option<(usize, usize)>,
    agent_dragging: bool,
    agent_error: Option<String>,
    agent_api_key_revealed: bool,
    compact_layout: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AgentInputField {
    ProviderId,
    ProviderName,
    Url,
    Model,
    ModelName,
    ApiKey,
    KeyEnv,
    ContextWindow,
    MaxTokens,
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
            agent_draft: loaded.agent,
            agent_draft_initialized: false,
            agent_provider_index: 0,
            agent_model_index: 0,
            agent_model_editor_open: false,
            agent_provider_id_focus: cx.focus_handle(),
            agent_provider_name_focus: cx.focus_handle(),
            agent_url_focus: cx.focus_handle(),
            agent_model_focus: cx.focus_handle(),
            agent_model_name_focus: cx.focus_handle(),
            agent_api_key_focus: cx.focus_handle(),
            agent_key_env_focus: cx.focus_handle(),
            agent_context_focus: cx.focus_handle(),
            agent_max_tokens_focus: cx.focus_handle(),
            agent_edit_field: None,
            agent_cursor: 0,
            agent_anchor: None,
            agent_ime_marked_text: String::new(),
            agent_ime_replacement: None,
            agent_dragging: false,
            agent_error: None,
            agent_api_key_revealed: false,
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
                    agent: shell.agent_settings.clone(),
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
        if section != SettingsSection::Providers {
            self.agent_model_editor_open = false;
            self.reset_agent_input_state();
        }
        self.scroll.set_offset(gpui::Point::new(px(0.), px(0.)));
        cx.notify();
    }

    fn render_general_settings(
        &self,
        settings: &SettingsSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let compact_layout = self.compact_layout;
        let settings_row = move |label: String, description: String, control: AnyElement| {
            responsive_settings_row(label, description, control, compact_layout)
        };
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
        let compact_layout = self.compact_layout;
        let settings_row = move |label: String, description: String, control: AnyElement| {
            responsive_settings_row(label, description, control, compact_layout)
        };
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
            .max_w(px(760.))
            .flex()
            .flex_col()
            .child(settings_heading("settings.terminal"))
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

    fn render_updates_settings(
        &mut self,
        settings: &SettingsSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let compact_layout = self.compact_layout;
        let settings_row = move |label: String, description: String, control: AnyElement| {
            responsive_settings_row(label, description, control, compact_layout)
        };
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
            .max_w(px(760.))
            .flex()
            .flex_col()
            .child(settings_heading("settings.updates"))
            .child(settings_row(
                i18n::text("settings.updates_check_on_startup"),
                i18n::text("settings.updates_check_on_startup_description"),
                startup_toggle.into_any_element(),
            ))
            .child(settings_row(
                i18n::text("settings.updates_status"),
                i18n::text("settings.updates_status_description"),
                status_control.into_any_element(),
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
                content = content.child(settings_row(
                    rust_i18n::t!("settings.updates_available", version = version).to_string(),
                    if candidate.notes.is_empty() {
                        i18n::text("settings.updates_available_description")
                    } else {
                        candidate.notes
                    },
                    actions.into_any_element(),
                ));
            }
            UpdateStatus::Downloading(candidate) => {
                content = content.child(settings_row(
                    rust_i18n::t!(
                        "settings.updates_downloading",
                        version = candidate.version.to_string()
                    )
                    .to_string(),
                    i18n::text("settings.updates_downloading_description"),
                    div().into_any_element(),
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
                content = content.child(settings_row(
                    rust_i18n::t!(
                        "settings.updates_ready",
                        version = candidate.version.to_string()
                    )
                    .to_string(),
                    package_text,
                    install,
                ));
            }
            _ => {}
        }

        content.into_any_element()
    }

    fn prepare_agent_draft(&mut self, settings: &SettingsSnapshot) {
        if !self.agent_draft_initialized {
            self.agent_draft = settings.agent.clone();
            self.agent_draft_initialized = true;
        }
        if self.agent_draft.providers.is_empty() {
            self.agent_provider_index = 0;
            self.agent_model_index = 0;
            return;
        }
        self.agent_provider_index = self
            .agent_provider_index
            .min(self.agent_draft.providers.len().saturating_sub(1));
        if self.agent_draft.providers[self.agent_provider_index]
            .models
            .is_empty()
        {
            self.agent_draft.providers[self.agent_provider_index]
                .models
                .push(AgentModel {
                    id: "model".into(),
                    name: "Model".into(),
                    reasoning: false,
                    context_window: 128_000,
                    max_tokens: 32_000,
                });
            self.agent_error = Some("At least one model is required".into());
        }
        self.agent_model_index = self.agent_model_index.min(
            self.agent_draft.providers[self.agent_provider_index]
                .models
                .len()
                .saturating_sub(1),
        );
    }

    fn render_provider_settings(
        &mut self,
        settings: &SettingsSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let compact_layout = self.compact_layout;
        self.prepare_agent_draft(settings);
        let provider_index = self.agent_provider_index;

        let add_provider = settings_text_action(
            "settings-agent-provider-add".into(),
            icons::IconName::Plus,
            i18n::text("settings.agent_provider_add"),
            cx.listener(|this, _ev, _window, cx| this.add_agent_provider(cx)),
        );
        let mut provider_rows = div().w_full().flex().flex_col().gap_1();
        for (index, provider) in self.agent_draft.providers.iter().enumerate() {
            let selected = index == provider_index;
            let protocol = provider.protocol.label();
            let row = div()
                .id(format!("settings-agent-provider-row-{index}"))
                .w_full()
                .min_w_0()
                .h(px(48.))
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(theme::RADIUS_SM))
                .border_l_2()
                .border_color(if selected {
                    theme::accent()
                } else {
                    theme::sidebar()
                })
                .cursor_pointer()
                .bg(if selected {
                    theme::accent_soft()
                } else {
                    theme::sidebar()
                })
                .hover(|style| style.bg(theme::raised()))
                .child(
                    icons::icon(icons::IconName::Server, 14.).text_color(if selected {
                        theme::accent()
                    } else {
                        theme::muted_text()
                    }),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(theme::text())
                                .child(SharedString::from(provider.name.clone())),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(theme::muted_text())
                                .child(SharedString::from(protocol)),
                        ),
                )
                .on_click(cx.listener(move |this, _ev, _window, cx| {
                    this.agent_provider_index = index;
                    this.agent_model_index = 0;
                    this.agent_error = None;
                    this.agent_model_editor_open = false;
                    this.reset_agent_input_state();
                    cx.notify();
                }));
            provider_rows = provider_rows.child(row);
        }

        let mut provider_actions = div().flex().items_center().gap_1().child(add_provider);
        if !self.agent_draft.providers.is_empty() {
            provider_actions = provider_actions.child(settings_icon_button(
                "settings-agent-provider-remove",
                icons::IconName::Trash,
                i18n::text("settings.agent_provider_remove"),
                cx.listener(|this, _ev, _window, cx| this.remove_agent_provider(cx)),
            ));
        }
        let provider_header = div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text())
                    .child(SharedString::from(i18n::text("settings.agent_provider"))),
            )
            .child(provider_actions);
        let provider_list = if compact_layout {
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap_2()
                .pb_3()
                .border_b_1()
                .border_color(theme::border())
                .child(provider_header)
                .child(provider_rows)
        } else {
            div()
                .w(px(190.))
                .flex_shrink_0()
                .pr_3()
                .flex()
                .flex_col()
                .gap_2()
                .border_r_1()
                .border_color(theme::border())
                .child(provider_header)
                .child(provider_rows)
        };

        if self.agent_draft.providers.is_empty() {
            let provider_detail =
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .p_6()
                    .child(
                        icons::icon(icons::IconName::Server, 28.).text_color(theme::muted_text()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::text())
                            .child(SharedString::from(i18n::text(
                                "settings.agent_provider_empty",
                            ))),
                    )
                    .child(div().text_xs().text_color(theme::muted_text()).child(
                        SharedString::from(i18n::text("settings.agent_provider_empty_description")),
                    ));
            let provider_detail = if compact_layout {
                provider_detail.w_full().pt_3()
            } else {
                provider_detail.pl_4()
            };
            let body = if compact_layout {
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(provider_list)
                    .child(provider_detail)
            } else {
                div()
                    .w_full()
                    .flex()
                    .items_start()
                    .child(provider_list)
                    .child(provider_detail)
            };
            return div()
                .id("settings-providers")
                .max_w(px(760.))
                .flex()
                .flex_col()
                .child(settings_heading("settings.providers"))
                .child(body)
                .into_any_element();
        }

        let selected_provider = &self.agent_draft.providers[provider_index];
        let provider_id = self.agent_input(AgentInputField::ProviderId, window, cx);
        let provider_name = self.agent_input(AgentInputField::ProviderName, window, cx);
        let url = self.agent_input(AgentInputField::Url, window, cx);
        let api_key = self.agent_input(AgentInputField::ApiKey, window, cx);
        let key_env = self.agent_input(AgentInputField::KeyEnv, window, cx);

        let selected_model =
            &self.agent_draft.providers[provider_index].models[self.agent_model_index];
        let add_model = settings_text_action(
            "settings-agent-model-add".into(),
            icons::IconName::Plus,
            i18n::text("settings.agent_model_add"),
            cx.listener(|this, _ev, _window, cx| this.add_agent_model(cx)),
        );
        let model_actions = div().flex().items_center().gap_1().child(add_model);
        let model_header = div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text())
                            .child(SharedString::from(i18n::text("settings.agent_models"))),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(theme::muted_text())
                            .child(SharedString::from(i18n::text(
                                "settings.agent_models_description",
                            ))),
                    ),
            )
            .child(model_actions);
        let mut model_rows = div()
            .id("settings-agent-model-list")
            .w_full()
            .max_h(px(180.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .border_1()
            .border_color(theme::border())
            .rounded(px(theme::RADIUS_SM));
        for (index, model_entry) in selected_provider.models.iter().enumerate() {
            let selected = index == self.agent_model_index;
            let context_badge = div()
                .h(px(24.))
                .px_2()
                .flex()
                .items_center()
                .rounded(px(theme::RADIUS_SM))
                .bg(theme::raised())
                .text_xs()
                .text_color(theme::muted_text())
                .child(SharedString::from(compact_token_count(
                    model_entry.context_window,
                )));
            let reasoning_mark = div()
                .w(px(16.))
                .h(px(16.))
                .flex()
                .items_center()
                .justify_center();
            let reasoning_mark = if model_entry.reasoning {
                reasoning_mark
                    .child(icons::icon(icons::IconName::Check, 13.).text_color(theme::accent()))
            } else {
                reasoning_mark
            };
            let edit_model = settings_icon_button(
                format!("settings-agent-model-edit-{index}"),
                icons::IconName::Pencil,
                i18n::text("settings.agent_model_edit"),
                cx.listener(move |this, _ev, _window, cx| {
                    cx.stop_propagation();
                    this.agent_model_index = index;
                    this.agent_error = None;
                    this.agent_model_editor_open = true;
                    this.reset_agent_input_state();
                    cx.notify();
                }),
            );
            let remove_model = settings_icon_button(
                format!("settings-agent-model-remove-{index}"),
                icons::IconName::Trash,
                i18n::text("settings.agent_model_remove"),
                cx.listener(move |this, _ev, _window, cx| {
                    this.agent_model_index = index;
                    this.remove_agent_model(cx);
                    cx.stop_propagation();
                }),
            );
            let row = div()
                .id(format!("settings-agent-model-row-{index}"))
                .w_full()
                .min_w_0()
                .h(px(44.))
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(theme::RADIUS_SM))
                .border_l_2()
                .border_color(if selected {
                    theme::accent()
                } else {
                    theme::sidebar()
                })
                .cursor_pointer()
                .bg(if selected {
                    theme::accent_soft()
                } else {
                    theme::sidebar()
                })
                .hover(|style| style.bg(theme::raised()))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(theme::text())
                                .child(SharedString::from(model_entry.name.clone())),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(theme::muted_text())
                                .child(SharedString::from(model_entry.id.clone())),
                        ),
                )
                .child(context_badge)
                .child(reasoning_mark)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(edit_model)
                        .child(remove_model),
                )
                .on_click(cx.listener(move |this, _ev, _window, cx| {
                    this.agent_model_index = index;
                    this.agent_error = None;
                    this.agent_model_editor_open = false;
                    this.reset_agent_input_state();
                    cx.notify();
                }));
            model_rows = model_rows.child(row);
        }
        let reasoning = settings_choice_button(
            "settings-agent-reasoning".into(),
            if selected_model.reasoning {
                i18n::text("settings.agent_reasoning_on")
            } else {
                i18n::text("settings.agent_reasoning_off")
            },
            selected_model.reasoning,
            cx.listener(|this, _ev, _window, cx| {
                let model = &mut this.agent_draft.providers[this.agent_provider_index].models
                    [this.agent_model_index];
                model.reasoning = !model.reasoning;
                this.agent_error = None;
                cx.notify();
            }),
        );
        let status_description = self
            .agent_error
            .clone()
            .unwrap_or_else(|| i18n::text("settings.provider_save_description"));
        let mut protocols = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_row()
            .gap_1()
            .flex_wrap()
            .justify_end();
        for protocol in AgentProtocol::ALL {
            let selected = protocol == self.agent_draft.providers[provider_index].protocol;
            protocols = protocols.child(
                div()
                    .id(format!("settings-agent-protocol-{protocol:?}"))
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
                    .child(SharedString::from(protocol.label()))
                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                        this.agent_draft.providers[this.agent_provider_index].protocol = protocol;
                        this.agent_error = None;
                        cx.notify();
                    })),
            );
        }

        let save = settings_icon_button(
            "settings-provider-save",
            icons::IconName::Save,
            i18n::text("settings.provider_save"),
            cx.listener(|this, _ev, _window, cx| this.save_agent_settings(cx)),
        );
        let provider_title = div()
            .w_full()
            .pb_3()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icons::icon(icons::IconName::Server, 20.).text_color(theme::accent()))
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::text())
                                    .child(SharedString::from(selected_provider.name.clone())),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::muted_text())
                                    .child(SharedString::from(selected_provider.protocol.label())),
                            ),
                    ),
            );
        let status_icon = if self.agent_error.is_some() {
            icons::IconName::CircleX
        } else {
            icons::IconName::Check
        };
        let status = div()
            .w_full()
            .pt_3()
            .flex()
            .items_center()
            .gap_2()
            .border_t_1()
            .border_color(theme::border())
            .child(
                icons::icon(status_icon, 14.).text_color(if self.agent_error.is_some() {
                    theme::danger()
                } else {
                    theme::accent()
                }),
            )
            .child(
                div()
                    .min_w_0()
                    .text_xs()
                    .text_color(if self.agent_error.is_some() {
                        theme::danger()
                    } else {
                        theme::muted_text()
                    })
                    .child(SharedString::from(status_description)),
            );
        let provider_ready = !selected_provider.api_key.trim().is_empty()
            || !selected_provider.api_key_env.trim().is_empty()
            || selected_provider.url.contains("localhost")
            || selected_provider.url.contains("127.0.0.1");
        let provider_status = if provider_ready {
            i18n::text("settings.agent_provider_ready")
        } else {
            i18n::text("settings.agent_provider_unconfigured")
        };
        let provider_status_chip = div()
            .h(px(28.))
            .px_2()
            .flex()
            .items_center()
            .rounded_full()
            .bg(if provider_ready {
                theme::accent_soft()
            } else {
                theme::raised()
            })
            .text_xs()
            .text_color(if provider_ready {
                theme::accent()
            } else {
                theme::muted_text()
            })
            .child(SharedString::from(provider_status));
        let provider_notice = div()
            .w_full()
            .p_3()
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(theme::RADIUS_SM))
            .border_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .child(icons::icon(icons::IconName::Info, 15.).text_color(theme::muted_text()))
            .child(
                div()
                    .min_w_0()
                    .text_xs()
                    .text_color(theme::muted_text())
                    .child(SharedString::from(if provider_ready {
                        i18n::text("settings.agent_provider_ready_notice")
                    } else {
                        i18n::text("settings.agent_provider_key_notice")
                    })),
            );
        let provider_title = provider_title.child(provider_status_chip).child(save);
        let provider_identity = if compact_layout {
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap_3()
                .child(settings_form_field(
                    i18n::text("settings.agent_provider_id"),
                    provider_id,
                ))
                .child(settings_form_field(
                    i18n::text("settings.agent_provider_name"),
                    provider_name,
                ))
        } else {
            div()
                .w_full()
                .flex()
                .gap_3()
                .child(settings_form_field(
                    i18n::text("settings.agent_provider_id"),
                    provider_id,
                ))
                .child(settings_form_field(
                    i18n::text("settings.agent_provider_name"),
                    provider_name,
                ))
        };
        let connection_method = settings_form_field(
            i18n::text("settings.agent_connection_method"),
            protocols.into_any_element(),
        );
        let base_url = settings_form_field(i18n::text("settings.agent_base_url"), url);
        let api_key_visibility = settings_icon_button(
            "settings-agent-api-key-visibility",
            if self.agent_api_key_revealed {
                icons::IconName::EyeOff
            } else {
                icons::IconName::Eye
            },
            if self.agent_api_key_revealed {
                i18n::text("settings.agent_api_key_hide")
            } else {
                i18n::text("settings.agent_api_key_show")
            },
            cx.listener(|this, _ev, _window, cx| this.toggle_api_key_visibility(cx)),
        );
        let api_key_control = div()
            .w_full()
            .flex()
            .items_center()
            .gap_1()
            .child(div().min_w_0().flex_1().child(api_key))
            .child(api_key_visibility);
        let credentials = if compact_layout {
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap_3()
                .child(settings_form_field(
                    i18n::text("settings.agent_credential"),
                    api_key_control.into_any_element(),
                ))
                .child(settings_form_field(
                    i18n::text("settings.agent_credential_env"),
                    key_env,
                ))
        } else {
            div()
                .w_full()
                .flex()
                .gap_3()
                .child(settings_form_field(
                    i18n::text("settings.agent_credential"),
                    api_key_control.into_any_element(),
                ))
                .child(settings_form_field(
                    i18n::text("settings.agent_credential_env"),
                    key_env,
                ))
        };
        let model_editor = if self.agent_model_editor_open {
            let model = self.agent_input(AgentInputField::Model, window, cx);
            let model_name = self.agent_input(AgentInputField::ModelName, window, cx);
            let context_window = self.agent_input(AgentInputField::ContextWindow, window, cx);
            let max_tokens = self.agent_input(AgentInputField::MaxTokens, window, cx);
            let close_editor = settings_icon_button(
                "settings-agent-model-editor-close",
                icons::IconName::X,
                i18n::text("settings.agent_model_editor_close"),
                cx.listener(|this, _ev, _window, cx| {
                    this.agent_model_editor_open = false;
                    this.reset_agent_input_state();
                    cx.notify();
                }),
            );
            let editor_header = div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::text())
                        .child(SharedString::from(i18n::text("settings.agent_model_edit"))),
                )
                .child(close_editor);
            Some(
                div()
                    .w_full()
                    .pt_3()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(editor_header)
                    .child(settings_form_field(
                        i18n::text("settings.agent_model_id"),
                        model,
                    ))
                    .child(settings_form_field(
                        i18n::text("settings.agent_model_name"),
                        model_name,
                    ))
                    .child(if compact_layout {
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(settings_form_field(
                                i18n::text("settings.agent_reasoning"),
                                reasoning,
                            ))
                            .child(settings_form_field(
                                i18n::text("settings.agent_context_window"),
                                context_window,
                            ))
                            .child(settings_form_field(
                                i18n::text("settings.agent_max_tokens"),
                                max_tokens,
                            ))
                    } else {
                        div()
                            .w_full()
                            .flex()
                            .gap_3()
                            .child(settings_form_field(
                                i18n::text("settings.agent_reasoning"),
                                reasoning,
                            ))
                            .child(settings_form_field(
                                i18n::text("settings.agent_context_window"),
                                context_window,
                            ))
                            .child(settings_form_field(
                                i18n::text("settings.agent_max_tokens"),
                                max_tokens,
                            ))
                    }),
            )
        } else {
            None
        };
        let mut provider_detail = div()
            .min_w_0()
            .flex_1()
            .flex()
            .flex_col()
            .gap_3()
            .child(provider_title)
            .child(provider_notice)
            .child(settings_subheading("settings.agent_connection"))
            .child(provider_identity)
            .child(connection_method)
            .child(base_url)
            .child(credentials)
            .child(settings_subheading("settings.agent_models"))
            .child(model_header)
            .child(model_rows);
        if let Some(model_editor) = model_editor {
            provider_detail = provider_detail.child(model_editor);
        }
        provider_detail = provider_detail.child(status);
        if compact_layout {
            provider_detail = provider_detail.w_full().pt_3();
        } else {
            provider_detail = provider_detail.pl_4();
        }

        let body = if compact_layout {
            div()
                .w_full()
                .flex()
                .flex_col()
                .child(provider_list)
                .child(provider_detail)
        } else {
            div()
                .w_full()
                .flex()
                .items_start()
                .child(provider_list)
                .child(provider_detail)
        };

        div()
            .id("settings-providers")
            .max_w(px(760.))
            .flex()
            .flex_col()
            .child(settings_heading("settings.providers"))
            .child(body)
            .into_any_element()
    }
}

#[path = "agent.rs"]
mod agent;
#[path = "input.rs"]
mod input;
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

fn settings_subheading(key: &str) -> AnyElement {
    div()
        .pt_2()
        .pb_1()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::text())
        .child(SharedString::from(i18n::text(key)))
        .into_any_element()
}

fn settings_form_field(label: String, control: AnyElement) -> AnyElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_sm()
                .text_color(theme::muted_text())
                .child(SharedString::from(label)),
        )
        .child(control)
        .into_any_element()
}

fn settings_text_action(
    id: String,
    icon: icons::IconName,
    label: String,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .h(px(30.))
        .px_2()
        .flex()
        .items_center()
        .gap_1()
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .bg(theme::raised())
        .text_xs()
        .text_color(theme::muted_text())
        .hover(|style| style.bg(theme::accent()).text_color(theme::canvas()))
        .child(icons::icon(icon, 14.).text_color(theme::muted_text()))
        .child(SharedString::from(label))
        .on_click(on_click)
        .into_any_element()
}

fn compact_token_count(value: u32) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f32 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{}K", value / 1_000)
    } else {
        value.to_string()
    }
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

fn settings_choice_button(
    id: String,
    label: String,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    Button::new(id)
        .size(ButtonSize::Small)
        .variant(if selected {
            ButtonVariant::Primary
        } else {
            ButtonVariant::Secondary
        })
        .selected(selected)
        .label(label)
        .on_click(on_click)
        .into_any_element()
}

fn input_text_part(field: AgentInputField, value: &str, mask_api_key: bool) -> AnyElement {
    let display = if field == AgentInputField::ApiKey && mask_api_key {
        "*".repeat(value.chars().count())
    } else {
        value.to_string()
    };
    div()
        .flex_shrink_0()
        .whitespace_nowrap()
        .child(SharedString::from(display))
        .into_any_element()
}

fn input_index_for_x(
    window: &Window,
    value: &str,
    masked: bool,
    x: Pixels,
    bounds: Bounds<Pixels>,
) -> usize {
    let relative_x = (x - bounds.origin.x - px(8.)).max(px(0.));
    let display = if masked {
        "*".repeat(value.chars().count())
    } else {
        value.to_string()
    };
    let mut previous_width = px(0.);
    for (character_index, (display_end, _)) in display
        .char_indices()
        .map(|(index, ch)| (index + ch.len_utf8(), ch))
        .enumerate()
    {
        let width = text_width(window, &display[..display_end], px(12.));
        if relative_x < previous_width + (width - previous_width) / 2. {
            return value
                .char_indices()
                .nth(character_index)
                .map(|(index, _)| index)
                .unwrap_or(value.len());
        }
        previous_width = width;
    }
    value.len()
}

fn clamp_char_boundary(value: &str, index: usize) -> usize {
    let mut index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn selection_bounds(anchor: Option<usize>, cursor: usize) -> Option<(usize, usize)> {
    let anchor = anchor?;
    (anchor != cursor).then_some(if anchor < cursor {
        (anchor, cursor)
    } else {
        (cursor, anchor)
    })
}

fn previous_char_boundary(value: &str, cursor: usize) -> usize {
    value[..cursor]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn next_char_boundary(value: &str, cursor: usize) -> usize {
    value[cursor..]
        .chars()
        .next()
        .map(|character| cursor + character.len_utf8())
        .unwrap_or(cursor)
}

fn settings_link_button(
    id: &'static str,
    label: String,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .h(px(30.))
        .px_2()
        .flex()
        .items_center()
        .gap_1()
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .border_1()
        .border_color(theme::border())
        .bg(theme::raised())
        .text_xs()
        .text_color(theme::muted_text())
        .hover(|s| s.bg(theme::accent()).text_color(theme::canvas()))
        .child(
            icons::icon(icons::IconName::Link, 14.)
                .text_color(theme::muted_text())
                .hover(|s| s.text_color(theme::canvas())),
        )
        .child(SharedString::from(label))
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

    #[test]
    fn input_cursor_is_clamped_to_a_valid_utf8_boundary() {
        assert_eq!(clamp_char_boundary("model", 99), 5);
        assert_eq!(clamp_char_boundary("模型", 2), 0);
        assert_eq!(clamp_char_boundary("模型", 3), 3);
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

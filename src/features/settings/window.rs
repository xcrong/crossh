//! 设置窗口：独立的 gpui 窗口（借鉴 Zed 的 `SettingsWindow`）。
//!
//! 设置值以主窗口的 `AppShell` 为准（唯一真源），本窗口只持有其弱引用：
//! 渲染时从 `AppShell` 读取，用户改动通过 `AppShell` 的既有 setter 应用并持久化。
//! 这样终端重放、i18n 全局同步、最近目录同步等副作用都仍由主窗口统一处理。

use gpui::{
    AnyElement, App, AppContext, Bounds, ClickEvent, ClipboardEntry, Context, Entity, FocusHandle,
    FontWeight, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render, SharedString, Size,
    StatefulInteractiveElement, Styled, TitlebarOptions, WeakEntity, Window, WindowBounds,
    WindowOptions, canvas, div, px,
};
use std::cell::Cell;
use std::rc::Rc;

use crate::features::settings::{self, SettingsSnapshot};
use crate::features::updates::{UpdateController, UpdateStatus};
use crate::features::workspace::AppShell;
use crate::shared::i18n::{self, LanguagePreference};
use crossh_agent::{AgentModel, AgentModelRef, AgentProtocol, AgentProvider, AgentSettings};
use crossh_ui::widgets::{LocalPathTooltip, printable_char, text_caret, text_width};
use crossh_ui::{icons, theme};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsSection {
    General,
    Terminal,
    Agent,
    Updates,
    About,
}

/// 设置窗口的根视图。窗口关闭即释放。
pub struct SettingsWindow {
    /// 主窗口 AppShell 的弱引用：设置值读写都委托给它。
    shell: WeakEntity<AppShell>,
    section: SettingsSection,
    scroll: gpui::ScrollHandle,
    updates: Entity<UpdateController>,
    agent_draft: AgentSettings,
    agent_provider_index: usize,
    agent_model_index: usize,
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
    agent_dragging: bool,
    agent_error: Option<String>,
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
        Self {
            shell,
            section: SettingsSection::General,
            scroll: gpui::ScrollHandle::new(),
            updates,
            agent_draft: loaded.agent,
            agent_provider_index: 0,
            agent_model_index: 0,
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
            agent_dragging: false,
            agent_error: None,
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

    fn render_updates_settings(
        &mut self,
        settings: &SettingsSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.updates.update(cx, |updates, _cx| {
            updates.set_settings(settings.updates.clone())
        });
        let status = self
            .updates
            .read_with(cx, |updates, _app| updates.status().clone());

        let mut startup_toggle = div()
            .id("settings-updates-startup-toggle")
            .w(px(42.))
            .h(px(24.))
            .p_1()
            .flex()
            .items_center()
            .rounded_full()
            .cursor_pointer()
            .bg(if settings.updates.check_on_startup {
                theme::accent()
            } else {
                theme::border_strong()
            });
        startup_toggle = if settings.updates.check_on_startup {
            startup_toggle.justify_end()
        } else {
            startup_toggle.justify_start()
        };
        startup_toggle = startup_toggle.child(
            div()
                .w(px(18.))
                .h(px(18.))
                .rounded_full()
                .bg(theme::canvas()),
        );
        startup_toggle = startup_toggle.on_click(cx.listener(|this, _ev, _window, cx| {
            let enabled = !this.shell_settings(cx).updates.check_on_startup;
            this.write_to_shell(cx, |shell, cx| {
                shell.set_update_check_on_startup(enabled, cx)
            });
        }));

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

    fn render_agent_settings(
        &mut self,
        settings: &SettingsSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.agent_draft == AgentSettings::default()
            && settings.agent != AgentSettings::default()
        {
            self.agent_draft = settings.agent.clone();
        }
        if self.agent_draft.providers.is_empty() {
            self.agent_draft = AgentSettings::default();
            self.agent_error = Some("At least one provider is required".into());
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
        let provider_id = self.agent_input(AgentInputField::ProviderId, window, cx);
        let provider_name = self.agent_input(AgentInputField::ProviderName, window, cx);
        let url = self.agent_input(AgentInputField::Url, window, cx);
        let model = self.agent_input(AgentInputField::Model, window, cx);
        let model_name = self.agent_input(AgentInputField::ModelName, window, cx);
        let api_key = self.agent_input(AgentInputField::ApiKey, window, cx);
        let key_env = self.agent_input(AgentInputField::KeyEnv, window, cx);
        let context_window = self.agent_input(AgentInputField::ContextWindow, window, cx);
        let max_tokens = self.agent_input(AgentInputField::MaxTokens, window, cx);
        let provider_index = self.agent_provider_index;
        let mut providers = div().flex().flex_row().gap_1().flex_wrap().justify_end();
        for (index, provider) in self.agent_draft.providers.iter().enumerate() {
            providers = providers.child(settings_choice_button(
                format!("settings-agent-provider-{index}"),
                provider.name.clone(),
                index == provider_index,
                cx.listener(move |this, _ev, _window, cx| {
                    this.agent_provider_index = index;
                    this.agent_model_index = 0;
                    this.agent_error = None;
                    cx.notify();
                }),
            ));
        }
        providers = providers
            .child(settings_icon_button(
                "settings-agent-provider-add",
                icons::IconName::Plus,
                i18n::text("settings.agent_provider_add"),
                cx.listener(|this, _ev, _window, cx| this.add_agent_provider(cx)),
            ))
            .child(settings_icon_button(
                "settings-agent-provider-remove",
                icons::IconName::Trash,
                i18n::text("settings.agent_provider_remove"),
                cx.listener(|this, _ev, _window, cx| this.remove_agent_provider(cx)),
            ));

        let selected_provider = &self.agent_draft.providers[provider_index];
        let mut models = div().flex().flex_row().gap_1().flex_wrap().justify_end();
        for (index, model_entry) in selected_provider.models.iter().enumerate() {
            models = models.child(settings_choice_button(
                format!("settings-agent-model-choice-{index}"),
                model_entry.name.clone(),
                index == self.agent_model_index,
                cx.listener(move |this, _ev, _window, cx| {
                    this.agent_model_index = index;
                    this.agent_error = None;
                    cx.notify();
                }),
            ));
        }
        models = models
            .child(settings_icon_button(
                "settings-agent-model-add",
                icons::IconName::Plus,
                i18n::text("settings.agent_model_add"),
                cx.listener(|this, _ev, _window, cx| this.add_agent_model(cx)),
            ))
            .child(settings_icon_button(
                "settings-agent-model-remove",
                icons::IconName::Trash,
                i18n::text("settings.agent_model_remove"),
                cx.listener(|this, _ev, _window, cx| this.remove_agent_model(cx)),
            ));

        let selected_model =
            &self.agent_draft.providers[provider_index].models[self.agent_model_index];
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
        let mut active_models = div().flex().flex_row().gap_1().flex_wrap().justify_end();
        let mut reviewer_models = div().flex().flex_row().gap_1().flex_wrap().justify_end();
        for provider in &self.agent_draft.providers {
            for model_entry in &provider.models {
                let reference = AgentModelRef {
                    provider: provider.id.clone(),
                    model: model_entry.id.clone(),
                };
                let label = format!("{}/{}", provider.name, model_entry.name);
                let active_reference = reference.clone();
                active_models = active_models.child(settings_choice_button(
                    format!("settings-agent-active-{}-{}", provider.id, model_entry.id),
                    label.clone(),
                    self.agent_draft.active_model == reference,
                    cx.listener(move |this, _ev, _window, cx| {
                        this.agent_draft.active_model = active_reference.clone();
                        this.agent_error = None;
                        cx.notify();
                    }),
                ));
                let reviewer_reference = reference.clone();
                reviewer_models = reviewer_models.child(settings_choice_button(
                    format!("settings-agent-reviewer-{}-{}", provider.id, model_entry.id),
                    label,
                    self.agent_draft.reviewer_model == reference,
                    cx.listener(move |this, _ev, _window, cx| {
                        this.agent_draft.reviewer_model = reviewer_reference.clone();
                        this.agent_error = None;
                        cx.notify();
                    }),
                ));
            }
        }
        let rounds = self.agent_draft.max_tool_rounds;
        let rounds_control = div()
            .flex()
            .items_center()
            .gap_1()
            .child(settings_icon_button(
                "settings-agent-rounds-decrease",
                icons::IconName::Minus,
                i18n::text("settings.agent_rounds"),
                cx.listener(|this, _ev, _window, cx| {
                    this.agent_draft.max_tool_rounds =
                        this.agent_draft.max_tool_rounds.saturating_sub(10).max(1);
                    cx.notify();
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
                    .child(SharedString::from(rounds.to_string())),
            )
            .child(settings_icon_button(
                "settings-agent-rounds-increase",
                icons::IconName::Plus,
                i18n::text("settings.agent_rounds"),
                cx.listener(|this, _ev, _window, cx| {
                    this.agent_draft.max_tool_rounds =
                        (this.agent_draft.max_tool_rounds + 10).min(1000);
                    cx.notify();
                }),
            ));
        let save = settings_icon_button(
            "settings-agent-save",
            icons::IconName::Save,
            i18n::text("settings.agent_save"),
            cx.listener(|this, _ev, _window, cx| this.save_agent_settings(cx)),
        );
        let status_description = self
            .agent_error
            .clone()
            .unwrap_or_else(|| i18n::text("settings.agent_save_description"));
        let mut protocols = div().flex().flex_row().gap_1().flex_wrap().justify_end();
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

        div()
            .id("settings-agent")
            .max_w(px(760.))
            .flex()
            .flex_col()
            .child(settings_heading("settings.agent"))
            .child(settings_row(
                i18n::text("settings.agent_provider"),
                i18n::text("settings.agent_provider_description"),
                providers.into_any_element(),
            ))
            .child(settings_row(
                i18n::text("settings.agent_provider_id"),
                i18n::text("settings.agent_provider_id_description"),
                provider_id,
            ))
            .child(settings_row(
                i18n::text("settings.agent_provider_name"),
                i18n::text("settings.agent_provider_name_description"),
                provider_name,
            ))
            .child(settings_row(
                i18n::text("settings.agent_protocol"),
                i18n::text("settings.agent_protocol_description"),
                protocols.into_any_element(),
            ))
            .child(settings_row(
                i18n::text("settings.agent_url"),
                i18n::text("settings.agent_url_description"),
                url,
            ))
            .child(settings_row(
                i18n::text("settings.agent_models"),
                i18n::text("settings.agent_models_description"),
                models.into_any_element(),
            ))
            .child(settings_row(
                i18n::text("settings.agent_model_id"),
                i18n::text("settings.agent_model_id_description"),
                model,
            ))
            .child(settings_row(
                i18n::text("settings.agent_model_name"),
                i18n::text("settings.agent_model_name_description"),
                model_name,
            ))
            .child(settings_row(
                i18n::text("settings.agent_reasoning"),
                i18n::text("settings.agent_reasoning_description"),
                reasoning,
            ))
            .child(settings_row(
                i18n::text("settings.agent_context_window"),
                i18n::text("settings.agent_context_window_description"),
                context_window,
            ))
            .child(settings_row(
                i18n::text("settings.agent_max_tokens"),
                i18n::text("settings.agent_max_tokens_description"),
                max_tokens,
            ))
            .child(settings_row(
                i18n::text("settings.agent_active_model"),
                i18n::text("settings.agent_active_model_description"),
                active_models.into_any_element(),
            ))
            .child(settings_row(
                i18n::text("settings.agent_reviewer_model"),
                i18n::text("settings.agent_reviewer_model_description"),
                reviewer_models.into_any_element(),
            ))
            .child(settings_row(
                i18n::text("settings.agent_rounds"),
                i18n::text("settings.agent_rounds_description"),
                rounds_control.into_any_element(),
            ))
            .child(settings_row(
                i18n::text("settings.agent_credential"),
                i18n::text("settings.agent_credential_description"),
                api_key,
            ))
            .child(settings_row(
                i18n::text("settings.agent_credential_env"),
                i18n::text("settings.agent_credential_env_description"),
                key_env,
            ))
            .child(settings_row(
                i18n::text("settings.agent_status"),
                status_description,
                save,
            ))
            .into_any_element()
    }

    fn agent_input(
        &self,
        field: AgentInputField,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (id, focus) = match field {
            AgentInputField::ProviderId => {
                ("settings-agent-provider-id", &self.agent_provider_id_focus)
            }
            AgentInputField::ProviderName => (
                "settings-agent-provider-name",
                &self.agent_provider_name_focus,
            ),
            AgentInputField::Url => ("settings-agent-url", &self.agent_url_focus),
            AgentInputField::Model => ("settings-agent-model", &self.agent_model_focus),
            AgentInputField::ModelName => {
                ("settings-agent-model-name", &self.agent_model_name_focus)
            }
            AgentInputField::ApiKey => ("settings-agent-api-key", &self.agent_api_key_focus),
            AgentInputField::KeyEnv => ("settings-agent-key-env", &self.agent_key_env_focus),
            AgentInputField::ContextWindow => ("settings-agent-context", &self.agent_context_focus),
            AgentInputField::MaxTokens => {
                ("settings-agent-max-tokens", &self.agent_max_tokens_focus)
            }
        };
        let value = self.agent_input_value(field);
        let focus = focus.clone();
        let focused = focus.is_focused(window);
        let active = focused && self.agent_edit_field == Some(field);
        let cursor = if active {
            self.agent_cursor.min(value.len())
        } else {
            value.len()
        };
        let selection = if active {
            selection_bounds(self.agent_anchor, cursor)
        } else {
            None
        };
        let (start, end) = selection.unwrap_or((cursor, cursor));
        let bounds: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
        let entity = cx.entity();
        let tracking = canvas(
            {
                let bounds = bounds.clone();
                move |input_bounds, _window, _cx| bounds.set(Some(input_bounds))
            },
            {
                let bounds = bounds.clone();
                let entity = entity.clone();
                move |_bounds, _state, window, _cx| {
                    window.on_mouse_event({
                        let bounds = bounds.clone();
                        let entity = entity.clone();
                        move |event: &MouseMoveEvent, _phase, window, cx| {
                            let Some(input_bounds) = bounds.get() else {
                                return;
                            };
                            if !entity.read(cx).agent_dragging
                                || entity.read(cx).agent_edit_field != Some(field)
                            {
                                return;
                            }
                            let value = entity.read(cx).agent_input_value(field);
                            let index = input_index_for_x(
                                window,
                                &value,
                                field == AgentInputField::ApiKey,
                                event.position.x,
                                input_bounds,
                            );
                            entity.update(cx, |this, cx| {
                                this.agent_cursor = index;
                                cx.notify();
                            });
                        }
                    });
                    window.on_mouse_event({
                        let entity = entity.clone();
                        move |_event: &MouseUpEvent, _phase, _window, cx| {
                            if entity.read(cx).agent_dragging {
                                entity.update(cx, |this, _cx| this.agent_dragging = false);
                            }
                        }
                    });
                }
            },
        )
        .absolute()
        .left_0()
        .top_0()
        .size_full();
        let mut input = div()
            .id(id)
            .w(px(300.))
            .h(px(32.))
            .px_2()
            .flex()
            .items_center()
            .overflow_hidden()
            .rounded(px(theme::RADIUS_SM))
            .border_1()
            .border_color(theme::border_strong())
            .focus(|style| style.border_color(theme::focus_ring()))
            .bg(theme::raised())
            .text_xs()
            .text_color(theme::text())
            .track_focus(&focus)
            .tab_stop(true)
            .relative()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener({
                    let focus = focus.clone();
                    let bounds = bounds.clone();
                    move |this, event: &MouseDownEvent, window, cx| {
                        window.focus(&focus, cx);
                        let value = this.agent_input_value(field);
                        let index = bounds
                            .get()
                            .map(|input_bounds| {
                                input_index_for_x(
                                    window,
                                    &value,
                                    field == AgentInputField::ApiKey,
                                    event.position.x,
                                    input_bounds,
                                )
                            })
                            .unwrap_or(value.len());
                        if this.agent_edit_field != Some(field) || !event.modifiers.shift {
                            this.agent_anchor = Some(index);
                        }
                        this.agent_edit_field = Some(field);
                        this.agent_cursor = index;
                        this.agent_dragging = true;
                        cx.notify();
                    }
                }),
            )
            .on_key_down(cx.listener(move |this, event, _window, cx| {
                this.handle_agent_input_key(field, event, cx)
            }));
        input = input.child(input_text_part(field, &value[..start]));
        if selection.is_some() {
            input = input.child(
                div()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .bg(theme::selection())
                    .child(input_text_part(field, &value[start..end])),
            );
        } else if focused {
            input = input.child(text_caret(px(16.)));
        }
        input
            .child(input_text_part(field, &value[end..]))
            .child(tracking)
            .into_any_element()
    }

    fn handle_agent_input_key(
        &mut self,
        field: AgentInputField,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.activate_agent_input(field);
        let provider_index = self.agent_provider_index;
        let model_index = self.agent_model_index;
        let old_model_id = self.agent_draft.providers[provider_index].models[model_index]
            .id
            .clone();
        let old_provider_id = self.agent_draft.providers[provider_index].id.clone();
        let key = &event.keystroke;
        let primary = key.modifiers.control || key.modifiers.platform;
        let extend = key.modifiers.shift;
        if primary && key.key == "a" {
            self.agent_anchor = Some(0);
            self.agent_cursor = self.agent_input_value(field).len();
        } else if primary && matches!(key.key.as_str(), "c" | "x") {
            if let Some((start, end)) = selection_bounds(self.agent_anchor, self.agent_cursor) {
                let value = self.agent_input_value(field);
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(value[start..end].into()));
                if key.key == "x" {
                    self.replace_agent_selection(field, "");
                }
            }
        } else if primary && key.key == "v" {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| {
                item.into_entries().find_map(|entry| match entry {
                    ClipboardEntry::String(value) => Some(value.text),
                    _ => None,
                })
            }) {
                if matches!(
                    field,
                    AgentInputField::ContextWindow | AgentInputField::MaxTokens
                ) {
                    let digits = text
                        .chars()
                        .filter(char::is_ascii_digit)
                        .collect::<String>();
                    if !digits.is_empty() {
                        self.replace_agent_selection(field, &digits);
                    }
                } else {
                    self.replace_agent_selection(field, &text);
                }
            }
        } else if matches!(key.key.as_str(), "enter" | "return") {
            self.save_agent_settings(cx);
            return;
        } else if key.key == "backspace" {
            self.agent_backspace(field);
        } else if key.key == "delete" {
            self.agent_delete(field);
        } else if matches!(key.key.as_str(), "left" | "right") {
            self.move_agent_cursor(field, key.key == "right", extend);
        } else if matches!(key.key.as_str(), "home" | "end") {
            if extend && self.agent_anchor.is_none() {
                self.agent_anchor = Some(self.agent_cursor);
            }
            self.agent_cursor = if key.key == "end" {
                self.agent_input_value(field).len()
            } else {
                0
            };
            if !extend {
                self.agent_anchor = None;
            }
        } else if let Some(character) = printable_char(key)
            && (!matches!(
                field,
                AgentInputField::ContextWindow | AgentInputField::MaxTokens
            ) || character.is_ascii_digit())
        {
            if matches!(
                field,
                AgentInputField::ContextWindow | AgentInputField::MaxTokens
            ) && self.agent_input_value(field) == "0"
            {
                self.agent_anchor = Some(0);
                self.agent_cursor = 1;
            }
            self.replace_agent_selection(field, &character.to_string());
        }
        if matches!(field, AgentInputField::Model) {
            let provider_id = self.agent_draft.providers[provider_index].id.clone();
            let new_model_id = self.agent_draft.providers[provider_index].models[model_index]
                .id
                .clone();
            for reference in [
                &mut self.agent_draft.active_model,
                &mut self.agent_draft.reviewer_model,
            ] {
                if reference.provider == provider_id && reference.model == old_model_id {
                    reference.model = new_model_id.clone();
                }
            }
        }
        if matches!(field, AgentInputField::ProviderId) {
            let new_provider_id = self.agent_draft.providers[provider_index].id.clone();
            for reference in [
                &mut self.agent_draft.active_model,
                &mut self.agent_draft.reviewer_model,
            ] {
                if reference.provider == old_provider_id {
                    reference.provider = new_provider_id.clone();
                }
            }
        }
        self.agent_error = None;
        cx.notify();
    }

    fn activate_agent_input(&mut self, field: AgentInputField) {
        if self.agent_edit_field != Some(field) {
            self.agent_edit_field = Some(field);
            self.agent_cursor = self.agent_input_value(field).len();
            self.agent_anchor = None;
        } else {
            self.agent_cursor = self.agent_cursor.min(self.agent_input_value(field).len());
        }
    }

    fn agent_input_value(&self, field: AgentInputField) -> String {
        let provider = &self.agent_draft.providers[self.agent_provider_index];
        let model = &provider.models[self.agent_model_index];
        match field {
            AgentInputField::ProviderId => provider.id.clone(),
            AgentInputField::ProviderName => provider.name.clone(),
            AgentInputField::Url => provider.url.clone(),
            AgentInputField::Model => model.id.clone(),
            AgentInputField::ModelName => model.name.clone(),
            AgentInputField::ApiKey => provider.api_key.clone(),
            AgentInputField::KeyEnv => provider.api_key_env.clone(),
            AgentInputField::ContextWindow => model.context_window.to_string(),
            AgentInputField::MaxTokens => model.max_tokens.to_string(),
        }
    }

    fn set_agent_input_value(&mut self, field: AgentInputField, value: String) {
        let provider = &mut self.agent_draft.providers[self.agent_provider_index];
        let model = &mut provider.models[self.agent_model_index];
        match field {
            AgentInputField::ProviderId => provider.id = value,
            AgentInputField::ProviderName => provider.name = value,
            AgentInputField::Url => provider.url = value,
            AgentInputField::Model => model.id = value,
            AgentInputField::ModelName => model.name = value,
            AgentInputField::ApiKey => provider.api_key = value,
            AgentInputField::KeyEnv => provider.api_key_env = value,
            AgentInputField::ContextWindow => {
                model.context_window = value.parse().unwrap_or(0);
            }
            AgentInputField::MaxTokens => {
                model.max_tokens = value.parse().unwrap_or(0);
            }
        }
    }

    fn replace_agent_selection(&mut self, field: AgentInputField, replacement: &str) {
        let mut value = self.agent_input_value(field);
        let (start, end) = selection_bounds(self.agent_anchor, self.agent_cursor)
            .unwrap_or((self.agent_cursor, self.agent_cursor));
        value.replace_range(start..end, replacement);
        self.agent_cursor = start + replacement.len();
        self.agent_anchor = None;
        self.set_agent_input_value(field, value);
    }

    fn agent_backspace(&mut self, field: AgentInputField) {
        if selection_bounds(self.agent_anchor, self.agent_cursor).is_some() {
            self.replace_agent_selection(field, "");
            return;
        }
        let value = self.agent_input_value(field);
        let start = previous_char_boundary(&value, self.agent_cursor);
        if start != self.agent_cursor {
            self.agent_anchor = Some(start);
            self.replace_agent_selection(field, "");
        }
    }

    fn agent_delete(&mut self, field: AgentInputField) {
        if selection_bounds(self.agent_anchor, self.agent_cursor).is_some() {
            self.replace_agent_selection(field, "");
            return;
        }
        let value = self.agent_input_value(field);
        let end = next_char_boundary(&value, self.agent_cursor);
        if end != self.agent_cursor {
            self.agent_anchor = Some(end);
            self.replace_agent_selection(field, "");
        }
    }

    fn move_agent_cursor(&mut self, field: AgentInputField, right: bool, extend: bool) {
        if !extend
            && let Some((start, end)) = selection_bounds(self.agent_anchor, self.agent_cursor)
        {
            self.agent_cursor = if right { end } else { start };
            self.agent_anchor = None;
            return;
        }
        if extend && self.agent_anchor.is_none() {
            self.agent_anchor = Some(self.agent_cursor);
        }
        let value = self.agent_input_value(field);
        self.agent_cursor = if right {
            next_char_boundary(&value, self.agent_cursor)
        } else {
            previous_char_boundary(&value, self.agent_cursor)
        };
        if !extend {
            self.agent_anchor = None;
        }
    }

    fn add_agent_provider(&mut self, cx: &mut Context<Self>) {
        let number = self.agent_draft.providers.len() + 1;
        let id = format!("provider-{number}");
        self.agent_draft.providers.push(AgentProvider {
            id: id.clone(),
            name: format!("Provider {number}"),
            protocol: AgentProtocol::OpenAiChat,
            url: "http://127.0.0.1:11434/v1/chat/completions".into(),
            api_key_env: String::new(),
            api_key: String::new(),
            models: vec![AgentModel {
                id: "model".into(),
                name: "Model".into(),
                reasoning: false,
                context_window: 128_000,
                max_tokens: 32_000,
            }],
        });
        self.agent_provider_index = self.agent_draft.providers.len() - 1;
        self.agent_model_index = 0;
        self.agent_error = None;
        cx.notify();
    }

    fn remove_agent_provider(&mut self, cx: &mut Context<Self>) {
        if self.agent_draft.providers.len() <= 1 {
            self.agent_error = Some("At least one provider is required".into());
            cx.notify();
            return;
        }
        let removed = self.agent_draft.providers.remove(self.agent_provider_index);
        self.agent_provider_index = self
            .agent_provider_index
            .min(self.agent_draft.providers.len() - 1);
        self.agent_model_index = 0;
        let fallback = &self.agent_draft.providers[self.agent_provider_index];
        let fallback_ref = AgentModelRef {
            provider: fallback.id.clone(),
            model: fallback.models[0].id.clone(),
        };
        if self.agent_draft.active_model.provider == removed.id {
            self.agent_draft.active_model = fallback_ref.clone();
        }
        if self.agent_draft.reviewer_model.provider == removed.id {
            self.agent_draft.reviewer_model = fallback_ref;
        }
        self.agent_error = None;
        cx.notify();
    }

    fn add_agent_model(&mut self, cx: &mut Context<Self>) {
        let provider = &mut self.agent_draft.providers[self.agent_provider_index];
        let number = provider.models.len() + 1;
        provider.models.push(AgentModel {
            id: format!("model-{number}"),
            name: format!("Model {number}"),
            reasoning: false,
            context_window: 128_000,
            max_tokens: 32_000,
        });
        self.agent_model_index = provider.models.len() - 1;
        self.agent_error = None;
        cx.notify();
    }

    fn remove_agent_model(&mut self, cx: &mut Context<Self>) {
        let provider = &mut self.agent_draft.providers[self.agent_provider_index];
        if provider.models.len() <= 1 {
            self.agent_error = Some("At least one model is required".into());
            cx.notify();
            return;
        }
        let removed = provider.models.remove(self.agent_model_index);
        self.agent_model_index = self.agent_model_index.min(provider.models.len() - 1);
        let fallback = AgentModelRef {
            provider: provider.id.clone(),
            model: provider.models[self.agent_model_index].id.clone(),
        };
        for reference in [
            &mut self.agent_draft.active_model,
            &mut self.agent_draft.reviewer_model,
        ] {
            if reference.provider == provider.id && reference.model == removed.id {
                *reference = fallback.clone();
            }
        }
        self.agent_error = None;
        cx.notify();
    }

    fn save_agent_settings(&mut self, cx: &mut Context<Self>) {
        let settings = self.agent_draft.clone().normalized();
        if let Err(error) = settings.validate() {
            self.agent_error = Some(error.to_string());
            cx.notify();
            return;
        }
        self.agent_draft = settings.clone();
        self.agent_error = None;
        self.write_to_shell(cx, |shell, cx| shell.set_agent_settings(settings, cx));
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
            .max_w(px(760.))
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
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .justify_center()
                            .rounded(px(theme::RADIUS_MD))
                            .bg(theme::accent_soft())
                            .child(
                                icons::icon(icons::IconName::Info, 30.).text_color(theme::accent()),
                            ),
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
            .child(settings_row(
                i18n::text("settings.about_version"),
                i18n::text("settings.about_version_description"),
                version.into_any_element(),
            ))
            .child(settings_row(
                i18n::text("settings.about_source"),
                i18n::text("settings.about_source_description"),
                source,
            ))
            .child(settings_row(
                i18n::text("settings.about_license"),
                i18n::text("settings.about_license_description"),
                license,
            ))
            .into_any_element()
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        let agent = nav_button(
            "settings-section-agent",
            icons::IconName::Terminal,
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

        let content = match section {
            SettingsSection::General => self.render_general_settings(&settings, cx),
            SettingsSection::Terminal => self.render_terminal_settings(&settings, cx),
            SettingsSection::Agent => self.render_agent_settings(&settings, window, cx),
            SettingsSection::Updates => self.render_updates_settings(&settings, cx),
            SettingsSection::About => self.render_about_settings(),
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
                            .bg(theme::sidebar())
                            .border_r_1()
                            .border_color(theme::border_strong())
                            .child(general)
                            .child(terminal)
                            .child(agent)
                            .child(updates)
                            .child(div().flex_1())
                            .child(about),
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
        .border_1()
        .border_color(theme::border())
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

fn settings_choice_button(
    id: String,
    label: String,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
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
        .child(SharedString::from(label))
        .on_click(on_click)
        .into_any_element()
}

fn input_text_part(field: AgentInputField, value: &str) -> AnyElement {
    let display = if field == AgentInputField::ApiKey {
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
                cx.new(|cx| SettingsWindow::new(shell, cx))
            },
        )
        .ok();
    });
}

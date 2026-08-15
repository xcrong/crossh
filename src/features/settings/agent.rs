use super::*;

impl SettingsWindow {
    pub(super) fn render_agent_settings(
        &mut self,
        settings: &SettingsSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let compact_layout = self.compact_layout;
        let settings_row = move |label: String, description: String, control: AnyElement| {
            responsive_settings_row(label, description, control, compact_layout)
        };
        self.prepare_agent_draft(settings);

        let mut active_models = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_row()
            .gap_1()
            .flex_wrap()
            .justify_end();
        let mut reviewer_models = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_row()
            .gap_1()
            .flex_wrap()
            .justify_end();
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
                    .text_color(theme::text())
                    .font_weight(FontWeight::MEDIUM)
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

        div()
            .id("settings-agent")
            .max_w(px(760.))
            .flex()
            .flex_col()
            .child(settings_heading("settings.agent"))
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
                i18n::text("settings.agent_status"),
                status_description,
                save,
            ))
            .into_any_element()
    }

    pub(super) fn agent_input(
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
        let mask_api_key = field == AgentInputField::ApiKey && !self.agent_api_key_revealed;
        let focus = focus.clone();
        let focused = focus.is_focused(window);
        let active = focused && self.agent_edit_field == Some(field);
        let cursor = if active {
            clamp_char_boundary(&value, self.agent_cursor)
        } else {
            value.len()
        };
        let anchor = if active {
            self.agent_anchor
                .map(|anchor| clamp_char_boundary(&value, anchor))
        } else {
            None
        };
        let selection = if active {
            selection_bounds(anchor, cursor)
        } else {
            None
        };
        let (start, end) = selection.unwrap_or((cursor, cursor));
        let ime_marked_text = if active {
            self.agent_ime_marked_text.clone()
        } else {
            String::new()
        };
        let ime_end = self
            .agent_ime_replacement
            .filter(|_| !ime_marked_text.is_empty())
            .map(|(_, end)| end)
            .unwrap_or(end)
            .min(value.len());
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
                        move |event: &MouseMoveEvent, phase, window, cx| {
                            if !matches!(phase, gpui::DispatchPhase::Bubble) {
                                return;
                            }
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
                                field == AgentInputField::ApiKey
                                    && !entity.read(cx).agent_api_key_revealed,
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
                        move |_event: &MouseUpEvent, phase, _window, cx| {
                            if !matches!(phase, gpui::DispatchPhase::Bubble) {
                                return;
                            }
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
            .w_full()
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
                                    field == AgentInputField::ApiKey
                                        && !this.agent_api_key_revealed,
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
        input = input.child(input_text_part(field, &value[..start], mask_api_key));
        if selection.is_some() {
            input = input.child(
                div()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .bg(theme::selection())
                    .child(input_text_part(field, &value[start..end], mask_api_key)),
            );
        } else if focused {
            input = input.child(text_caret(px(16.)));
        }
        if !ime_marked_text.is_empty() {
            input = input.child(
                div()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .underline()
                    .text_decoration_color(theme::accent())
                    .child(input_text_part(field, &ime_marked_text, mask_api_key)),
            );
        }
        input
            .child(input_text_part(field, &value[ime_end..], mask_api_key))
            .child(ime_input_canvas(focus, cx.entity()))
            .child(tracking)
            .into_any_element()
    }

    pub(super) fn handle_agent_input_key(
        &mut self,
        field: AgentInputField,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.activate_agent_input(field);
        self.agent_ime_marked_text.clear();
        self.agent_ime_replacement = None;
        let provider_index = self.agent_provider_index;
        let model_index = self.agent_model_index;
        let Some(provider) = self.agent_draft.providers.get(provider_index) else {
            return;
        };
        let Some(model) = provider.models.get(model_index) else {
            return;
        };
        let old_model_id = model.id.clone();
        let old_provider_id = provider.id.clone();
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

    pub(super) fn activate_agent_input(&mut self, field: AgentInputField) {
        let value = self.agent_input_value(field);
        if self.agent_edit_field != Some(field) {
            self.agent_edit_field = Some(field);
            self.agent_cursor = value.len();
            self.agent_anchor = None;
        } else {
            self.agent_cursor = clamp_char_boundary(&value, self.agent_cursor);
            self.agent_anchor = self
                .agent_anchor
                .map(|anchor| clamp_char_boundary(&value, anchor));
        }
    }

    pub(super) fn reset_agent_input_state(&mut self) {
        self.agent_edit_field = None;
        self.agent_cursor = 0;
        self.agent_anchor = None;
        self.agent_ime_marked_text.clear();
        self.agent_ime_replacement = None;
        self.agent_dragging = false;
    }

    pub(super) fn toggle_api_key_visibility(&mut self, cx: &mut Context<Self>) {
        self.agent_api_key_revealed = !self.agent_api_key_revealed;
        cx.notify();
    }

    pub(super) fn agent_input_value(&self, field: AgentInputField) -> String {
        let Some(provider) = self.agent_draft.providers.get(self.agent_provider_index) else {
            return String::new();
        };
        let model = provider.models.get(self.agent_model_index);
        match field {
            AgentInputField::ProviderId => provider.id.clone(),
            AgentInputField::ProviderName => provider.name.clone(),
            AgentInputField::Url => provider.url.clone(),
            AgentInputField::Model => model.map(|model| model.id.clone()).unwrap_or_default(),
            AgentInputField::ModelName => model.map(|model| model.name.clone()).unwrap_or_default(),
            AgentInputField::ApiKey => provider.api_key.clone(),
            AgentInputField::KeyEnv => provider.api_key_env.clone(),
            AgentInputField::ContextWindow => model
                .map(|model| model.context_window.to_string())
                .unwrap_or_default(),
            AgentInputField::MaxTokens => model
                .map(|model| model.max_tokens.to_string())
                .unwrap_or_default(),
        }
    }

    pub(super) fn set_agent_input_value(&mut self, field: AgentInputField, value: String) {
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

    pub(super) fn replace_agent_selection(&mut self, field: AgentInputField, replacement: &str) {
        let mut value = self.agent_input_value(field);
        let cursor = clamp_char_boundary(&value, self.agent_cursor);
        let anchor = self
            .agent_anchor
            .map(|anchor| clamp_char_boundary(&value, anchor));
        let (start, end) = selection_bounds(anchor, cursor).unwrap_or((cursor, cursor));
        value.replace_range(start..end, replacement);
        self.agent_cursor = start + replacement.len();
        self.agent_anchor = None;
        self.set_agent_input_value(field, value);
    }

    pub(super) fn agent_backspace(&mut self, field: AgentInputField) {
        if selection_bounds(self.agent_anchor, self.agent_cursor).is_some() {
            self.replace_agent_selection(field, "");
            return;
        }
        let value = self.agent_input_value(field);
        self.agent_cursor = clamp_char_boundary(&value, self.agent_cursor);
        let start = previous_char_boundary(&value, self.agent_cursor);
        if start != self.agent_cursor {
            self.agent_anchor = Some(start);
            self.replace_agent_selection(field, "");
        }
    }

    pub(super) fn agent_delete(&mut self, field: AgentInputField) {
        if selection_bounds(self.agent_anchor, self.agent_cursor).is_some() {
            self.replace_agent_selection(field, "");
            return;
        }
        let value = self.agent_input_value(field);
        self.agent_cursor = clamp_char_boundary(&value, self.agent_cursor);
        let end = next_char_boundary(&value, self.agent_cursor);
        if end != self.agent_cursor {
            self.agent_anchor = Some(end);
            self.replace_agent_selection(field, "");
        }
    }

    pub(super) fn move_agent_cursor(&mut self, field: AgentInputField, right: bool, extend: bool) {
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
        self.agent_cursor = clamp_char_boundary(&value, self.agent_cursor);
        self.agent_anchor = self
            .agent_anchor
            .map(|anchor| clamp_char_boundary(&value, anchor));
        self.agent_cursor = if right {
            next_char_boundary(&value, self.agent_cursor)
        } else {
            previous_char_boundary(&value, self.agent_cursor)
        };
        if !extend {
            self.agent_anchor = None;
        }
    }

    pub(super) fn add_agent_provider(&mut self, cx: &mut Context<Self>) {
        let number = self.agent_draft.providers.len() + 1;
        let id = format!("provider-{number}");
        self.agent_draft.providers.push(AgentProvider {
            id: id.clone(),
            name: format!("Provider {number}"),
            protocol: AgentProtocol::OpenAiChat,
            url: String::new(),
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
        let first_model = AgentModelRef {
            provider: id,
            model: "model".into(),
        };
        if self.agent_draft.active_model == AgentModelRef::default() {
            self.agent_draft.active_model = first_model.clone();
        }
        if self.agent_draft.reviewer_model == AgentModelRef::default() {
            self.agent_draft.reviewer_model = first_model;
        }
        self.agent_provider_index = self.agent_draft.providers.len() - 1;
        self.agent_model_index = 0;
        self.agent_error = None;
        self.agent_model_editor_open = false;
        self.reset_agent_input_state();
        cx.notify();
    }

    pub(super) fn remove_agent_provider(&mut self, cx: &mut Context<Self>) {
        if self.agent_draft.providers.is_empty() {
            return;
        }
        let removed = self.agent_draft.providers.remove(self.agent_provider_index);
        self.agent_provider_index = self
            .agent_provider_index
            .min(self.agent_draft.providers.len().saturating_sub(1));
        self.agent_model_index = 0;
        if self.agent_draft.providers.is_empty() {
            self.agent_draft.active_model = AgentModelRef::default();
            self.agent_draft.reviewer_model = AgentModelRef::default();
            self.agent_error = None;
            self.agent_model_editor_open = false;
            self.reset_agent_input_state();
            cx.notify();
            return;
        }
        let fallback = &self.agent_draft.providers[self.agent_provider_index];
        let fallback_ref = fallback
            .models
            .first()
            .map(|model| AgentModelRef {
                provider: fallback.id.clone(),
                model: model.id.clone(),
            })
            .unwrap_or_default();
        if self.agent_draft.active_model.provider == removed.id {
            self.agent_draft.active_model = fallback_ref.clone();
        }
        if self.agent_draft.reviewer_model.provider == removed.id {
            self.agent_draft.reviewer_model = fallback_ref;
        }
        self.agent_error = None;
        self.agent_model_editor_open = false;
        self.reset_agent_input_state();
        cx.notify();
    }

    pub(super) fn add_agent_model(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self
            .agent_draft
            .providers
            .get_mut(self.agent_provider_index)
        else {
            self.agent_error = Some(i18n::text("settings.agent_provider_required"));
            cx.notify();
            return;
        };
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
        self.agent_model_editor_open = true;
        self.reset_agent_input_state();
        cx.notify();
    }

    pub(super) fn remove_agent_model(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self
            .agent_draft
            .providers
            .get_mut(self.agent_provider_index)
        else {
            self.agent_error = Some(i18n::text("settings.agent_provider_required"));
            cx.notify();
            return;
        };
        if self.agent_model_index >= provider.models.len() {
            return;
        }
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
        self.agent_model_editor_open = false;
        self.reset_agent_input_state();
        cx.notify();
    }

    pub(super) fn save_agent_settings(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn install_update(&mut self, cx: &mut Context<Self>) {
        match self.updates.update(cx, |updates, _cx| updates.install()) {
            Ok(()) => self.write_to_shell(cx, |shell, cx| shell.quit_for_update(cx)),
            Err(error) => {
                self.updates
                    .update(cx, |updates, _cx| updates.set_failed(error));
                cx.notify();
            }
        }
    }

    pub(super) fn render_about_settings(&self) -> AnyElement {
        let compact_layout = self.compact_layout;
        let settings_row = move |label: String, description: String, control: AnyElement| {
            responsive_settings_row(label, description, control, compact_layout)
        };
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

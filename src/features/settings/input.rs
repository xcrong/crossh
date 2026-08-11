use super::*;

use gpui::{Bounds, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window, px};
use std::ops::Range;

use crossh_ui::widgets::{
    byte_index_for_utf16, ime_caret_bounds, replace_utf16_range, utf16_len, utf16_offset_for_byte,
    utf16_slice,
};

impl SettingsWindow {
    fn agent_focus_handle(&self, field: AgentInputField) -> &FocusHandle {
        match field {
            AgentInputField::ProviderId => &self.agent_provider_id_focus,
            AgentInputField::ProviderName => &self.agent_provider_name_focus,
            AgentInputField::Url => &self.agent_url_focus,
            AgentInputField::Model => &self.agent_model_focus,
            AgentInputField::ModelName => &self.agent_model_name_focus,
            AgentInputField::ApiKey => &self.agent_api_key_focus,
            AgentInputField::KeyEnv => &self.agent_key_env_focus,
            AgentInputField::ContextWindow => &self.agent_context_focus,
            AgentInputField::MaxTokens => &self.agent_max_tokens_focus,
        }
    }

    fn active_agent_input_field(&self, window: &Window) -> Option<AgentInputField> {
        self.agent_edit_field
            .filter(|field| self.agent_focus_handle(*field).is_focused(window))
    }
}

impl EntityInputHandler for SettingsWindow {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let field = self.active_agent_input_field(window)?;
        let value = self.agent_input_value(field);
        Some(utf16_slice(&value, range))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let field = self.active_agent_input_field(window)?;
        let value = self.agent_input_value(field);
        let cursor = clamp_char_boundary(&value, self.agent_cursor);
        let anchor = self
            .agent_anchor
            .map(|anchor| clamp_char_boundary(&value, anchor));
        let (start, end) = selection_bounds(anchor, cursor).unwrap_or((cursor, cursor));
        Some(UTF16Selection {
            range: utf16_offset_for_byte(&value, start)..utf16_offset_for_byte(&value, end),
            reversed: anchor.is_some_and(|anchor| anchor > cursor),
        })
    }

    fn marked_text_range(
        &self,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let field = self.active_agent_input_field(window)?;
        if self.agent_ime_marked_text.is_empty() {
            return None;
        }
        let value = self.agent_input_value(field);
        let start = self
            .agent_ime_replacement
            .map(|(start, _)| start)
            .unwrap_or_else(|| clamp_char_boundary(&value, self.agent_cursor));
        let start = utf16_offset_for_byte(&value, start);
        Some(start..start + utf16_len(&self.agent_ime_marked_text))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_agent_input_field(window).is_none() {
            return;
        }
        if let Some((start, end)) = self.agent_ime_replacement.take() {
            self.agent_cursor = end;
            self.agent_anchor = (start != end).then_some(start);
        }
        self.agent_ime_marked_text.clear();
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        replacement_range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = self.active_agent_input_field(window) else {
            return;
        };
        let value = self.agent_input_value(field);
        let replacement = if matches!(
            field,
            AgentInputField::ContextWindow | AgentInputField::MaxTokens
        ) {
            text.chars()
                .filter(char::is_ascii_digit)
                .collect::<String>()
        } else {
            text.to_owned()
        };
        let (start, end) = self
            .agent_ime_replacement
            .take()
            .or_else(|| {
                replacement_range.map(|range| {
                    (
                        byte_index_for_utf16(&value, range.start),
                        byte_index_for_utf16(&value, range.end),
                    )
                })
            })
            .or_else(|| {
                let cursor = clamp_char_boundary(&value, self.agent_cursor);
                selection_bounds(self.agent_anchor, cursor)
            })
            .unwrap_or_else(|| {
                let cursor = clamp_char_boundary(&value, self.agent_cursor);
                (cursor, cursor)
            });
        let mut value = value;
        let replacement_range =
            utf16_offset_for_byte(&value, start)..utf16_offset_for_byte(&value, end);
        let cursor = replace_utf16_range(&mut value, replacement_range, &replacement);
        self.set_agent_input_value(field, value);
        self.agent_cursor = cursor;
        self.agent_anchor = None;
        self.agent_ime_marked_text.clear();
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = self.active_agent_input_field(window) else {
            return;
        };
        let value = self.agent_input_value(field);
        let replacement = self
            .agent_ime_replacement
            .or_else(|| {
                range.map(|range| {
                    (
                        byte_index_for_utf16(&value, range.start),
                        byte_index_for_utf16(&value, range.end),
                    )
                })
            })
            .or_else(|| {
                let cursor = clamp_char_boundary(&value, self.agent_cursor);
                selection_bounds(self.agent_anchor, cursor)
            })
            .unwrap_or_else(|| {
                let cursor = clamp_char_boundary(&value, self.agent_cursor);
                (cursor, cursor)
            });
        self.agent_ime_replacement = Some(replacement);
        self.agent_cursor = replacement.0;
        self.agent_anchor = None;
        self.agent_ime_marked_text = if matches!(
            field,
            AgentInputField::ContextWindow | AgentInputField::MaxTokens
        ) {
            new_text.chars().filter(char::is_ascii_digit).collect()
        } else {
            new_text.to_owned()
        };
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let field = self.active_agent_input_field(window)?;
        let value = self.agent_input_value(field);
        let cursor = byte_index_for_utf16(&value, range.start);
        let text_before = if field == AgentInputField::ApiKey && !self.agent_api_key_revealed {
            "•".repeat(value[..cursor].chars().count())
        } else {
            value[..cursor].to_owned()
        };
        Some(ime_caret_bounds(
            window,
            element_bounds,
            &text_before,
            px(12.),
            px(8.),
            px(0.),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    fn text_length_utf16(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> Option<usize> {
        let field = self.active_agent_input_field(window)?;
        Some(utf16_len(&self.agent_input_value(field)))
    }
}

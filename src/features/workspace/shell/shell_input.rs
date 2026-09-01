//! AppShell text input and IME handling.

use std::ops::Range;

use gpui::{Bounds, EntityInputHandler, Pixels, UTF16Selection};

use crate::shared::text_editing::{byte_index_for_utf16, utf16_len, utf16_slice};
use crossh_ui::widgets::ime_caret_bounds;

use crate::shared::input_handler::{
    editing_mark_text, editing_marked_range, editing_replace, editing_selected_range,
    editing_unmark, plain_mark, plain_marked_range, plain_replace, plain_selected_range,
};
use crate::shared::text_editing::TextEditingState;

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum AppShellInputField {
    HostSearch,
    Rename,
    DefaultCommand,
    CommandPalette,
    Compose,
}

impl AppShell {
    fn active_input_field(&self, window: &Window) -> Option<AppShellInputField> {
        if self
            .command_palette
            .as_ref()
            .is_some_and(|palette| palette.focus.is_focused(window))
        {
            Some(AppShellInputField::CommandPalette)
        } else if self
            .default_command_editor
            .as_ref()
            .is_some_and(|editor| editor.focus.is_focused(window))
        {
            Some(AppShellInputField::DefaultCommand)
        } else if self
            .rename_editor
            .as_ref()
            .is_some_and(|editor| editor.focus.is_focused(window))
        {
            Some(AppShellInputField::Rename)
        } else if self
            .workspace
            .focused_view()
            .is_some_and(|view| self.workspace.compose_visible(view))
            && self.compose_focus.is_focused(window)
        {
            Some(AppShellInputField::Compose)
        } else if self.search_focus.is_focused(window) {
            Some(AppShellInputField::HostSearch)
        } else {
            None
        }
    }

    fn plain_value(&self, field: AppShellInputField) -> Option<&String> {
        match field {
            AppShellInputField::HostSearch => Some(&self.search_query),
            _ => None,
        }
    }

    fn plain_marked(&self, field: AppShellInputField) -> Option<&String> {
        match field {
            AppShellInputField::HostSearch => Some(&self.search_ime_marked_text),
            _ => None,
        }
    }

    fn plain_value_and_marked_mut(
        &mut self,
        field: AppShellInputField,
    ) -> Option<(&mut String, &mut String)> {
        match field {
            AppShellInputField::HostSearch => {
                Some((&mut self.search_query, &mut self.search_ime_marked_text))
            }
            _ => None,
        }
    }

    fn editing_state(&self, field: AppShellInputField) -> Option<&TextEditingState> {
        match field {
            AppShellInputField::HostSearch => None,
            AppShellInputField::CommandPalette => {
                self.command_palette.as_ref().map(|palette| &palette.query)
            }
            AppShellInputField::Rename => self.rename_editor.as_ref().map(|editor| &editor.state),
            AppShellInputField::DefaultCommand => self
                .default_command_editor
                .as_ref()
                .map(|editor| &editor.state),
            AppShellInputField::Compose => {
                let view = self.workspace.focused_view()?;
                self.workspace.compose.get(&view).map(|entry| &entry.state)
            }
        }
    }

    fn editing_state_mut(&mut self, field: AppShellInputField) -> Option<&mut TextEditingState> {
        match field {
            AppShellInputField::HostSearch => None,
            AppShellInputField::CommandPalette => self
                .command_palette
                .as_mut()
                .map(|palette| &mut palette.query),
            AppShellInputField::Rename => {
                self.rename_editor.as_mut().map(|editor| &mut editor.state)
            }
            AppShellInputField::DefaultCommand => self
                .default_command_editor
                .as_mut()
                .map(|editor| &mut editor.state),
            AppShellInputField::Compose => {
                let view = self.workspace.focused_view()?;
                self.workspace
                    .compose
                    .get_mut(&view)
                    .map(|entry| &mut entry.state)
            }
        }
    }

    fn editing_state_mut_for_replace(
        &mut self,
        field: AppShellInputField,
    ) -> Option<&mut TextEditingState> {
        match field {
            AppShellInputField::HostSearch => None,
            AppShellInputField::Compose => {
                let view = self.workspace.focused_view()?;
                Some(&mut self.workspace.compose_entry_mut(view).state)
            }
            _ => self.editing_state_mut(field),
        }
    }

    fn value_for_field(&self, field: AppShellInputField) -> Option<&String> {
        if let Some(value) = self.plain_value(field) {
            return Some(value);
        }
        self.editing_state(field).map(|state| &state.value)
    }

    fn editing_scroll_x(&self, field: AppShellInputField) -> Pixels {
        match field {
            AppShellInputField::CommandPalette => self
                .command_palette
                .as_ref()
                .map(|palette| palette.scroll.offset().x)
                .unwrap_or(px(0.)),
            AppShellInputField::Compose => self.compose_scroll.offset().x,
            _ => px(0.),
        }
    }
}

impl EntityInputHandler for AppShell {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let field = self.active_input_field(window)?;
        let text = self.value_for_field(field)?;
        Some(utf16_slice(text, range))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let field = self.active_input_field(window)?;
        let selection = if let Some(state) = self.editing_state(field) {
            editing_selected_range(state)
        } else {
            plain_selected_range(self.plain_value(field)?)
        };
        Some(UTF16Selection {
            range: selection.range,
            reversed: selection.reversed,
        })
    }

    fn marked_text_range(
        &self,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let field = self.active_input_field(window)?;
        if let Some(state) = self.editing_state(field) {
            editing_marked_range(state)
        } else {
            let value = self.plain_value(field)?;
            let marked = self.plain_marked(field)?;
            plain_marked_range(value, marked)
        }
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.active_input_field(window) {
            if let Some((_, marked)) = self.plain_value_and_marked_mut(field) {
                marked.clear();
            } else if let Some(state) = self.editing_state_mut(field) {
                editing_unmark(state);
            }
        }
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
        if let Some(field) = self.active_input_field(window) {
            if let Some((value, marked)) = self.plain_value_and_marked_mut(field) {
                plain_replace(value, marked, replacement_range, text);
            } else if let Some(state) = self.editing_state_mut_for_replace(field) {
                editing_replace(state, replacement_range, text);
                if field == AppShellInputField::CommandPalette
                    && let Some(palette) = self.command_palette.as_mut()
                {
                    palette.clamp_selection();
                }
            } else {
                return;
            }
        } else {
            return;
        }
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.active_input_field(window) {
            if let Some((_, marked)) = self.plain_value_and_marked_mut(field) {
                plain_mark(marked, new_text);
            } else if let Some(state) = self.editing_state_mut_for_replace(field) {
                editing_mark_text(state, new_text);
            } else {
                return;
            }
        } else {
            return;
        }
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
        let field = self.active_input_field(window)?;
        match field {
            AppShellInputField::HostSearch => {
                let cursor = byte_index_for_utf16(&self.search_query, range.start);
                Some(ime_caret_bounds(
                    window,
                    element_bounds,
                    &self.search_query[..cursor],
                    px(12.),
                    px(30.),
                    px(0.),
                ))
            }
            _ => {
                let state = self.editing_state(field)?;
                let scroll_x = self.editing_scroll_x(field);
                let cursor = byte_index_for_utf16(&state.value, range.start);
                Some(ime_caret_bounds(
                    window,
                    element_bounds,
                    &state.value[..cursor],
                    px(14.),
                    px(12.),
                    scroll_x,
                ))
            }
        }
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
        let field = self.active_input_field(window)?;
        let text = self.value_for_field(field)?;
        Some(utf16_len(text))
    }
}

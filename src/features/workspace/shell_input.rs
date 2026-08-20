//! AppShell text input and IME handling.

use std::ops::Range;

use gpui::{Bounds, EntityInputHandler, Pixels, UTF16Selection};

use crossh_ui::widgets::{
    byte_index_for_utf16, ime_caret_bounds, replace_utf16_range, utf16_len, utf16_offset_for_byte,
    utf16_slice,
};

use super::*;

#[derive(Clone, Copy)]
enum AppShellInputField {
    HostSearch,
    Credential,
    QuickCommand,
    Rename,
    DefaultCommand,
    Compose,
}

impl AppShell {
    fn active_input_field(&self, window: &Window) -> Option<AppShellInputField> {
        if self.modal_focus.is_focused(window) {
            Some(AppShellInputField::Credential)
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
            .quick_command_editor
            .as_ref()
            .is_some_and(|editor| editor.focus.is_focused(window))
        {
            Some(AppShellInputField::QuickCommand)
        } else if self
            .workspace
            .focused_view()
            .is_some_and(|view| self.workspace.compose_visible(view))
            && self.compose_focus.is_focused(window)
        {
            Some(AppShellInputField::Compose)
        } else if self.host_focus.is_focused(window) {
            Some(AppShellInputField::HostSearch)
        } else {
            None
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
        // 为 Compose 单独处理以避免借用冲突：需通过 workspace 获取终端级状态
        if let AppShellInputField::Compose = field {
            let view = self.workspace.focused_view()?;
            let text = &self.workspace.compose.get(&view)?.state.value;
            return Some(utf16_slice(text, range));
        }
        let text = match field {
            AppShellInputField::HostSearch => &self.host_query,
            AppShellInputField::Credential => &self.prompt_input,
            AppShellInputField::QuickCommand => &self.quick_command_editor.as_ref()?.state.value,
            AppShellInputField::Rename => &self.rename_editor.as_ref()?.state.value,
            AppShellInputField::DefaultCommand => {
                &self.default_command_editor.as_ref()?.state.value
            }
            AppShellInputField::Compose => unreachable!(),
        };
        Some(utf16_slice(text, range))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        match self.active_input_field(window)? {
            AppShellInputField::HostSearch => {
                let position = utf16_len(&self.host_query);
                Some(UTF16Selection {
                    range: position..position,
                    reversed: false,
                })
            }
            AppShellInputField::Credential => {
                let position = utf16_len(&self.prompt_input);
                Some(UTF16Selection {
                    range: position..position,
                    reversed: false,
                })
            }
            AppShellInputField::QuickCommand => {
                let editor = self.quick_command_editor.as_ref()?;
                let (start, end) = editor
                    .state
                    .selection()
                    .unwrap_or((editor.state.cursor, editor.state.cursor));
                Some(UTF16Selection {
                    range: utf16_offset_for_byte(&editor.state.value, start)
                        ..utf16_offset_for_byte(&editor.state.value, end),
                    reversed: editor
                        .state
                        .anchor
                        .is_some_and(|anchor| anchor > editor.state.cursor),
                })
            }
            AppShellInputField::Rename => {
                let editor = self.rename_editor.as_ref()?;
                let (start, end) = editor
                    .state
                    .selection()
                    .unwrap_or((editor.state.cursor, editor.state.cursor));
                Some(UTF16Selection {
                    range: utf16_offset_for_byte(&editor.state.value, start)
                        ..utf16_offset_for_byte(&editor.state.value, end),
                    reversed: editor
                        .state
                        .anchor
                        .is_some_and(|anchor| anchor > editor.state.cursor),
                })
            }
            AppShellInputField::DefaultCommand => {
                let editor = self.default_command_editor.as_ref()?;
                let (start, end) = editor
                    .state
                    .selection()
                    .unwrap_or((editor.state.cursor, editor.state.cursor));
                Some(UTF16Selection {
                    range: utf16_offset_for_byte(&editor.state.value, start)
                        ..utf16_offset_for_byte(&editor.state.value, end),
                    reversed: editor
                        .state
                        .anchor
                        .is_some_and(|anchor| anchor > editor.state.cursor),
                })
            }
            AppShellInputField::Compose => {
                let view = self.workspace.focused_view()?;
                let state = &self.workspace.compose.get(&view)?.state;
                let (start, end) = state.selection().unwrap_or((state.cursor, state.cursor));
                Some(UTF16Selection {
                    range: utf16_offset_for_byte(&state.value, start)
                        ..utf16_offset_for_byte(&state.value, end),
                    reversed: state.anchor.is_some_and(|anchor| anchor > state.cursor),
                })
            }
        }
    }

    fn marked_text_range(
        &self,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        match self.active_input_field(window)? {
            AppShellInputField::HostSearch => (!self.host_ime_marked_text.is_empty()).then(|| {
                let start = utf16_len(&self.host_query);
                start..start + utf16_len(&self.host_ime_marked_text)
            }),
            AppShellInputField::Credential => {
                (!self.prompt_ime_marked_text.is_empty()).then(|| {
                    let start = utf16_len(&self.prompt_input);
                    start..start + utf16_len(&self.prompt_ime_marked_text)
                })
            }
            AppShellInputField::QuickCommand => {
                let editor = self.quick_command_editor.as_ref()?;
                let (start, _) = editor.state.ime_replacement?;
                (!editor.state.ime_marked_text.is_empty()).then(|| {
                    let start = utf16_offset_for_byte(&editor.state.value, start);
                    start..start + utf16_len(&editor.state.ime_marked_text)
                })
            }
            AppShellInputField::Rename => {
                let editor = self.rename_editor.as_ref()?;
                let (start, _) = editor.state.ime_replacement?;
                (!editor.state.ime_marked_text.is_empty()).then(|| {
                    let start = utf16_offset_for_byte(&editor.state.value, start);
                    start..start + utf16_len(&editor.state.ime_marked_text)
                })
            }
            AppShellInputField::DefaultCommand => {
                let editor = self.default_command_editor.as_ref()?;
                let (start, _) = editor.state.ime_replacement?;
                (!editor.state.ime_marked_text.is_empty()).then(|| {
                    let start = utf16_offset_for_byte(&editor.state.value, start);
                    start..start + utf16_len(&editor.state.ime_marked_text)
                })
            }
            AppShellInputField::Compose => {
                let view = self.workspace.focused_view()?;
                let state = &self.workspace.compose.get(&view)?.state;
                let (start, _) = state.ime_replacement?;
                (!state.ime_marked_text.is_empty()).then(|| {
                    let start = utf16_offset_for_byte(&state.value, start);
                    start..start + utf16_len(&state.ime_marked_text)
                })
            }
        }
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_input_field(window) {
            Some(AppShellInputField::HostSearch) => self.host_ime_marked_text.clear(),
            Some(AppShellInputField::Credential) => self.prompt_ime_marked_text.clear(),
            Some(AppShellInputField::QuickCommand) => {
                if let Some(editor) = &mut self.quick_command_editor {
                    if let Some((start, end)) = editor.state.ime_replacement.take() {
                        editor.state.cursor = end;
                        editor.state.anchor = (start != end).then_some(start);
                    }
                    editor.state.ime_marked_text.clear();
                }
            }
            Some(AppShellInputField::Rename) => {
                if let Some(editor) = &mut self.rename_editor {
                    if let Some((start, end)) = editor.state.ime_replacement.take() {
                        editor.state.cursor = end;
                        editor.state.anchor = (start != end).then_some(start);
                    }
                    editor.state.ime_marked_text.clear();
                }
            }
            Some(AppShellInputField::DefaultCommand) => {
                if let Some(editor) = &mut self.default_command_editor {
                    if let Some((start, end)) = editor.state.ime_replacement.take() {
                        editor.state.cursor = end;
                        editor.state.anchor = (start != end).then_some(start);
                    }
                    editor.state.ime_marked_text.clear();
                }
            }
            Some(AppShellInputField::Compose) => {
                if let Some(view) = self.workspace.focused_view()
                    && let Some(entry) = self.workspace.compose.get_mut(&view)
                {
                    if let Some((start, end)) = entry.state.ime_replacement.take() {
                        entry.state.cursor = end;
                        entry.state.anchor = (start != end).then_some(start);
                    }
                    entry.state.ime_marked_text.clear();
                }
            }
            None => {}
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
        match self.active_input_field(window) {
            Some(AppShellInputField::HostSearch) => {
                let position = utf16_len(&self.host_query);
                replace_utf16_range(
                    &mut self.host_query,
                    replacement_range.unwrap_or(position..position),
                    text,
                );
                self.host_ime_marked_text.clear();
            }
            Some(AppShellInputField::Credential) => {
                let position = utf16_len(&self.prompt_input);
                replace_utf16_range(
                    &mut self.prompt_input,
                    replacement_range.unwrap_or(position..position),
                    text,
                );
                self.prompt_ime_marked_text.clear();
            }
            Some(AppShellInputField::QuickCommand) => {
                if let Some(editor) = &mut self.quick_command_editor {
                    let (start, end) = if let Some(range) = editor.state.ime_replacement.take() {
                        range
                    } else if let Some(range) = replacement_range {
                        (
                            byte_index_for_utf16(&editor.state.value, range.start),
                            byte_index_for_utf16(&editor.state.value, range.end),
                        )
                    } else {
                        editor
                            .state
                            .selection()
                            .unwrap_or((editor.state.cursor, editor.state.cursor))
                    };
                    editor.state.value.replace_range(start..end, text);
                    editor.state.cursor = start + text.len();
                    editor.state.anchor = None;
                    editor.state.ime_marked_text.clear();
                }
            }
            Some(AppShellInputField::Rename) => {
                if let Some(editor) = &mut self.rename_editor {
                    let (start, end) = if let Some(range) = editor.state.ime_replacement.take() {
                        range
                    } else if let Some(range) = replacement_range {
                        (
                            byte_index_for_utf16(&editor.state.value, range.start),
                            byte_index_for_utf16(&editor.state.value, range.end),
                        )
                    } else {
                        editor
                            .state
                            .selection()
                            .unwrap_or((editor.state.cursor, editor.state.cursor))
                    };
                    editor.state.value.replace_range(start..end, text);
                    editor.state.cursor = start + text.len();
                    editor.state.anchor = None;
                    editor.state.ime_marked_text.clear();
                }
            }
            Some(AppShellInputField::DefaultCommand) => {
                if let Some(editor) = &mut self.default_command_editor {
                    let (start, end) = if let Some(range) = editor.state.ime_replacement.take() {
                        range
                    } else if let Some(range) = replacement_range {
                        (
                            byte_index_for_utf16(&editor.state.value, range.start),
                            byte_index_for_utf16(&editor.state.value, range.end),
                        )
                    } else {
                        editor
                            .state
                            .selection()
                            .unwrap_or((editor.state.cursor, editor.state.cursor))
                    };
                    editor.state.value.replace_range(start..end, text);
                    editor.state.cursor = start + text.len();
                    editor.state.anchor = None;
                    editor.state.ime_marked_text.clear();
                }
            }
            Some(AppShellInputField::Compose) => {
                let Some(view) = self.workspace.focused_view() else {
                    return;
                };
                let entry = self.workspace.compose_entry_mut(view);
                let (start, end) = if let Some(range) = entry.state.ime_replacement.take() {
                    range
                } else if let Some(range) = replacement_range {
                    (
                        byte_index_for_utf16(&entry.state.value, range.start),
                        byte_index_for_utf16(&entry.state.value, range.end),
                    )
                } else {
                    entry
                        .state
                        .selection()
                        .unwrap_or((entry.state.cursor, entry.state.cursor))
                };
                entry.state.value.replace_range(start..end, text);
                entry.state.cursor = start + text.len();
                entry.state.anchor = None;
                entry.state.ime_marked_text.clear();
            }
            None => return,
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
        match self.active_input_field(window) {
            Some(AppShellInputField::HostSearch) => {
                self.host_ime_marked_text.clear();
                self.host_ime_marked_text.push_str(new_text);
            }
            Some(AppShellInputField::Credential) => {
                self.prompt_ime_marked_text.clear();
                self.prompt_ime_marked_text.push_str(new_text);
            }
            Some(AppShellInputField::QuickCommand) => {
                if let Some(editor) = &mut self.quick_command_editor {
                    if editor.state.ime_replacement.is_none() {
                        let replacement = editor
                            .state
                            .selection()
                            .unwrap_or((editor.state.cursor, editor.state.cursor));
                        editor.state.ime_replacement = Some(replacement);
                        editor.state.cursor = replacement.0;
                        editor.state.anchor = None;
                    }
                    editor.state.ime_marked_text.clear();
                    editor.state.ime_marked_text.push_str(new_text);
                }
            }
            Some(AppShellInputField::Rename) => {
                if let Some(editor) = &mut self.rename_editor {
                    if editor.state.ime_replacement.is_none() {
                        let replacement = editor
                            .state
                            .selection()
                            .unwrap_or((editor.state.cursor, editor.state.cursor));
                        editor.state.ime_replacement = Some(replacement);
                        editor.state.cursor = replacement.0;
                        editor.state.anchor = None;
                    }
                    editor.state.ime_marked_text.clear();
                    editor.state.ime_marked_text.push_str(new_text);
                }
            }
            Some(AppShellInputField::DefaultCommand) => {
                if let Some(editor) = &mut self.default_command_editor {
                    if editor.state.ime_replacement.is_none() {
                        let replacement = editor
                            .state
                            .selection()
                            .unwrap_or((editor.state.cursor, editor.state.cursor));
                        editor.state.ime_replacement = Some(replacement);
                        editor.state.cursor = replacement.0;
                        editor.state.anchor = None;
                    }
                    editor.state.ime_marked_text.clear();
                    editor.state.ime_marked_text.push_str(new_text);
                }
            }
            Some(AppShellInputField::Compose) => {
                let Some(view) = self.workspace.focused_view() else {
                    return;
                };
                let entry = self.workspace.compose_entry_mut(view);
                if entry.state.ime_replacement.is_none() {
                    let replacement = entry
                        .state
                        .selection()
                        .unwrap_or((entry.state.cursor, entry.state.cursor));
                    entry.state.ime_replacement = Some(replacement);
                    entry.state.cursor = replacement.0;
                    entry.state.anchor = None;
                }
                entry.state.ime_marked_text.clear();
                entry.state.ime_marked_text.push_str(new_text);
            }
            None => return,
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
        match self.active_input_field(window)? {
            AppShellInputField::HostSearch => {
                let cursor = byte_index_for_utf16(&self.host_query, range.start);
                Some(ime_caret_bounds(
                    window,
                    element_bounds,
                    &self.host_query[..cursor],
                    px(12.),
                    px(30.),
                    px(0.),
                ))
            }
            AppShellInputField::Credential => {
                let cursor = byte_index_for_utf16(&self.prompt_input, range.start);
                let bullet_count = self.prompt_input[..cursor].chars().count();
                let text_before = "•".repeat(bullet_count);
                Some(ime_caret_bounds(
                    window,
                    element_bounds,
                    &text_before,
                    px(14.),
                    px(12.),
                    px(0.),
                ))
            }
            AppShellInputField::QuickCommand => {
                let editor = self.quick_command_editor.as_ref()?;
                let cursor = byte_index_for_utf16(&editor.state.value, range.start);
                Some(ime_caret_bounds(
                    window,
                    element_bounds,
                    &editor.state.value[..cursor],
                    px(14.),
                    px(12.),
                    editor.scroll.offset().x,
                ))
            }
            AppShellInputField::Rename => {
                let editor = self.rename_editor.as_ref()?;
                let cursor = byte_index_for_utf16(&editor.state.value, range.start);
                Some(ime_caret_bounds(
                    window,
                    element_bounds,
                    &editor.state.value[..cursor],
                    px(14.),
                    px(12.),
                    px(0.),
                ))
            }
            AppShellInputField::DefaultCommand => {
                let editor = self.default_command_editor.as_ref()?;
                let cursor = byte_index_for_utf16(&editor.state.value, range.start);
                Some(ime_caret_bounds(
                    window,
                    element_bounds,
                    &editor.state.value[..cursor],
                    px(14.),
                    px(12.),
                    px(0.),
                ))
            }
            AppShellInputField::Compose => {
                let view = self.workspace.focused_view()?;
                let state = &self.workspace.compose.get(&view)?.state;
                Some(ime_caret_bounds(
                    window,
                    element_bounds,
                    &state.value[..byte_index_for_utf16(&state.value, range.start)],
                    px(14.),
                    px(12.),
                    self.compose_scroll.offset().x,
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
        match self.active_input_field(window)? {
            AppShellInputField::HostSearch => Some(utf16_len(&self.host_query)),
            AppShellInputField::Credential => Some(utf16_len(&self.prompt_input)),
            AppShellInputField::QuickCommand => self
                .quick_command_editor
                .as_ref()
                .map(|editor| utf16_len(&editor.state.value)),
            AppShellInputField::Rename => self
                .rename_editor
                .as_ref()
                .map(|editor| utf16_len(&editor.state.value)),
            AppShellInputField::DefaultCommand => self
                .default_command_editor
                .as_ref()
                .map(|editor| utf16_len(&editor.state.value)),
            AppShellInputField::Compose => {
                let view = self.workspace.focused_view()?;
                let state = &self.workspace.compose.get(&view)?.state;
                Some(utf16_len(&state.value))
            }
        }
    }
}

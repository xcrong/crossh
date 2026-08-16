//! Git 提交信息输入、剪贴板与 IME 处理。

use std::ops::Range;

use gpui::{
    Bounds, ClipboardEntry, ClipboardItem, Context, EntityInputHandler, KeyDownEvent, Pixels,
    Point, UTF16Selection, Window, px, size,
};

use crossh_ui::widgets::{
    byte_index_for_utf16, printable_char, replace_utf16_range, text_width, utf16_len,
    utf16_offset_for_byte, utf16_slice,
};

use super::window::GitWindow;
use crate::shared::text_editing::selection_bounds;

impl GitWindow {
    pub(super) fn handle_history_search_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        let primary = keystroke.modifiers.control || keystroke.modifiers.platform;
        let extend = keystroke.modifiers.shift;
        let mut handled = true;

        if keystroke.key == "escape" {
            self.history_query.clear();
            self.set_history_query(String::new(), cx);
            window.focus(&self.history_focus, cx);
        } else if primary && keystroke.key == "a" {
            self.history_query.select_all();
        } else if primary && matches!(keystroke.key.as_str(), "c" | "x") {
            if let Some(text) = self.history_query.selected_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                if keystroke.key == "x" {
                    self.history_query.clear_composition();
                    self.history_query.replace_selection("");
                }
            }
        } else if primary && keystroke.key == "v" {
            let pasted = cx.read_from_clipboard().and_then(|item| {
                item.into_entries().find_map(|entry| match entry {
                    ClipboardEntry::String(value) => Some(value.text),
                    _ => None,
                })
            });
            if let Some(text) = pasted {
                self.history_query.clear_composition();
                self.history_query.replace_selection(&text);
            }
        } else {
            match keystroke.key.as_str() {
                "backspace" => {
                    self.history_query.clear_composition();
                    self.history_query.backspace();
                }
                "delete" => {
                    self.history_query.clear_composition();
                    self.history_query.delete();
                }
                "left" => self.history_query.move_horizontal(-1, extend),
                "right" => self.history_query.move_horizontal(1, extend),
                "home" => self.history_query.move_to_boundary(false, extend),
                "end" => self.history_query.move_to_boundary(true, extend),
                _ => {
                    if let Some(character) = printable_char(keystroke) {
                        self.history_query.clear_composition();
                        self.history_query.replace_selection(&character.to_string());
                    } else {
                        handled = false;
                    }
                }
            }
        }

        if handled {
            let query = self.history_query.value.clone();
            self.set_history_query(query, cx);
            cx.stop_propagation();
            cx.notify();
        }
    }

    pub(super) fn handle_commit_editor_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        let primary = keystroke.modifiers.control || keystroke.modifiers.platform;
        let extend = keystroke.modifiers.shift;
        let mut handled = true;

        if primary && keystroke.key == "a" {
            self.commit_editor.clear_composition();
            self.commit_editor.select_all();
        } else if primary && matches!(keystroke.key.as_str(), "c" | "x") {
            if let Some(text) = self.commit_editor.selected_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                if keystroke.key == "x" {
                    self.commit_editor.clear_composition();
                    self.commit_editor.replace_selection("");
                }
            }
        } else if primary && keystroke.key == "v" {
            let pasted = cx.read_from_clipboard().and_then(|item| {
                item.into_entries().find_map(|entry| match entry {
                    ClipboardEntry::String(value) => Some(value.text),
                    _ => None,
                })
            });
            if let Some(text) = pasted {
                self.commit_editor.clear_composition();
                self.commit_editor.replace_selection(&text);
            }
        } else {
            match keystroke.key.as_str() {
                "enter" | "return" if primary => self.commit_changes(cx),
                "enter" | "return" => {
                    self.commit_editor.clear_composition();
                    self.commit_editor.replace_selection("\n");
                }
                "backspace" => {
                    self.commit_editor.clear_composition();
                    self.commit_editor.backspace();
                }
                "delete" => {
                    self.commit_editor.clear_composition();
                    self.commit_editor.delete();
                }
                "left" => self.commit_editor.move_horizontal(-1, extend),
                "right" => self.commit_editor.move_horizontal(1, extend),
                "home" => self.commit_editor.move_to_boundary(false, extend),
                "end" => self.commit_editor.move_to_boundary(true, extend),
                _ => {
                    if let Some(character) = printable_char(keystroke) {
                        self.commit_editor.clear_composition();
                        self.commit_editor.replace_selection(&character.to_string());
                    } else {
                        handled = false;
                    }
                }
            }
        }

        if handled {
            cx.stop_propagation();
            cx.notify();
        }
    }
}

impl EntityInputHandler for GitWindow {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let value = if self.history_search_focus.is_focused(_window) {
            &self.history_query.value
        } else {
            &self.commit_editor.state.value
        };
        Some(utf16_slice(value, range))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let (start, end) = if self.history_search_focus.is_focused(_window) {
            self.history_query
                .selection()
                .unwrap_or((self.history_query.cursor, self.history_query.cursor))
        } else {
            self.commit_editor.selection().unwrap_or((
                self.commit_editor.state.cursor,
                self.commit_editor.state.cursor,
            ))
        };
        let value = if self.history_search_focus.is_focused(_window) {
            &self.history_query.value
        } else {
            &self.commit_editor.state.value
        };
        Some(UTF16Selection {
            range: utf16_offset_for_byte(value, start)..utf16_offset_for_byte(value, end),
            reversed: if self.history_search_focus.is_focused(_window) {
                self.history_query
                    .anchor
                    .is_some_and(|anchor| anchor > self.history_query.cursor)
            } else {
                self.commit_editor
                    .state
                    .anchor
                    .is_some_and(|anchor| anchor > self.commit_editor.state.cursor)
            },
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let history_search = self.history_search_focus.is_focused(_window);
        let marked_text = if history_search {
            &self.history_query.ime_marked_text
        } else {
            &self.commit_editor.state.ime_marked_text
        };
        if marked_text.is_empty() {
            return None;
        }
        let (value, replacement, cursor) = if history_search {
            (
                &self.history_query.value,
                self.history_query.ime_replacement,
                self.history_query.cursor,
            )
        } else {
            (
                &self.commit_editor.state.value,
                self.commit_editor.state.ime_replacement,
                self.commit_editor.state.cursor,
            )
        };
        let start = replacement.map(|(start, _)| start).unwrap_or(cursor);
        let start = utf16_offset_for_byte(value, start);
        Some(start..start + utf16_len(marked_text))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.history_search_focus.is_focused(window) {
            if let Some((start, end)) = self.history_query.ime_replacement.take() {
                self.history_query.cursor = end;
                self.history_query.anchor = (start != end).then_some(start);
            }
            self.history_query.ime_marked_text.clear();
        } else {
            if let Some((start, end)) = self.commit_editor.state.ime_replacement.take() {
                self.commit_editor.state.cursor = end;
                self.commit_editor.state.anchor = (start != end).then_some(start);
            }
            self.commit_editor.state.ime_marked_text.clear();
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
        let history_search = self.history_search_focus.is_focused(window);
        let state = if history_search {
            &mut self.history_query
        } else {
            &mut self.commit_editor.state
        };
        let (start, end) = state
            .ime_replacement
            .take()
            .or_else(|| {
                replacement_range.map(|range| {
                    (
                        byte_index_for_utf16(&state.value, range.start),
                        byte_index_for_utf16(&state.value, range.end),
                    )
                })
            })
            .or_else(|| selection_bounds(state.anchor, state.cursor))
            .unwrap_or((state.cursor, state.cursor));
        let range =
            utf16_offset_for_byte(&state.value, start)..utf16_offset_for_byte(&state.value, end);
        state.cursor = replace_utf16_range(&mut state.value, range, text);
        state.anchor = None;
        state.ime_marked_text.clear();
        window.invalidate_character_coordinates();
        if history_search {
            self.set_history_query(self.history_query.value.clone(), cx);
        } else {
            cx.notify();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let history_search = self.history_search_focus.is_focused(window);
        let state = if history_search {
            &mut self.history_query
        } else {
            &mut self.commit_editor.state
        };
        let replacement = state
            .ime_replacement
            .or_else(|| {
                range.map(|range| {
                    (
                        byte_index_for_utf16(&state.value, range.start),
                        byte_index_for_utf16(&state.value, range.end),
                    )
                })
            })
            .or_else(|| state.selection())
            .unwrap_or((state.cursor, state.cursor));
        state.ime_replacement = Some(replacement);
        state.cursor = replacement.0;
        state.anchor = None;
        state.ime_marked_text = new_text.to_string();
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
        let value = if self.history_search_focus.is_focused(window) {
            &self.history_query.value
        } else {
            &self.commit_editor.state.value
        };
        let cursor = byte_index_for_utf16(value, range.start);
        let before_cursor = &value[..cursor];
        let current_line = before_cursor.rsplit('\n').next().unwrap_or("");
        let line_index = before_cursor.bytes().filter(|byte| *byte == b'\n').count();
        Some(Bounds {
            origin: Point::new(
                element_bounds.origin.x + px(12.) + text_width(window, current_line, px(12.)),
                element_bounds.origin.y + px(10. + line_index as f32 * 17.),
            ),
            size: size(px(1.), px(16.)),
        })
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        if self.history_search_focus.is_focused(_window) {
            Some(utf16_len(&self.history_query.value))
        } else {
            Some(utf16_len(&self.commit_editor.state.value))
        }
    }
}

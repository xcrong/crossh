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
        Some(utf16_slice(&self.commit_editor.state.value, range))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let (start, end) = self.commit_editor.selection().unwrap_or((
            self.commit_editor.state.cursor,
            self.commit_editor.state.cursor,
        ));
        Some(UTF16Selection {
            range: utf16_offset_for_byte(&self.commit_editor.state.value, start)
                ..utf16_offset_for_byte(&self.commit_editor.state.value, end),
            reversed: self
                .commit_editor
                .state
                .anchor
                .is_some_and(|anchor| anchor > self.commit_editor.state.cursor),
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        if self.commit_editor.state.ime_marked_text.is_empty() {
            return None;
        }
        let start = self
            .commit_editor
            .state
            .ime_replacement
            .map(|(start, _)| start)
            .unwrap_or(self.commit_editor.state.cursor);
        let start = utf16_offset_for_byte(&self.commit_editor.state.value, start);
        Some(start..start + utf16_len(&self.commit_editor.state.ime_marked_text))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((start, end)) = self.commit_editor.state.ime_replacement.take() {
            self.commit_editor.state.cursor = end;
            self.commit_editor.state.anchor = (start != end).then_some(start);
        }
        self.commit_editor.state.ime_marked_text.clear();
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
        let (start, end) = self
            .commit_editor
            .state
            .ime_replacement
            .take()
            .or_else(|| {
                replacement_range.map(|range| {
                    (
                        byte_index_for_utf16(&self.commit_editor.state.value, range.start),
                        byte_index_for_utf16(&self.commit_editor.state.value, range.end),
                    )
                })
            })
            .or_else(|| {
                selection_bounds(
                    self.commit_editor.state.anchor,
                    self.commit_editor.state.cursor,
                )
            })
            .unwrap_or((
                self.commit_editor.state.cursor,
                self.commit_editor.state.cursor,
            ));
        let range = utf16_offset_for_byte(&self.commit_editor.state.value, start)
            ..utf16_offset_for_byte(&self.commit_editor.state.value, end);
        self.commit_editor.state.cursor =
            replace_utf16_range(&mut self.commit_editor.state.value, range, text);
        self.commit_editor.state.anchor = None;
        self.commit_editor.state.ime_marked_text.clear();
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
        let replacement = self
            .commit_editor
            .state
            .ime_replacement
            .or_else(|| {
                range.map(|range| {
                    (
                        byte_index_for_utf16(&self.commit_editor.state.value, range.start),
                        byte_index_for_utf16(&self.commit_editor.state.value, range.end),
                    )
                })
            })
            .or_else(|| self.commit_editor.selection())
            .unwrap_or((
                self.commit_editor.state.cursor,
                self.commit_editor.state.cursor,
            ));
        self.commit_editor.state.ime_replacement = Some(replacement);
        self.commit_editor.state.cursor = replacement.0;
        self.commit_editor.state.anchor = None;
        self.commit_editor.state.ime_marked_text = new_text.to_string();
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
        let cursor = byte_index_for_utf16(&self.commit_editor.state.value, range.start);
        let before_cursor = &self.commit_editor.state.value[..cursor];
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
        Some(utf16_len(&self.commit_editor.state.value))
    }
}

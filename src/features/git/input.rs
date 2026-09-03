//! Git 提交信息输入、剪贴板与 IME 处理。

use std::ops::Range;

use gpui::{
    Bounds, ClipboardEntry, ClipboardItem, Context, EntityInputHandler, KeyDownEvent, Pixels,
    Point, UTF16Selection, Window, px, size,
};

use crate::shared::text_editing::{
    EditingKeystroke, TextEditingState, byte_index_for_utf16, handle_text_editing_key,
    replace_utf16_range, utf16_len, utf16_offset_for_byte, utf16_slice,
};
use crossh_ui::widgets::{printable_char, text_width};

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
                "left" => {
                    self.history_query.move_horizontal(-1, extend);
                }
                "right" => {
                    self.history_query.move_horizontal(1, extend);
                }
                "home" => {
                    self.history_query.move_to_boundary(false, extend);
                }
                "end" => {
                    self.history_query.move_to_boundary(true, extend);
                }
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

    pub(super) fn handle_remote_add_name_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "enter" | "return" => {
                window.focus(&self.remote_add_url_focus, cx);
                cx.stop_propagation();
                cx.notify();
                return;
            }
            "escape" => {
                self.close_remote_add(cx);
                cx.stop_propagation();
                return;
            }
            "tab" => {
                window.focus(&self.remote_add_url_focus, cx);
                cx.stop_propagation();
                cx.notify();
                return;
            }
            _ => {}
        }
        if dispatch_text_editing_key(&mut self.remote_add_name, event, cx) {
            cx.stop_propagation();
            cx.notify();
        }
    }

    pub(super) fn handle_remote_add_url_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "enter" | "return" => {
                self.submit_remote_add(cx);
                cx.stop_propagation();
                return;
            }
            "escape" => {
                self.close_remote_add(cx);
                cx.stop_propagation();
                return;
            }
            "tab" => {
                window.focus(&self.remote_add_name_focus, cx);
                cx.stop_propagation();
                cx.notify();
                return;
            }
            _ => {}
        }
        if dispatch_text_editing_key(&mut self.remote_add_url, event, cx) {
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
            self.commit_editor.state.clear_composition();
            self.commit_editor.state.select_all();
        } else if primary && matches!(keystroke.key.as_str(), "c" | "x") {
            if let Some(text) = self.commit_editor.state.selected_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                if keystroke.key == "x" {
                    self.commit_editor.state.clear_composition();
                    self.commit_editor.state.replace_selection("");
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
                self.commit_editor.state.clear_composition();
                self.commit_editor.state.replace_selection(&text);
            }
        } else {
            match keystroke.key.as_str() {
                "enter" | "return" if primary => self.commit_changes(cx),
                "enter" | "return" => {
                    self.commit_editor.state.clear_composition();
                    self.commit_editor.state.replace_selection("\n");
                }
                "backspace" => {
                    self.commit_editor.state.clear_composition();
                    self.commit_editor.state.backspace();
                }
                "delete" => {
                    self.commit_editor.state.clear_composition();
                    self.commit_editor.state.delete();
                }
                "left" => {
                    self.commit_editor.state.move_horizontal(-1, extend);
                }
                "right" => {
                    self.commit_editor.state.move_horizontal(1, extend);
                }
                "home" => {
                    self.commit_editor.state.move_to_boundary(false, extend);
                }
                "end" => {
                    self.commit_editor.state.move_to_boundary(true, extend);
                }
                _ => {
                    if let Some(character) = printable_char(keystroke) {
                        self.commit_editor.state.clear_composition();
                        self.commit_editor
                            .state
                            .replace_selection(&character.to_string());
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

/// 单行输入的通用按键分发：远端添加表单两字段共用。
fn dispatch_text_editing_key(
    state: &mut TextEditingState,
    event: &KeyDownEvent,
    cx: &mut Context<GitWindow>,
) -> bool {
    let keystroke = &event.keystroke;
    let primary = keystroke.modifiers.control || keystroke.modifiers.platform;
    let paste_text = if primary && keystroke.key == "v" {
        cx.read_from_clipboard().and_then(|item| {
            item.into_entries().find_map(|entry| match entry {
                ClipboardEntry::String(value) => Some(value.text),
                _ => None,
            })
        })
    } else {
        None
    };
    let editing_keystroke = EditingKeystroke {
        key: keystroke.key.clone(),
        key_char: keystroke.key_char.clone(),
        control: keystroke.modifiers.control,
        platform: keystroke.modifiers.platform,
        shift: keystroke.modifiers.shift,
    };
    let result = handle_text_editing_key(state, &editing_keystroke, paste_text.as_deref());
    if let Some(text) = result.copy_text {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }
    result.handled
}

impl GitWindow {
    /// 当前聚焦的单行文本状态：历史搜索优先，其次远端添加表单，最后提交信息。
    /// IME 回调统一经此路由，焦点归属唯一。
    fn focused_text_state(&mut self, window: &Window) -> &mut TextEditingState {
        if self.history_search_focus.is_focused(window) {
            &mut self.history_query
        } else if self.remote_add_name_focus.is_focused(window) {
            &mut self.remote_add_name
        } else if self.remote_add_url_focus.is_focused(window) {
            &mut self.remote_add_url
        } else {
            &mut self.commit_editor.state
        }
    }

    fn focused_text_state_ref(&self, window: &Window) -> &TextEditingState {
        if self.history_search_focus.is_focused(window) {
            &self.history_query
        } else if self.remote_add_name_focus.is_focused(window) {
            &self.remote_add_name
        } else if self.remote_add_url_focus.is_focused(window) {
            &self.remote_add_url
        } else {
            &self.commit_editor.state
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
        let value = &self.focused_text_state_ref(_window).value;
        Some(utf16_slice(value, range))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let state = self.focused_text_state_ref(_window);
        let (start, end) = state.selection().unwrap_or((state.cursor, state.cursor));
        Some(UTF16Selection {
            range: utf16_offset_for_byte(&state.value, start)
                ..utf16_offset_for_byte(&state.value, end),
            reversed: state.anchor.is_some_and(|anchor| anchor > state.cursor),
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let state = self.focused_text_state_ref(_window);
        if state.ime_marked_text.is_empty() {
            return None;
        }
        let start = state
            .ime_replacement
            .map(|(start, _)| start)
            .unwrap_or(state.cursor);
        let start = utf16_offset_for_byte(&state.value, start);
        Some(start..start + utf16_len(&state.ime_marked_text))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.focused_text_state(window);
        if let Some((start, end)) = state.ime_replacement.take() {
            state.cursor = end;
            state.anchor = (start != end).then_some(start);
        }
        state.ime_marked_text.clear();
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
        let state = self.focused_text_state(window);
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
        let state = self.focused_text_state(window);
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
        let value = &self.focused_text_state_ref(window).value;
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
        Some(utf16_len(&self.focused_text_state_ref(_window).value))
    }
}

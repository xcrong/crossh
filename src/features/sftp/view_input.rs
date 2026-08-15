//! SFTP text input and IME handling.

use super::*;

#[derive(Clone, Copy)]
enum SftpInputField {
    Path,
    Editor,
    Upload,
}

impl SftpPane {
    fn active_input_field(&self, window: &Window) -> Option<SftpInputField> {
        if self
            .pending_path_input
            .as_ref()
            .is_some_and(|input| input.focus.is_focused(window))
        {
            Some(SftpInputField::Path)
        } else if self
            .editor
            .as_ref()
            .is_some_and(|editor| !editor.read_only && editor.focus.is_focused(window))
        {
            Some(SftpInputField::Editor)
        } else if self.editor.is_none() && self.focus.is_focused(window) {
            Some(SftpInputField::Upload)
        } else {
            None
        }
    }

    /// 当前活动输入为 Path / Upload 时对应的 [`EndCaretInput`]；Editor 返回 `None`。
    fn active_end_caret_input(&self, field: SftpInputField) -> Option<&EndCaretInput> {
        match field {
            SftpInputField::Path => self.pending_path_input.as_ref().map(|input| &input.state),
            SftpInputField::Upload => Some(&self.upload_input),
            SftpInputField::Editor => None,
        }
    }

    fn active_end_caret_input_mut(&mut self, field: SftpInputField) -> Option<&mut EndCaretInput> {
        match field {
            SftpInputField::Path => self
                .pending_path_input
                .as_mut()
                .map(|input| &mut input.state),
            SftpInputField::Upload => Some(&mut self.upload_input),
            SftpInputField::Editor => None,
        }
    }
}

impl EntityInputHandler for SftpPane {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        match self.active_input_field(window)? {
            SftpInputField::Editor => Some(utf16_slice(&self.editor.as_ref()?.content, range)),
            field => Some(self.active_end_caret_input(field)?.text_for_range(range)),
        }
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        match self.active_input_field(window)? {
            SftpInputField::Editor => {
                let editor = self.editor.as_ref()?;
                let position = utf16_offset_for_byte(&editor.content, editor.cursor);
                Some(UTF16Selection {
                    range: position..position,
                    reversed: false,
                })
            }
            field => Some(self.active_end_caret_input(field)?.selection_range()),
        }
    }

    fn marked_text_range(
        &self,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        match self.active_input_field(window)? {
            SftpInputField::Editor => {
                let editor = self.editor.as_ref()?;
                let (start, _) = editor
                    .ime_replacement
                    .unwrap_or((editor.cursor, editor.cursor));
                (!editor.ime_marked_text.is_empty()).then(|| {
                    let start = utf16_offset_for_byte(&editor.content, start);
                    start..start + utf16_len(&editor.ime_marked_text)
                })
            }
            field => self.active_end_caret_input(field)?.marked_range(),
        }
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_input_field(window) {
            Some(SftpInputField::Editor) => {
                if let Some(editor) = &mut self.editor {
                    if let Some((_, end)) = editor.ime_replacement.take() {
                        editor.cursor = end;
                    }
                    editor.ime_marked_text.clear();
                }
            }
            field => {
                if let Some(input) = field.and_then(|f| self.active_end_caret_input_mut(f)) {
                    input.unmark();
                }
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
        match self.active_input_field(window) {
            Some(SftpInputField::Editor) => {
                if let Some(editor) = &mut self.editor {
                    let range = editor
                        .ime_replacement
                        .take()
                        .map(|(start, end)| {
                            utf16_offset_for_byte(&editor.content, start)
                                ..utf16_offset_for_byte(&editor.content, end)
                        })
                        .or(replacement_range)
                        .unwrap_or_else(|| {
                            let position = utf16_offset_for_byte(&editor.content, editor.cursor);
                            position..position
                        });
                    editor.cursor = replace_utf16_range(&mut editor.content, range, text);
                    editor.ime_marked_text.clear();
                    editor.dirty = true;
                }
            }
            field => {
                let Some(input) = field.and_then(|f| self.active_end_caret_input_mut(f)) else {
                    return;
                };
                input.replace_at_end(replacement_range, text);
            }
        }
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
        match self.active_input_field(window) {
            Some(SftpInputField::Editor) => {
                if let Some(editor) = &mut self.editor {
                    let replacement = editor
                        .ime_replacement
                        .take()
                        .or_else(|| {
                            range.map(|range| {
                                (
                                    byte_index_for_utf16(&editor.content, range.start),
                                    byte_index_for_utf16(&editor.content, range.end),
                                )
                            })
                        })
                        .unwrap_or((editor.cursor, editor.cursor));
                    editor.ime_replacement = Some(replacement);
                    editor.cursor = replacement.0;
                    editor.ime_marked_text.clear();
                    editor.ime_marked_text.push_str(new_text);
                }
            }
            field => {
                let Some(input) = field.and_then(|f| self.active_end_caret_input_mut(f)) else {
                    return;
                };
                input.mark(new_text);
            }
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
            SftpInputField::Editor => {
                let editor = self.editor.as_ref()?;
                let cursor = editor
                    .ime_replacement
                    .map(|(start, _)| start)
                    .unwrap_or_else(|| byte_index_for_utf16(&editor.content, range.start));
                let line_start = super::line_bounds(&editor.content, cursor).0;
                Some(ime_caret_bounds(
                    window,
                    element_bounds,
                    &editor.content[line_start..cursor],
                    px(12.),
                    px(42.),
                    self.editor_scroll.offset().x,
                ))
            }
            SftpInputField::Path => {
                let input = self.pending_path_input.as_ref()?;
                input
                    .state
                    .bounds_for_range(range, element_bounds, window, 14., 12.)
            }
            SftpInputField::Upload => {
                self.upload_input
                    .bounds_for_range(range, element_bounds, window, 12., 8.)
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
            SftpInputField::Editor => self
                .editor
                .as_ref()
                .map(|editor| utf16_len(&editor.content)),
            field => Some(self.active_end_caret_input(field)?.length()),
        }
    }
}

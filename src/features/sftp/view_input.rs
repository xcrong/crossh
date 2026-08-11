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
}

impl EntityInputHandler for SftpPane {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let text = match self.active_input_field(window)? {
            SftpInputField::Path => &self.pending_path_input.as_ref()?.value,
            SftpInputField::Editor => &self.editor.as_ref()?.content,
            SftpInputField::Upload => &self.upload_input,
        };
        Some(utf16_slice(text, range))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let text = match self.active_input_field(window)? {
            SftpInputField::Path => &self.pending_path_input.as_ref()?.value,
            SftpInputField::Editor => {
                let editor = self.editor.as_ref()?;
                let position = utf16_offset_for_byte(&editor.content, editor.cursor);
                return Some(UTF16Selection {
                    range: position..position,
                    reversed: false,
                });
            }
            SftpInputField::Upload => &self.upload_input,
        };
        let position = utf16_len(text);
        Some(UTF16Selection {
            range: position..position,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        match self.active_input_field(window)? {
            SftpInputField::Path => {
                let input = self.pending_path_input.as_ref()?;
                (!input.ime_marked_text.is_empty()).then(|| {
                    let start = utf16_len(&input.value);
                    start..start + utf16_len(&input.ime_marked_text)
                })
            }
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
            SftpInputField::Upload => (!self.upload_ime_marked_text.is_empty()).then(|| {
                let start = utf16_len(&self.upload_input);
                start..start + utf16_len(&self.upload_ime_marked_text)
            }),
        }
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_input_field(window) {
            Some(SftpInputField::Path) => {
                if let Some(input) = &mut self.pending_path_input {
                    input.ime_marked_text.clear();
                }
            }
            Some(SftpInputField::Editor) => {
                if let Some(editor) = &mut self.editor {
                    if let Some((_, end)) = editor.ime_replacement.take() {
                        editor.cursor = end;
                    }
                    editor.ime_marked_text.clear();
                }
            }
            Some(SftpInputField::Upload) => self.upload_ime_marked_text.clear(),
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
            Some(SftpInputField::Path) => {
                if let Some(input) = &mut self.pending_path_input {
                    let position = utf16_len(&input.value);
                    replace_utf16_range(
                        &mut input.value,
                        replacement_range.unwrap_or(position..position),
                        text,
                    );
                    input.ime_marked_text.clear();
                }
            }
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
            Some(SftpInputField::Upload) => {
                let position = utf16_len(&self.upload_input);
                replace_utf16_range(
                    &mut self.upload_input,
                    replacement_range.unwrap_or(position..position),
                    text,
                );
                self.upload_ime_marked_text.clear();
            }
            None => return,
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
            Some(SftpInputField::Path) => {
                if let Some(input) = &mut self.pending_path_input {
                    input.ime_marked_text.clear();
                    input.ime_marked_text.push_str(new_text);
                }
            }
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
            Some(SftpInputField::Upload) => {
                self.upload_ime_marked_text.clear();
                self.upload_ime_marked_text.push_str(new_text);
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
            SftpInputField::Path => {
                let input = self.pending_path_input.as_ref()?;
                let cursor = byte_index_for_utf16(&input.value, range.start);
                Some(ime_caret_bounds(
                    window,
                    element_bounds,
                    &input.value[..cursor],
                    px(14.),
                    px(12.),
                    px(0.),
                ))
            }
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
            SftpInputField::Upload => {
                let cursor = byte_index_for_utf16(&self.upload_input, range.start);
                Some(ime_caret_bounds(
                    window,
                    element_bounds,
                    &self.upload_input[..cursor],
                    px(12.),
                    px(8.),
                    px(0.),
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
            SftpInputField::Path => self
                .pending_path_input
                .as_ref()
                .map(|input| utf16_len(&input.value)),
            SftpInputField::Editor => self
                .editor
                .as_ref()
                .map(|editor| utf16_len(&editor.content)),
            SftpInputField::Upload => Some(utf16_len(&self.upload_input)),
        }
    }
}

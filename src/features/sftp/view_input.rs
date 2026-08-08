//! SFTP text input and IME handling.

use super::*;

#[derive(Clone, Copy)]
enum SftpInputField {
    Path,
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
        _range: Option<Range<usize>>,
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
            SftpInputField::Upload => Some(utf16_len(&self.upload_input)),
        }
    }
}

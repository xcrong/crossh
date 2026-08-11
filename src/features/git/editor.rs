use gpui::FocusHandle;

pub(super) struct CommitEditor {
    pub(super) value: String,
    pub(super) cursor: usize,
    pub(super) anchor: Option<usize>,
    pub(super) focus: FocusHandle,
    pub(super) ime_marked_text: String,
    pub(super) ime_replacement: Option<(usize, usize)>,
}

impl CommitEditor {
    pub(super) fn new(focus: FocusHandle) -> Self {
        Self {
            value: String::new(),
            cursor: 0,
            anchor: None,
            focus,
            ime_marked_text: String::new(),
            ime_replacement: None,
        }
    }

    pub(super) fn selection(&self) -> Option<(usize, usize)> {
        selection_bounds(self.anchor, self.cursor)
    }

    pub(super) fn replace_selection(&mut self, text: &str) {
        let (start, end) = self.selection().unwrap_or((self.cursor, self.cursor));
        self.value.replace_range(start..end, text);
        self.cursor = start + text.len();
        self.anchor = None;
    }

    pub(super) fn backspace(&mut self) {
        if let Some((start, end)) = self.selection() {
            self.value.replace_range(start..end, "");
            self.cursor = start;
            self.anchor = None;
            return;
        }
        let start = previous_char_boundary(&self.value, self.cursor);
        self.value.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    pub(super) fn delete(&mut self) {
        if let Some((start, end)) = self.selection() {
            self.value.replace_range(start..end, "");
            self.cursor = start;
            self.anchor = None;
            return;
        }
        let end = next_char_boundary(&self.value, self.cursor);
        self.value.replace_range(self.cursor..end, "");
    }

    pub(super) fn move_horizontal(&mut self, direction: i8, extend: bool) {
        if !extend && let Some((start, end)) = self.selection() {
            self.cursor = if direction < 0 { start } else { end };
            self.anchor = None;
            return;
        }
        if extend && self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }
        self.cursor = if direction < 0 {
            previous_char_boundary(&self.value, self.cursor)
        } else {
            next_char_boundary(&self.value, self.cursor)
        };
        if !extend {
            self.anchor = None;
        }
    }

    pub(super) fn move_to_boundary(&mut self, end: bool, extend: bool) {
        if extend && self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }
        self.cursor = if end { self.value.len() } else { 0 };
        if !extend {
            self.anchor = None;
        }
    }

    pub(super) fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.value.len();
    }

    pub(super) fn selected_text(&self) -> Option<String> {
        self.selection()
            .map(|(start, end)| self.value[start..end].to_string())
    }

    pub(super) fn clear_composition(&mut self) {
        self.ime_marked_text.clear();
        self.ime_replacement = None;
    }
}

pub(super) fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

pub(super) fn next_char_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .chars()
        .next()
        .map(|character| cursor + character.len_utf8())
        .unwrap_or(cursor)
}

pub(super) fn selection_bounds(anchor: Option<usize>, cursor: usize) -> Option<(usize, usize)> {
    let anchor = anchor?;
    (anchor != cursor).then_some(if anchor < cursor {
        (anchor, cursor)
    } else {
        (cursor, anchor)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn editor_keeps_unicode_cursor_on_character_boundaries(cx: &mut TestAppContext) {
        let focus = cx.update(|cx| cx.focus_handle());
        let mut editor = CommitEditor::new(focus);
        editor.replace_selection("提交😀");
        editor.backspace();
        assert_eq!(editor.value, "提交");
        assert!(editor.value.is_char_boundary(editor.cursor));
        editor.move_horizontal(-1, true);
        assert_eq!(editor.selected_text().as_deref(), Some("交"));
    }
}

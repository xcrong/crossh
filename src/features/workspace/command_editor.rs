//! Quick Command editor state and UTF-8 cursor operations.

use gpui::{FocusHandle, ScrollHandle};

pub(crate) struct QuickCommandEditor {
    pub(crate) scope: String,
    pub(crate) original: String,
    pub(crate) value: String,
    pub(crate) cursor: usize,
    pub(crate) anchor: Option<usize>,
    pub(crate) scroll: ScrollHandle,
    pub(crate) focus: FocusHandle,
    pub(crate) ime_marked_text: String,
    pub(crate) ime_replacement: Option<(usize, usize)>,
}

impl QuickCommandEditor {
    pub(crate) fn selection(&self) -> Option<(usize, usize)> {
        selection_bounds(self.anchor, self.cursor)
    }

    pub(crate) fn replace_selection(&mut self, text: &str) {
        let (start, end) = self.selection().unwrap_or((self.cursor, self.cursor));
        self.value.replace_range(start..end, text);
        self.cursor = start + text.len();
        self.anchor = None;
    }

    pub(crate) fn backspace(&mut self) {
        if let Some((start, end)) = self.selection() {
            self.value.replace_range(start..end, "");
            self.cursor = start;
            self.anchor = None;
            return;
        }
        let start = previous_char_boundary(&self.value, self.cursor);
        if start != self.cursor {
            self.value.replace_range(start..self.cursor, "");
            self.cursor = start;
        }
    }

    pub(crate) fn delete(&mut self) {
        if let Some((start, end)) = self.selection() {
            self.value.replace_range(start..end, "");
            self.cursor = start;
            self.anchor = None;
            return;
        }
        let end = next_char_boundary(&self.value, self.cursor);
        if end != self.cursor {
            self.value.replace_range(self.cursor..end, "");
        }
    }

    pub(crate) fn move_horizontal(&mut self, direction: i8, extend: bool) {
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

    pub(crate) fn move_to_boundary(&mut self, end: bool, extend: bool) {
        if extend && self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }
        self.cursor = if end { self.value.len() } else { 0 };
        if !extend {
            self.anchor = None;
        }
    }

    pub(crate) fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.value.len();
    }

    pub(crate) fn selected_text(&self) -> Option<String> {
        self.selection()
            .map(|(start, end)| self.value[start..end].to_string())
    }
}

pub(crate) fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

pub(crate) fn next_char_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .chars()
        .next()
        .map(|ch| cursor + ch.len_utf8())
        .unwrap_or(cursor)
}

pub(crate) fn selection_bounds(anchor: Option<usize>, cursor: usize) -> Option<(usize, usize)> {
    let anchor = anchor?;
    (anchor != cursor).then_some({
        if anchor < cursor {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn editor(value: &str, cx: &mut TestAppContext) -> QuickCommandEditor {
        QuickCommandEditor {
            scope: "local".into(),
            original: value.into(),
            value: value.into(),
            cursor: value.len(),
            anchor: None,
            scroll: ScrollHandle::new(),
            focus: cx.update(|cx| cx.focus_handle()),
            ime_marked_text: String::new(),
            ime_replacement: None,
        }
    }

    #[gpui::test]
    fn unicode_edits_always_leave_cursor_on_a_character_boundary(cx: &mut TestAppContext) {
        let mut editor = editor("a中😀b", cx);
        for expected in ["a中😀", "a中", "a", ""] {
            editor.backspace();
            assert_eq!(editor.value, expected);
            assert!(editor.value.is_char_boundary(editor.cursor));
        }

        editor.replace_selection("中😀");
        editor.move_to_boundary(false, false);
        editor.delete();
        assert_eq!(editor.value, "😀");
        assert!(editor.value.is_char_boundary(editor.cursor));
    }

    #[gpui::test]
    fn reversed_selection_replacement_collapses_at_inserted_text_end(cx: &mut TestAppContext) {
        let mut editor = editor("alpha中omega", cx);
        editor.cursor = 5;
        editor.anchor = Some("alpha中".len());
        assert_eq!(editor.selected_text().as_deref(), Some("中"));

        editor.replace_selection("😀");
        assert_eq!(editor.value, "alpha😀omega");
        assert_eq!(editor.cursor, "alpha😀".len());
        assert_eq!(editor.anchor, None);
    }
}

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

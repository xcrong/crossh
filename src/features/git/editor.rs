use gpui::FocusHandle;

use crate::shared::text_editing::TextEditingState;

pub(super) struct CommitEditor {
    pub(super) state: TextEditingState,
    pub(super) focus: FocusHandle,
}

impl CommitEditor {
    pub(super) fn new(focus: FocusHandle) -> Self {
        Self {
            state: TextEditingState::new(String::new()),
            focus,
        }
    }

    pub(super) fn selection(&self) -> Option<(usize, usize)> {
        self.state.selection()
    }

    pub(super) fn replace_selection(&mut self, text: &str) {
        self.state.replace_selection(text);
    }

    pub(super) fn backspace(&mut self) {
        self.state.backspace();
    }

    pub(super) fn delete(&mut self) {
        self.state.delete();
    }

    pub(super) fn move_horizontal(&mut self, direction: i8, extend: bool) {
        self.state.move_horizontal(direction, extend);
    }

    pub(super) fn move_to_boundary(&mut self, end: bool, extend: bool) {
        self.state.move_to_boundary(end, extend);
    }

    pub(super) fn select_all(&mut self) {
        self.state.select_all();
    }

    pub(super) fn selected_text(&self) -> Option<String> {
        self.state.selected_text()
    }

    pub(super) fn clear_composition(&mut self) {
        self.state.clear_composition();
    }
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
        assert_eq!(editor.state.value, "提交");
        assert!(editor.state.value.is_char_boundary(editor.state.cursor));
        editor.move_horizontal(-1, true);
        assert_eq!(editor.selected_text().as_deref(), Some("交"));
    }
}

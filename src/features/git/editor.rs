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
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn editor_keeps_unicode_cursor_on_character_boundaries(cx: &mut TestAppContext) {
        let focus = cx.update(|cx| cx.focus_handle());
        let mut editor = CommitEditor::new(focus);
        editor.state.replace_selection("提交😀");
        editor.state.backspace();
        assert_eq!(editor.state.value, "提交");
        assert!(editor.state.value.is_char_boundary(editor.state.cursor));
        editor.state.move_horizontal(-1, true);
        assert_eq!(editor.state.selected_text().as_deref(), Some("交"));
    }
}

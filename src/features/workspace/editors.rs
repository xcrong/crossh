use gpui::{FocusHandle, ScrollHandle};

use crate::features::workspace::view::LocalSessionId;
use crate::shared::text_editing::TextEditingState;

pub(crate) struct QuickCommandEditor {
    pub(crate) scope: String,
    pub(crate) original: String,
    pub(crate) state: TextEditingState,
    pub(crate) scroll: ScrollHandle,
    pub(crate) focus: FocusHandle,
}

impl QuickCommandEditor {
    pub(crate) fn new(scope: String, original: String, focus: FocusHandle) -> Self {
        Self {
            scope,
            original: original.clone(),
            state: TextEditingState::new(original),
            scroll: ScrollHandle::new(),
            focus,
        }
    }
}

pub(crate) struct RenameEditor {
    pub(crate) session_id: LocalSessionId,
    pub(crate) state: TextEditingState,
    pub(crate) focus: FocusHandle,
}

impl RenameEditor {
    pub(crate) fn new(session_id: LocalSessionId, current: String, focus: FocusHandle) -> Self {
        Self {
            session_id,
            state: TextEditingState::new(current),
            focus,
        }
    }
}

pub(crate) struct DefaultCommandEditor {
    pub(crate) session_id: LocalSessionId,
    pub(crate) state: TextEditingState,
    pub(crate) focus: FocusHandle,
}

impl DefaultCommandEditor {
    pub(crate) fn new(session_id: LocalSessionId, current: String, focus: FocusHandle) -> Self {
        Self {
            session_id,
            state: TextEditingState::new(current),
            focus,
        }
    }
}

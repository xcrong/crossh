//! 固定标签默认命令弹窗的视图侧状态。

use gpui::FocusHandle;

use crate::features::workspace::view::LocalSessionId;
use crate::shared::text_editing::TextEditingState;

pub(crate) struct DefaultCommandEditor {
    /// 被编辑的本地会话。
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

//! 固定标签重命名弹窗的视图侧状态。
//!
//! 文本编辑状态与字符边界运算位于 `crate::shared::text_editing::TextEditingState`；
//! 本文件只保留 `RenameEditor` 的 UI 侧字段（session_id/focus），调用方直接
//! 操作 `state`，与 `QuickCommandEditor` 保持同一模式。

use gpui::FocusHandle;

use crate::features::workspace::view::LocalSessionId;
use crate::shared::text_editing::TextEditingState;

pub(crate) struct RenameEditor {
    /// 被重命名的本地会话。
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

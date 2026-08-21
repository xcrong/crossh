use gpui::{AnyElement, IntoElement, SharedString, px};

use crate::hint::Hint;

/// 统一的列表空状态，收敛 `git/` 4 个页面重复的 Hint + padding。
///
/// 调用方传入已翻译的文案（`SharedString` 拥有所有权，避免跨臂临时借用
/// 的生命周期问题），组件仅负责一致的 `text_xs + faint_text` 与
/// `px(12) / px(16)` 内边距，避免各页面 `px(8)` / `px(12)` 不一致。
/// `Ready` 表示有数据，无需渲染空态，配合 `list_empty` 的 `match` 卫语句使用。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListState {
    Loading(SharedString),
    Error(SharedString),
    Empty(SharedString),
    Ready,
}

impl ListState {
    pub fn loading(text: impl Into<SharedString>) -> Self {
        Self::Loading(text.into())
    }
    pub fn error(text: impl Into<SharedString>) -> Self {
        Self::Error(text.into())
    }
    pub fn empty(text: impl Into<SharedString>) -> Self {
        Self::Empty(text.into())
    }
}

/// 将 [`ListState`] 渲染为统一的 [`Hint`] 占位元素。
///
/// `Ready` 分支不应调用本函数；若误传会 `panic` 以暴露调用错误。
pub fn list_empty(state: ListState) -> AnyElement {
    let text: SharedString = match state {
        ListState::Loading(message) => message,
        ListState::Error(message) => message,
        ListState::Empty(message) => message,
        ListState::Ready => panic!("list_empty called with Ready state"),
    };
    Hint::new(text)
        .padding_x(px(12.))
        .padding_y(px(16.))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{ListState, list_empty};

    #[test]
    fn list_state_variants_are_distinct() {
        assert_ne!(
            ListState::Loading("loading".into()),
            ListState::Empty("loading".into())
        );
        assert_ne!(
            ListState::Error("oops".into()),
            ListState::Empty("oops".into())
        );
        assert_eq!(ListState::Ready, ListState::Ready);
    }

    #[test]
    fn list_empty_creates_element_for_each_state() {
        // 仅验证构造不 panic 且返回 AnyElement（GPUI 元素需窗口上下文才能渲染像素）
        let _ = list_empty(ListState::Loading("loading".into()));
        let _ = list_empty(ListState::Error("failed".into()));
        let _ = list_empty(ListState::Empty("no data".into()));
    }

    #[test]
    #[should_panic(expected = "Ready")]
    fn list_empty_panics_on_ready() {
        let _ = list_empty(ListState::Ready);
    }
}

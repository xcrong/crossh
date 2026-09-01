use gpui::{AnyElement, IntoElement, SharedString};

use crate::hint::Hint;

/// 统一的列表空状态，收敛 `git/` 4 个页面重复的 Hint + padding。
///
/// 调用方传入已翻译的文案（`SharedString` 拥有所有权，避免跨臂临时借用
/// 的生命周期问题），组件仅负责一致的 `text_xs + faint_text` 与
/// `px(12) / px(16)` 内边距，避免各页面 `px(8)` / `px(12)` 不一致。
/// `Ready` 表示有数据，无需渲染空态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListState {
    Loading(SharedString),
    Error(SharedString),
    Empty(SharedString),
    Ready,
}

/// 统一的列表内容分发，收敛 `git/` 4 页重复的 `match { Idle|Loading => Hint, Error => Hint, Empty => Hint, Ready => uniform_list }`。
///
/// `Ready` 时调用 `rows` 产出 `uniform_list`，其余状态复用 [`list_empty`] 的统一样式（`px12/py16`）。
/// 调用方仅需将领域状态（如 `BranchListState` / `HistoryListState`）映射为 [`ListState`] 后传入。
pub fn list_state_body(state: ListState, rows: impl FnOnce() -> AnyElement) -> AnyElement {
    match state {
        ListState::Ready => rows(),
        other => list_empty(other),
    }
}

/// 将 [`ListState`] 渲染为统一的 [`Hint`] 占位元素。
///
/// `Ready` 分支不应调用本函数；若误传会 `panic` 以暴露调用错误。
fn list_empty(state: ListState) -> AnyElement {
    let text: SharedString = match state {
        ListState::Loading(message) => message,
        ListState::Error(message) => message,
        ListState::Empty(message) => message,
        ListState::Ready => panic!("list_empty called with Ready state"),
    };
    Hint::new(text).padded().into_any_element()
}

#[cfg(test)]
mod tests {
    use crate::hint::Hint;
    use gpui::IntoElement as _;

    use super::{ListState, list_state_body};

    #[test]
    fn list_state_body_dispatches_to_rows_or_hint() {
        let ready = list_state_body(ListState::Ready, || Hint::new("row").into_any_element());
        let _ = ready;
        let loading = list_state_body(ListState::Loading("loading".into()), || {
            Hint::new("row").into_any_element()
        });
        let _ = loading;
    }
}

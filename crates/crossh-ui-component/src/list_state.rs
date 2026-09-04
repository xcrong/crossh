//! 统一的列表空状态，收敛 `git/` 各页面重复的 Hint + padding。
//!
//! 空态语义（[`ListStatus`]）由 `crossh-ui-base` 拥有，本模块只负责一致的
//! `text_xs + faint_text` 与 `px(12) / px(16)` 内边距，避免各页面
//! `px(8)` / `px(12)` 不一致。调用方传入已翻译的文案（`SharedString`
//! 拥有所有权，避免跨臂临时借用的生命周期问题）。
//! `Ready` 表示有数据，无需渲染空态。

use gpui::{AnyElement, IntoElement};

use crate::hint::Hint;

pub use crossh_ui_base::ListStatus;

/// 统一的列表内容分发，收敛 `git/` 各页重复的 `match { Idle|Loading => Hint, Error => Hint, Empty => Hint, Ready => uniform_list }`。
///
/// `Ready` 时调用 `rows` 产出 `uniform_list`，其余状态复用 [`list_empty`] 的统一样式（`px12/py16`）。
/// 调用方仅需将领域状态（如 `BranchListState` / `HistoryListState`）映射为 [`ListStatus`] 后传入。
pub fn list_state_body(state: ListStatus, rows: impl FnOnce() -> AnyElement) -> AnyElement {
    match state {
        ListStatus::Ready => rows(),
        other => list_empty(other),
    }
}

/// 将 [`ListStatus`] 渲染为统一的 [`Hint`] 占位元素。
///
/// `Ready` 分支不应调用本函数；若误传会 `panic` 以暴露调用错误。
fn list_empty(state: ListStatus) -> AnyElement {
    let text = match state {
        ListStatus::Loading(message) => message,
        ListStatus::Error(message) => message,
        ListStatus::Empty(message) => message,
        ListStatus::Ready => panic!("list_empty called with Ready state"),
    };
    Hint::new(text).padded().into_any_element()
}

#[cfg(test)]
mod tests {
    use crate::hint::Hint;
    use gpui::IntoElement as _;

    use super::{ListStatus, list_state_body};

    #[test]
    fn list_state_body_dispatches_to_rows_or_hint() {
        let ready = list_state_body(ListStatus::Ready, || Hint::new("row").into_any_element());
        let _ = ready;
        let loading = list_state_body(ListStatus::Loading("loading".into()), || {
            Hint::new("row").into_any_element()
        });
        let _ = loading;
    }
}

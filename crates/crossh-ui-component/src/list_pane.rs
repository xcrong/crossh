use gpui::{
    Div, FocusHandle, InteractiveElement, SharedString, Stateful, StatefulInteractiveElement,
    Styled, div,
};

use crate::theme;

/// 统一的列表容器骨架，收敛 `git/` 4 个页面重复的 pane 容器样式。
///
/// 封装 `size_full + min_h_0 + flex_col + bg(sidebar) + focus_ring` 以及
/// `track_focus + tab_stop + on_click focus` 的焦点样板，调用方仅需
/// 串联 `key_context`、工具栏与 `on_action`。
pub fn list_pane(
    id: impl Into<SharedString>,
    focus: FocusHandle,
    key_context: &'static str,
) -> Stateful<Div> {
    let focus_for_click = focus.clone();
    div()
        .id(id.into())
        .key_context(key_context)
        .track_focus(&focus)
        .tab_stop(true)
        .size_full()
        .min_h_0()
        .flex()
        .flex_col()
        .bg(theme::sidebar())
        .focus(|style| style.border_color(theme::focus_ring()))
        .on_click(move |_event, window, cx| window.focus(&focus_for_click, cx))
}

/// 结构化封装的 `ListPane`，与 [`list_pane`] 函数等价，满足
/// Issue P0-2 建议的 `ListPane { id, focus, context, toolbar, scroll }` 形态。
///
/// 保留 `toolbar` / `scroll` 的占位语义，但渲染仍由调用方通过
/// `.child(toolbar).child(body)` 组合，避免将 `on_action` 等视图回调
/// 硬编码进组件，保持 `crossh-ui-component` 的无状态原则。
#[derive(Clone)]
pub struct ListPane {
    id: SharedString,
    focus: FocusHandle,
    context: SharedString,
}

impl ListPane {
    pub fn new(
        id: impl Into<SharedString>,
        focus: FocusHandle,
        context: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            focus,
            context: context.into(),
        }
    }

    /// 产出与 [`list_pane`] 等价的骨架 `Div`。
    ///
    /// `context` 为运行时字符串时需 `'static` 泄漏；本方法仅在调用方
    /// 已持有 `'static` 常量（如 `GIT_BRANCH_CONTEXT`）时经由 `list_pane` 函数
    /// 更高效。保留本方法以兼容结构体形态的调用示例。
    pub fn scaffold(self) -> Stateful<Div> {
        // SAFETY: 仅在测试或非热点路径使用 owned context；泄漏的字符串生命周期
        // 与进程等长，符合 `key_context(&'static str)` 的要求。
        let static_context: &'static str = Box::leak(self.context.to_string().into_boxed_str());
        list_pane(self.id, self.focus, static_context)
    }

    /// 快捷构造，直接产出骨架并允许链式追加 `toolbar` / `body`。
    pub fn div(self) -> Stateful<Div> {
        self.scaffold()
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::{ListPane, list_pane};

    #[gpui::test]
    fn list_pane_builder_is_chainable(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let focus = cx.focus_handle();
            let _pane = list_pane("test-pane", focus.clone(), "TestContext");
            let _pane2 = ListPane::new("test-pane-2", focus, "TestContext").scaffold();
        });
    }
}

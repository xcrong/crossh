//! 单行模态输入框：`TextInput` 的选中高亮 + placeholder + 横向滚动缺口。
//!
//! 复用 `TextEditingState`（value / cursor / anchor / IME）作为单一状态源，
//! 把 `view.rs` 中 3 个 `render_*_editor` 里 90% 逐行相同的
//! `div().id(...).min_h(38).px_3.py_2.flex.items_center...` 骨架收敛到一处。
//! 选中块、caret、placeholder、IME marked 与 `ime_input_canvas` 的分支与
//! 原 `render_quick_command_editor` 等逐行等价，修 IME 选中只需改此处。
//!
//! 与 `TextInput` 的差异：
//! - `TextInput` 是无选区、末尾 caret 的通用单行框（掩码、搜索等）；
//! - `ModalField` 暴露 `TextEditingState` 的选区高亮（`accent_soft` 块）
//!   与 `placeholder` + 可选横向滚动（`ScrollHandle`），专门服务模态单行编辑。

use std::rc::Rc;

use crossh_ui::widgets::{ime_input_canvas, marked_text_span, text_caret, text_span};
use gpui::{
    App, ElementId, Entity, EntityInputHandler, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, Styled,
    Window, div, px,
};

use crate::theme;

type KeyHandler = Rc<dyn Fn(&KeyDownEvent, &mut Window, &mut App)>;

/// 单行模态输入框：选中高亮 + placeholder + IME + 可选横向滚动。
///
/// 泛型 `V` 为 IME 输入目标实体，同 `TextInput` 通过 `entity()` 注册。
pub struct ModalField<V> {
    id: ElementId,
    focus: FocusHandle,
    state: SharedTextState,
    placeholder: Option<SharedString>,
    scroll: Option<gpui::ScrollHandle>,
    caret_height: gpui::Pixels,
    text_size: gpui::Pixels,
    entity: Option<Entity<V>>,
    on_key_down: Option<KeyHandler>,
}

/// 供 `ModalField` 内部使用的 `TextEditingState` 快照。
///
/// 为避免 `crossh-ui-component` 依赖上层 `shared::text_editing`，此处用
/// 与 `TextEditingState` 字段一一对应的轻量拷贝；调用方通过 `From` / `new`
/// 传入 `&TextEditingState` 即可，无需手写转换。
#[derive(Clone, Debug)]
pub struct SharedTextState {
    pub value: String,
    pub cursor: usize,
    pub anchor: Option<usize>,
    pub ime_marked_text: String,
    pub ime_replacement: Option<(usize, usize)>,
}

impl SharedTextState {
    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        (anchor != self.cursor).then_some(if anchor < self.cursor {
            (anchor, self.cursor)
        } else {
            (self.cursor, anchor)
        })
    }
}

/// 建议的收敛形态：把 3 个 `render_*_editor` 的 6 个差异参数收口为两段配置。
///
/// - `ModalTextInput` 承载输入框差异（id / placeholder / 是否需要横向滚动）
/// - `ModalDialogActions` 承载对话框差异（标题 / 图标 / 宽度 / 主按钮文案）
///
/// 三个 `render_*_editor` 通过 `render_modal_single_line_editor` 收敛到一处，
/// 仅此 6 个参数不同，其余 div 样式/选中高亮/caret/marked/ime 均由 `ModalField` 统一。
pub struct ModalTextInput {
    pub id: SharedString,
    pub focus: FocusHandle,
    pub state: SharedTextState,
    pub placeholder: SharedString,
    pub scroll: Option<gpui::ScrollHandle>,
}

impl ModalTextInput {
    pub fn new(
        id: impl Into<SharedString>,
        focus: FocusHandle,
        state: SharedTextState,
        placeholder: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            focus,
            state,
            placeholder: placeholder.into(),
            scroll: None,
        }
    }

    pub fn scrollable(mut self, handle: gpui::ScrollHandle) -> Self {
        self.scroll = Some(handle);
        self
    }
}

pub struct ModalDialogActions {
    pub title: SharedString,
    pub icon: crossh_ui::icons::IconName,
    pub width: gpui::Pixels,
    pub scrim_id: SharedString,
    pub card_id: SharedString,
    pub primary_label: SharedString,
}

impl<V> ModalField<V> {
    pub fn new(id: impl Into<ElementId>, focus: FocusHandle, state: &SharedTextState) -> Self {
        Self {
            id: id.into(),
            focus,
            state: state.clone(),
            placeholder: None,
            scroll: None,
            caret_height: px(20.),
            text_size: px(14.),
            entity: None,
            on_key_down: None,
        }
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// 仅 quick_command 等需要横向滚动的模态使用；无滚动的 rename/default 保持无 scroll。
    pub fn scrollable(mut self, handle: gpui::ScrollHandle) -> Self {
        self.scroll = Some(handle);
        self
    }

    pub fn caret_height(mut self, height: impl Into<gpui::Pixels>) -> Self {
        self.caret_height = height.into();
        self
    }

    pub fn text_size(mut self, size: impl Into<gpui::Pixels>) -> Self {
        self.text_size = size.into();
        self
    }

    pub fn entity(mut self, entity: Entity<V>) -> Self {
        self.entity = Some(entity);
        self
    }

    pub fn on_key_down(
        mut self,
        handler: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_key_down = Some(Rc::new(handler));
        self
    }
}

impl<V: EntityInputHandler + 'static> IntoElement for ModalField<V> {
    type Element = gpui::ViewElement<Self>;

    #[track_caller]
    fn into_element(self) -> Self::Element {
        gpui::ViewElement::new(self)
    }
}

impl<V: EntityInputHandler + 'static> RenderOnce for ModalField<V> {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        debug_assert!(
            self.entity.is_some(),
            "ModalField requires an entity() to register IME input"
        );
        let ModalField {
            id,
            focus,
            state,
            placeholder,
            scroll,
            caret_height,
            text_size,
            entity,
            on_key_down,
        } = self;
        let focused = focus.is_focused(window);
        let selection = state.selection();
        let (selection_start, selection_end) = selection.unwrap_or((state.cursor, state.cursor));
        let value = state.value;
        let ime_marked_text = state.ime_marked_text;

        // quick_command 在渲染前调用 `scroll.scroll_to_item(1)` 以自动滚动到光标；
        // 此处在 render 阶段复现，保持调用点精简到一行链式调用。
        if let Some(handle) = &scroll {
            handle.scroll_to_item(1);
        }

        let mut input = div()
            .id(id)
            .w_full()
            .min_w_0()
            .min_h(px(38.))
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .bg(theme::canvas())
            .border_1()
            .border_color(theme::border_strong())
            .rounded(px(theme::RADIUS_SM))
            .relative()
            .text_size(text_size)
            .text_color(theme::text())
            .track_focus(&focus)
            .tab_stop(true)
            .focus(|style| style.border_color(theme::focus_ring()))
            .on_click({
                let focus = focus.clone();
                move |_ev, window, cx| window.focus(&focus, cx)
            });

        if let Some(handle) = scroll.clone() {
            input = input.overflow_x_scroll().track_scroll(&handle);
        }

        if on_key_down.is_some() {
            let handler = on_key_down.clone();
            input = input.on_key_down(move |ev, window, cx| {
                if let Some(h) = &handler {
                    h(ev, window, cx);
                }
            });
        }

        if value.is_empty() {
            if focused {
                input = input.child(text_caret(caret_height));
            }
            if ime_marked_text.is_empty() {
                if let Some(ph) = placeholder {
                    input = input.child(
                        div()
                            .flex_shrink_0()
                            .whitespace_nowrap()
                            .text_color(theme::faint_text())
                            .child(ph),
                    );
                }
            } else {
                input = input.child(marked_text_span(ime_marked_text.clone()));
            }
        } else {
            input = input.child(text_span(value[..selection_start].to_string()));
            if let Some((start, end)) = selection {
                input = input.child(
                    div()
                        .flex_shrink_0()
                        .whitespace_nowrap()
                        .bg(theme::accent_soft())
                        .text_color(theme::text())
                        .child(SharedString::from(value[start..end].to_string())),
                );
            } else {
                if focused {
                    input = input.child(text_caret(caret_height));
                }
                if !ime_marked_text.is_empty() {
                    input = input.child(marked_text_span(ime_marked_text.clone()));
                }
            }
            input = input.child(text_span(value[selection_end..].to_string()));
        }

        input = input.child(ime_input_canvas(
            focus,
            entity.expect("entity required for IME input"),
        ));

        input
    }
}

#[cfg(test)]
mod tests {
    use gpui::{FocusHandle, TestAppContext, px};

    use super::{ModalField, SharedTextState};

    fn focus(cx: &mut TestAppContext) -> FocusHandle {
        cx.update(|cx| cx.focus_handle())
    }

    fn state(value: &str, cursor: usize, anchor: Option<usize>) -> SharedTextState {
        SharedTextState {
            value: value.to_string(),
            cursor,
            anchor,
            ime_marked_text: String::new(),
            ime_replacement: None,
        }
    }

    #[gpui::test]
    fn modal_field_defaults(cx: &mut TestAppContext) {
        let s = state("hello", 5, None);
        let field: ModalField<()> = ModalField::new("test", focus(cx), &s);
        assert_eq!(field.state.value, "hello");
        assert!(field.placeholder.is_none());
        assert!(field.scroll.is_none());
        assert_eq!(field.caret_height, px(20.));
        assert_eq!(field.text_size, px(14.));
        assert!(field.entity.is_none());
        assert!(field.on_key_down.is_none());
    }

    #[gpui::test]
    fn modal_field_builder_sets_placeholder_scroll_and_handlers(cx: &mut TestAppContext) {
        let s = state("cmd", 3, None);
        let handle = cx.update(|cx| gpui::ScrollHandle::new());
        let field: ModalField<()> = ModalField::new("prompt", focus(cx), &s)
            .placeholder("Type here")
            .scrollable(handle.clone())
            .caret_height(px(18.))
            .text_size(px(12.))
            .on_key_down(|_, _, _| {});
        assert_eq!(field.placeholder.as_deref(), Some("Type here"));
        assert!(field.scroll.is_some());
        assert_eq!(field.caret_height, px(18.));
        assert_eq!(field.text_size, px(12.));
        assert!(field.on_key_down.is_some());
    }

    #[test]
    fn shared_state_selection_bounds() {
        let s = state("hello", 2, Some(5));
        assert_eq!(s.selection(), Some((2, 5)));
        let s2 = state("hello", 5, Some(2));
        assert_eq!(s2.selection(), Some((2, 5)));
        let s3 = state("hello", 3, Some(3));
        assert_eq!(s3.selection(), None);
        let s4 = state("hello", 3, None);
        assert_eq!(s4.selection(), None);
    }
}

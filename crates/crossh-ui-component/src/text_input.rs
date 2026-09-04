//! 单行文本输入框外壳:值 / caret / placeholder / IME 标记 / IME 输入 canvas。

use std::rc::Rc;

use crossh_ui::icons;
use crossh_ui::widgets::{ime_input_canvas, marked_text_span, text_caret, text_span};
use crossh_ui_base::{
    clamp_to_char_boundary, is_valid_selection, normalize_selection, should_highlight_selection,
    use_cursor_split,
};
use gpui::{
    App, ElementId, Entity, EntityInputHandler, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, RenderOnce, Rgba, SharedString, StatefulInteractiveElement,
    Styled, Window, div, prelude::FluentBuilder, px,
};

use crate::theme;

type KeyHandler = Rc<dyn Fn(&KeyDownEvent, &mut Window, &mut App)>;

/// 单行文本输入框外壳,渲染「值 + caret + placeholder + IME 标记 + IME 输入 canvas」。
///
/// 无状态组件:值与 IME 标记文本由调用方传入声明;按键逻辑保留在各 owner
/// (调用方通过 `cx.listener(...)` 传入 [`TextInput::on_key_down`])。
/// 泛型 `V` 是 IME 输入目标,渲染时通过 [`TextInput::entity`] 注册到平台输入系统。
///
/// 内容布局:值为空时显示 caret(聚焦时)+ placeholder 或 IME 标记;
/// 非空时显示 `display`(默认取 `value`)+ caret(聚焦时)+ IME 标记。
/// 若传入 `selection`（`Some((start,end))` 字节区间）且 `display` 为空，则按
/// `ModalField` 同款高亮渲染「before + 选中块(accent_soft) + after」，不再显示 caret/IME；
/// 该扩展使 `compose_bar`/`git` 等单行输入可复用同一选中渲染路径。
pub struct TextInput<V> {
    id: ElementId,
    focus: FocusHandle,
    value: SharedString,
    display: Option<SharedString>,
    placeholder: Option<SharedString>,
    ime_marked_text: SharedString,
    selection: Option<(usize, usize)>,
    cursor: Option<usize>,
    caret_height: gpui::Pixels,
    height: gpui::Pixels,
    padding_x: gpui::Pixels,
    text_size: gpui::Pixels,
    text_color: Rgba,
    background: Rgba,
    focus_visible_accent: bool,
    flex_1: bool,
    full_width: bool,
    suffix_icon: Option<icons::IconName>,
    entity: Option<Entity<V>>,
    on_key_down: Option<KeyHandler>,
}

impl<V> TextInput<V> {
    pub fn new(id: impl Into<ElementId>, focus: FocusHandle) -> Self {
        Self {
            id: id.into(),
            focus,
            value: SharedString::default(),
            display: None,
            placeholder: None,
            ime_marked_text: SharedString::default(),
            selection: None,
            cursor: None,
            caret_height: px(16.),
            height: px(32.),
            padding_x: px(8.),
            text_size: px(12.),
            text_color: theme::text(),
            background: theme::canvas(),
            focus_visible_accent: false,
            flex_1: false,
            full_width: false,
            suffix_icon: None,
            entity: None,
            on_key_down: None,
        }
    }

    /// 输入框的实际值;决定「是否为空 → placeholder / caret」分支。
    pub fn value(mut self, value: impl Into<SharedString>) -> Self {
        self.value = value.into();
        self
    }

    /// 渲染用文本;缺省使用 `value`(掩码等场景由调用方传入 ● 串)。
    pub fn display(mut self, display: impl Into<SharedString>) -> Self {
        self.display = Some(display.into());
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// IME 组合中的标记文本;穿 underlining + accent 下划线样式。
    pub fn ime_marked_text(mut self, ime: impl Into<SharedString>) -> Self {
        self.ime_marked_text = ime.into();
        self
    }

    /// 选中区间（字节索引，左闭右开）；`None` 或 `start==end` 表示无选区。
    /// 仅在 `display` 为空时生效，掩码等场景保持原 caret-at-end 渲染。
    pub fn selection(mut self, selection: Option<(usize, usize)>) -> Self {
        self.selection = selection;
        self
    }

    /// 光标字节索引；无选区时按此位置拆分 before/caret/after，`None` 则置末尾。
    pub fn cursor(mut self, cursor: usize) -> Self {
        self.cursor = Some(cursor);
        self
    }

    pub fn caret_height(mut self, height: impl Into<gpui::Pixels>) -> Self {
        self.caret_height = height.into();
        self
    }

    pub fn height(mut self, height: impl Into<gpui::Pixels>) -> Self {
        self.height = height.into();
        self
    }

    pub fn padding_x(mut self, padding: impl Into<gpui::Pixels>) -> Self {
        self.padding_x = padding.into();
        self
    }

    pub fn text_size(mut self, size: impl Into<gpui::Pixels>) -> Self {
        self.text_size = size.into();
        self
    }

    /// 便捷方法:与 gpui `Styled::text_xs` 对齐的 12px。
    pub fn text_xs(mut self) -> Self {
        self.text_size = px(12.);
        self
    }

    /// 便捷方法:与 gpui `Styled::text_sm` 对齐的 14px。
    pub fn text_sm(mut self) -> Self {
        self.text_size = px(14.);
        self
    }

    /// 容器文字颜色(含 placeholder);必填,调用方需传入条件色。
    pub fn text_color(mut self, color: impl Into<Rgba>) -> Self {
        self.text_color = color.into();
        self
    }

    /// 容器背景色;缺省 `theme::canvas()`。
    pub fn bg(mut self, background: impl Into<Rgba>) -> Self {
        self.background = background.into();
        self
    }

    /// 键盘聚焦时用 accent 描边替代 focus_ring(对应 compose/git 输入框)。
    pub fn focus_visible_accent(mut self) -> Self {
        self.focus_visible_accent = true;
        self
    }

    /// 在横向 flex 行中撑满剩余宽度(等价原 `.flex_1().min_w_0()`)。
    pub fn flex_1(mut self) -> Self {
        self.flex_1 = true;
        self
    }

    /// 占据整行宽度(等价原 `.w_full()`)。
    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }
    /// 框内后缀图标（如筛选框的放大镜）：固定在右侧，不挤占文本。
    pub fn suffix_icon(mut self, icon: icons::IconName) -> Self {
        self.suffix_icon = Some(icon);
        self
    }

    /// IME 输入目标;渲染时以该实体向平台输入系统注册输入处理。
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

impl<V: EntityInputHandler + 'static> IntoElement for TextInput<V> {
    type Element = gpui::ViewElement<Self>;

    #[track_caller]
    fn into_element(self) -> Self::Element {
        gpui::ViewElement::new(self)
    }
}

impl<V: EntityInputHandler + 'static> RenderOnce for TextInput<V> {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        debug_assert!(
            self.entity.is_some(),
            "TextInput requires an entity() to register IME input"
        );
        let TextInput {
            id,
            focus,
            value,
            display,
            placeholder,
            ime_marked_text,
            selection,
            cursor,
            caret_height,
            height,
            padding_x,
            text_size,
            text_color,
            background,
            focus_visible_accent,
            flex_1,
            full_width,
            entity,
            on_key_down,
            suffix_icon,
        } = self;
        let focused = focus.is_focused(window);
        let display_text = display.clone().unwrap_or_else(|| value.clone());
        // 仅非掩码（display 为空）时启用选区高亮，避免 value 与 display 长度不一致导致错位。
        let masked = display.is_some();
        let has_selection = should_highlight_selection(selection, masked);
        let (sel_start, sel_end) = normalize_selection(selection).unwrap_or((0, 0));

        let mut children: Vec<gpui::AnyElement> = Vec::new();
        if value.is_empty() {
            if focused {
                children.push(text_caret(caret_height).into_any_element());
            }
            if ime_marked_text.is_empty() {
                if let Some(placeholder) = placeholder {
                    children.push(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .child(placeholder)
                            .into_any_element(),
                    );
                }
            } else {
                children.push(marked_element(ime_marked_text).into_any_element());
            }
        } else if has_selection {
            let val = value.as_ref();
            let s = sel_start.min(val.len());
            let e = sel_end.min(val.len());
            // 容错：若切片落在字符内部，回退到末尾 caret 渲染，避免 panic。
            let valid = is_valid_selection(val, s, e);
            if valid {
                if !val[..s].is_empty() {
                    children.push(text_span(val[..s].to_string()).into_any_element());
                }
                children.push(
                    div()
                        .flex_shrink_0()
                        .whitespace_nowrap()
                        .bg(theme::accent_soft())
                        .text_color(theme::text())
                        .child(SharedString::from(val[s..e].to_string()))
                        .into_any_element(),
                );
                if !val[e..].is_empty() {
                    children.push(text_span(val[e..].to_string()).into_any_element());
                }
                if !ime_marked_text.is_empty() {
                    children.push(marked_text_span(ime_marked_text).into_any_element());
                }
            } else {
                children.push(
                    div()
                        .min_w_0()
                        .flex_shrink_0()
                        .whitespace_nowrap()
                        .child(display_text)
                        .into_any_element(),
                );
                if focused {
                    children.push(text_caret(caret_height).into_any_element());
                }
                if !ime_marked_text.is_empty() {
                    children.push(marked_element(ime_marked_text).into_any_element());
                }
            }
        } else {
            let val = value.as_ref();
            let cursor_pos = clamp_to_char_boundary(val, cursor.unwrap_or(val.len()));
            // 有明确 cursor 位时按 before/caret/after 拆分，否则整体显示后置 caret。
            let use_cursor_split = use_cursor_split(cursor, masked);
            if use_cursor_split {
                if !val[..cursor_pos].is_empty() {
                    children.push(text_span(val[..cursor_pos].to_string()).into_any_element());
                }
                if focused {
                    children.push(text_caret(caret_height).into_any_element());
                }
                if !ime_marked_text.is_empty() {
                    children.push(marked_text_span(ime_marked_text).into_any_element());
                }
                if !val[cursor_pos..].is_empty() {
                    children.push(text_span(val[cursor_pos..].to_string()).into_any_element());
                }
            } else {
                children.push(
                    div()
                        .min_w_0()
                        .flex_shrink_0()
                        .whitespace_nowrap()
                        .child(display_text)
                        .into_any_element(),
                );
                if focused {
                    children.push(text_caret(caret_height).into_any_element());
                }
                if !ime_marked_text.is_empty() {
                    children.push(marked_element(ime_marked_text).into_any_element());
                }
            }
        }
        if let Some(icon) = suffix_icon {
            children.push(
                div()
                    .ml_auto()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .child(icons::icon(icon, 13.).text_color(theme::muted_text()))
                    .into_any_element(),
            );
        }

        let click_focus = focus.clone();
        div()
            .id(id)
            .relative()
            .h(height)
            .px(padding_x)
            .flex()
            .items_center()
            .overflow_x_hidden()
            .bg(background)
            .border_1()
            .border_color(theme::border_strong())
            .rounded(px(theme::RADIUS_SM))
            .text_size(text_size)
            .text_color(text_color)
            .track_focus(&focus)
            .tab_stop(true)
            .focus(|style| style.border_color(theme::focus_ring()))
            .when(focus_visible_accent, |el| {
                el.focus_visible(|style| style.border_color(theme::accent()))
            })
            .when(flex_1, |el| el.flex_1().min_w_0())
            .when(full_width, |el| el.w_full())
            .on_click(move |_ev, window, cx| window.focus(&click_focus, cx))
            .when(on_key_down.is_some(), |el| {
                el.on_key_down(move |ev, window, cx| {
                    if let Some(handler) = &on_key_down {
                        handler(ev, window, cx);
                    }
                })
            })
            .children(children)
            .child(ime_input_canvas(
                focus,
                entity.expect("entity required for IME input"),
            ))
    }
}

fn marked_element(text: SharedString) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .whitespace_nowrap()
        .underline()
        .text_decoration_color(theme::accent())
        .child(text)
}

#[cfg(test)]
mod tests {
    use gpui::{FocusHandle, TestAppContext, px};

    use super::{TextInput, theme};

    fn focus(cx: &mut TestAppContext) -> FocusHandle {
        cx.update(|cx| cx.focus_handle())
    }

    #[gpui::test]
    fn text_input_defaults(cx: &mut TestAppContext) {
        let input: TextInput<()> = TextInput::new("search", focus(cx));
        assert!(input.value.is_empty());
        assert!(input.display.is_none());
        assert!(input.placeholder.is_none());
        assert!(input.ime_marked_text.is_empty());
        assert!(input.selection.is_none());
        assert!(input.cursor.is_none());
        assert_eq!(input.caret_height, px(16.));
        assert_eq!(input.height, px(32.));
        assert_eq!(input.padding_x, px(8.));
        assert_eq!(input.text_size, px(12.));
        assert_eq!(input.text_color, theme::text());
        assert_eq!(input.background, theme::canvas());
        assert!(!input.focus_visible_accent);
        assert!(!input.flex_1);
        assert!(!input.full_width);
        assert!(input.suffix_icon.is_none());
        assert!(input.entity.is_none());
        assert!(input.on_key_down.is_none());
    }

    #[gpui::test]
    fn text_input_builder_sets_value_placeholder_display_ime_and_metrics(cx: &mut TestAppContext) {
        let masked = "••••".to_string();
        let input: TextInput<()> = TextInput::new("prompt", focus(cx))
            .value("secret")
            .display(masked.clone())
            .placeholder("Type here")
            .ime_marked_text("中")
            .selection(Some((1, 3)))
            .cursor(2)
            .caret_height(px(15.))
            .height(px(34.))
            .padding_x(px(12.))
            .text_size(px(14.))
            .text_color(theme::accent())
            .bg(theme::surface())
            .suffix_icon(crossh_ui::icons::IconName::Search)
            .focus_visible_accent()
            .on_key_down(|_, _, _| {});

        assert_eq!(input.value.as_ref(), "secret");
        assert_eq!(input.display.as_deref(), Some(masked.as_str()));
        assert_eq!(input.placeholder.as_deref(), Some("Type here"));
        assert_eq!(input.ime_marked_text.as_ref(), "中");
        assert_eq!(input.selection, Some((1, 3)));
        assert_eq!(input.cursor, Some(2));
        assert_eq!(input.caret_height, px(15.));
        assert_eq!(input.height, px(34.));
        assert_eq!(input.padding_x, px(12.));
        assert_eq!(input.text_size, px(14.));
        assert_eq!(input.text_color, theme::accent());
        assert_eq!(input.background, theme::surface());
        assert!(input.focus_visible_accent);
        assert!(input.suffix_icon.is_some());
        assert!(input.on_key_down.is_some());
    }

    #[gpui::test]
    fn text_xs_and_text_sm_are_12_and_14px(cx: &mut TestAppContext) {
        let small: TextInput<()> = TextInput::new("xs", focus(cx)).text_xs();
        let medium: TextInput<()> = TextInput::new("sm", focus(cx)).text_sm();
        assert_eq!(small.text_size, px(12.));
        assert_eq!(medium.text_size, px(14.));
    }

    #[gpui::test]
    fn layout_flags_and_focus_visible_are_sticky(cx: &mut TestAppContext) {
        let input: TextInput<()> = TextInput::new("test_input", focus(cx))
            .flex_1()
            .full_width();
        assert!(input.flex_1);
        assert!(input.full_width);
    }

    #[gpui::test]
    fn selection_and_cursor_are_stored_ordered(cx: &mut TestAppContext) {
        let with_sel: TextInput<()> = TextInput::new("sel", focus(cx)).selection(Some((5, 2)));
        assert_eq!(with_sel.selection, Some((5, 2)));
        let with_cursor: TextInput<()> = TextInput::new("cur", focus(cx)).cursor(3);
        assert_eq!(with_cursor.cursor, Some(3));
        let none: TextInput<()> = TextInput::new("none", focus(cx)).selection(None);
        assert!(none.selection.is_none());
    }
}

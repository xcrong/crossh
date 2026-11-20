//! 单行文本输入框外壳:值 / caret / placeholder / IME 标记 / IME 输入 canvas / 鼠标选区回调。

use std::cell::Cell;
use std::rc::Rc;

use crossh_ui::icons;
use crossh_ui::widgets::{ime_input_canvas, marked_text_span, text_caret, text_span, text_width};
use crossh_ui_base::{
    clamp_to_char_boundary, is_valid_selection, normalize_selection, should_highlight_selection,
    use_cursor_split,
};
use gpui::{
    App, Bounds, DispatchPhase, Div, ElementId, Entity, EntityInputHandler, FocusHandle,
    InteractiveElement, IntoElement, KeyContext, KeyDownEvent, MouseButton, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, RenderOnce, Rgba, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, canvas, div, point, prelude::FluentBuilder, px,
    size,
};

use crate::theme;

type KeyHandler = Rc<dyn Fn(&KeyDownEvent, &mut Window, &mut App)>;
type PointerHandler = Rc<dyn Fn(&TextPointerEvent, &mut Window, &mut App)>;

/// 指针事件的三个阶段;落点语义由调用方的 `TextEditingState::pointer_*` 决定。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextPointerPhase {
    Down,
    Drag,
    Up,
}

/// 输入框内容区(已去除 `padding_x`)上的指针命中结果。
#[derive(Clone, Debug)]
pub struct TextPointerEvent {
    pub phase: TextPointerPhase,
    /// 命中位置换算出的 UTF-8 字节偏移,始终落在字符边界上。
    pub byte_offset: usize,
    /// 按下时是否按住 Shift(扩展选区)。
    pub extend: bool,
    /// 连续点击次数:1 单击、2 双击选词、≥3 三击全选。
    pub click_count: usize,
    /// 内容区 bounds:拖拽阶段的偏移换算与 IME 命中测试共用。
    pub text_bounds: Bounds<Pixels>,
}

/// 把输入框内容区内的横坐标(`x` 从文本起点算起)换算成最近的 UTF-8 字符边界字节偏移。
///
/// 空串或 `x <= 0` 返回 0;`x` 不小于整段宽度返回 `text.len()`;
/// 其余情况在字符边界上二分,取更近的边界,等距取右侧,绝不返回字符内部偏移。
pub fn byte_offset_for_x(text: &str, x: Pixels, mut measure: impl FnMut(&str) -> Pixels) -> usize {
    if text.is_empty() || x <= px(0.) {
        return 0;
    }
    if x >= measure(text) {
        return text.len();
    }
    let boundaries = text
        .char_indices()
        .map(|(index, _)| index)
        .chain([text.len()])
        .collect::<Vec<_>>();
    // 第一个前缀宽度不小于 x 的边界;总宽已排除在函数开头,故该边界必落在 (0, len]。
    let (mut low, mut high) = (1, boundaries.len() - 1);
    while low < high {
        let middle = (low + high) / 2;
        if measure(&text[..boundaries[middle]]) < x {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    let left = boundaries[low - 1];
    let right = boundaries[low];
    let right_width = measure(&text[..right]);
    let left_width = measure(&text[..left]);
    if x - left_width < right_width - x {
        left
    } else {
        right
    }
}

/// 用系统 UI 字体量宽,把窗口坐标下的 x 换算成字节偏移。
pub fn text_offset_for_point(window: &Window, text: &str, x: Pixels, text_size: Pixels) -> usize {
    byte_offset_for_x(text, x, |prefix| text_width(window, prefix, text_size))
}

/// 单行文本输入框外壳,渲染「值 + caret + placeholder + IME 标记 + IME 输入 canvas」。
///
/// 无状态组件:值与 IME 标记文本由调用方传入声明;按键逻辑保留在各 owner
/// (调用方通过 `cx.listener(...)` 传入 [`TextInput::on_key_down`])。
/// 鼠标选区同样归各 owner:传入 [`TextInput::on_pointer_select`] 后,本组件只把
/// 「按下 / 拖动 / 松开」换算成字节偏移与内容区 bounds,落点由 `TextEditingState`
/// 的 `pointer_*` 方法决定;未传回调时只保留点击聚焦。
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
    on_pointer_select: Option<PointerHandler>,
    key_context: Option<KeyContext>,
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
            on_pointer_select: None,
            key_context: None,
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

    /// 鼠标点击 / 拖拽选区回调;不传则输入框只有点击聚焦。
    /// 掩码输入(`display` 非空)不会触发,避免用掩码串长度反推原文偏移。
    pub fn on_pointer_select(
        mut self,
        handler: impl Fn(&TextPointerEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_pointer_select = Some(Rc::new(handler));
        self
    }

    /// Push a key context while this input holds focus, so window-level key
    /// bindings scoped to it (Enter to confirm, Escape to dismiss, …) resolve
    /// here instead of bubbling to an ancestor context.
    pub fn key_context(mut self, context: &str) -> Self {
        self.key_context = KeyContext::try_from(context).ok();
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
            on_pointer_select,
            key_context,
            suffix_icon,
        } = self;
        let focused = focus.is_focused(window);
        let display_text = display.clone().unwrap_or_else(|| value.clone());
        // 仅非掩码（display 为空）时启用选区高亮，避免 value 与 display 长度不一致导致错位。
        let masked = display.is_some();
        let has_selection = should_highlight_selection(selection, masked);
        let (sel_start, sel_end) = normalize_selection(selection).unwrap_or((0, 0));
        let run = TextRun::new(focused, caret_height, ime_marked_text);

        let mut children: Vec<gpui::AnyElement> = if value.is_empty() {
            empty_text_children(run, placeholder)
        } else if has_selection {
            selection_text_children(&value, display_text, sel_start, sel_end, run)
        } else {
            cursor_text_children(&value, display_text, cursor, masked, run)
        };
        if let Some(icon) = suffix_icon {
            children.push(suffix_icon_child(icon));
        }

        let click_focus = focus.clone();
        // 掩码输入不发射指针事件:`value` 与掩码串长度不一致,偏移无法互推。
        let pointer = on_pointer_select.filter(|_| !masked);
        // 内容区 bounds 由下方 canvas prepaint 每帧写入,供鼠标监听与 IME 命中测试读取。
        let text_bounds: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
        let pointer_text = value;
        // 测试用选择器:元素 id 即 `VisualTestContext::debug_bounds` 的 key(发布构建为空操作)。
        let debug_selector = id.to_string();

        let mut root = div()
            .id(id)
            .debug_selector(move || debug_selector)
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
            .cursor_text()
            .when_some(key_context, |el, context| el.key_context(context))
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
            .children(children);

        if let Some(handler) = pointer {
            root = attach_pointer_handlers(
                root,
                handler,
                text_bounds,
                pointer_text,
                text_size,
                padding_x,
            );
        }

        root.child(ime_input_canvas(
            focus,
            entity.expect("entity required for IME input"),
        ))
    }
}

/// 三个 children 分支共享的聚焦 / caret / IME 标记输入。
struct TextRun {
    focused: bool,
    caret_height: Pixels,
    ime_marked_text: SharedString,
}

impl TextRun {
    fn new(focused: bool, caret_height: Pixels, ime_marked_text: SharedString) -> Self {
        Self {
            focused,
            caret_height,
            ime_marked_text,
        }
    }
}

/// 空值态的 children：聚焦 caret → placeholder，或有 IME 标记时只显示标记。
fn empty_text_children(run: TextRun, placeholder: Option<SharedString>) -> Vec<gpui::AnyElement> {
    let mut children = Vec::new();
    if run.focused {
        children.push(text_caret(run.caret_height).into_any_element());
    }
    if run.ime_marked_text.is_empty() {
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
        children.push(marked_element(run.ime_marked_text).into_any_element());
    }
    children
}

/// 选区态的 children：before → 高亮中段 → after → IME 标记；
/// 切片落在字符内部时回退为整体文本 + caret + 标记。
fn selection_text_children(
    value: &SharedString,
    display_text: SharedString,
    sel_start: usize,
    sel_end: usize,
    run: TextRun,
) -> Vec<gpui::AnyElement> {
    let mut children = Vec::new();
    let val = value.as_ref();
    let s = sel_start.min(val.len());
    let e = sel_end.min(val.len());
    // 容错：若切片落在字符内部，回退到末尾 caret 渲染，避免 panic。
    if is_valid_selection(val, s, e) {
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
        if !run.ime_marked_text.is_empty() {
            children.push(marked_text_span(run.ime_marked_text).into_any_element());
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
        if run.focused {
            children.push(text_caret(run.caret_height).into_any_element());
        }
        if !run.ime_marked_text.is_empty() {
            children.push(marked_element(run.ime_marked_text).into_any_element());
        }
    }
    children
}

/// 无选区态的 children：有明确 cursor 位时按 before/caret/标记/after 拆分，
/// 否则整体显示文本后置 caret 与标记。
fn cursor_text_children(
    value: &SharedString,
    display_text: SharedString,
    cursor: Option<usize>,
    masked: bool,
    run: TextRun,
) -> Vec<gpui::AnyElement> {
    let mut children = Vec::new();
    let val = value.as_ref();
    let cursor_pos = clamp_to_char_boundary(val, cursor.unwrap_or(val.len()));
    // 有明确 cursor 位时按 before/caret/after 拆分，否则整体显示后置 caret。
    if use_cursor_split(cursor, masked) {
        if !val[..cursor_pos].is_empty() {
            children.push(text_span(val[..cursor_pos].to_string()).into_any_element());
        }
        if run.focused {
            children.push(text_caret(run.caret_height).into_any_element());
        }
        if !run.ime_marked_text.is_empty() {
            children.push(marked_text_span(run.ime_marked_text).into_any_element());
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
        if run.focused {
            children.push(text_caret(run.caret_height).into_any_element());
        }
        if !run.ime_marked_text.is_empty() {
            children.push(marked_element(run.ime_marked_text).into_any_element());
        }
    }
    children
}

/// 给输入框挂上按下 / 移出 / 拖拽 / 松开的指针监听，落点换算为字节偏移后交给 `handler`。
fn attach_pointer_handlers(
    root: Stateful<Div>,
    handler: PointerHandler,
    text_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    pointer_text: SharedString,
    text_size: Pixels,
    padding_x: Pixels,
) -> Stateful<Div> {
    let root = attach_press_handlers(root, &handler, &text_bounds, &pointer_text, text_size);
    attach_drag_handlers(
        root,
        handler,
        text_bounds,
        pointer_text,
        text_size,
        padding_x,
    )
}

/// 按下与「在别处按下」两个即时监听。
fn attach_press_handlers(
    mut root: Stateful<Div>,
    handler: &PointerHandler,
    text_bounds: &Rc<Cell<Option<Bounds<Pixels>>>>,
    pointer_text: &SharedString,
    text_size: Pixels,
) -> Stateful<Div> {
    let down_bounds = text_bounds.clone();
    let down_handler = handler.clone();
    let down_text = pointer_text.clone();
    root = root.on_mouse_down(MouseButton::Left, move |event, window, cx| {
        let Some(bounds) = down_bounds.get() else {
            return;
        };
        down_handler(
            &TextPointerEvent {
                phase: TextPointerPhase::Down,
                byte_offset: text_offset_for_point(
                    window,
                    &down_text,
                    event.position.x - bounds.origin.x,
                    text_size,
                ),
                extend: event.modifiers.shift,
                click_count: event.click_count,
                text_bounds: bounds,
            },
            window,
            cx,
        );
    });

    // 在别处按下说明上一次按下已无从松开:补一个 Up,免得选中态一直挂着。
    let out_bounds = text_bounds.clone();
    let out_handler = handler.clone();
    root = root.on_mouse_down_out(move |_, window, cx| {
        out_handler(
            &TextPointerEvent {
                phase: TextPointerPhase::Up,
                byte_offset: 0,
                extend: false,
                click_count: 0,
                text_bounds: out_bounds.get().unwrap_or_default(),
            },
            window,
            cx,
        );
    });
    root
}

/// 按下后指针可能移出输入框,拖动 / 松开只能走窗口级监听。
fn attach_drag_handlers(
    root: Stateful<Div>,
    handler: PointerHandler,
    text_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    pointer_text: SharedString,
    text_size: Pixels,
    padding_x: Pixels,
) -> Stateful<Div> {
    let capture_bounds = text_bounds.clone();
    let drag_bounds = text_bounds.clone();
    let up_bounds = text_bounds.clone();
    let drag_handler = handler.clone();
    let up_handler = handler;
    let drag_text = pointer_text.clone();
    root.child(
        canvas(
            move |bounds, _window, _cx| {
                let width = (bounds.size.width - padding_x * 2.).max(px(0.));
                capture_bounds.set(Some(Bounds {
                    origin: point(bounds.origin.x + padding_x, bounds.origin.y),
                    size: size(width, bounds.size.height),
                }));
            },
            move |_bounds, (), window, _cx| {
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if !matches!(phase, DispatchPhase::Bubble)
                        || event.pressed_button != Some(MouseButton::Left)
                    {
                        return;
                    }
                    let Some(bounds) = drag_bounds.get() else {
                        return;
                    };
                    drag_handler(
                        &TextPointerEvent {
                            phase: TextPointerPhase::Drag,
                            byte_offset: text_offset_for_point(
                                window,
                                &drag_text,
                                event.position.x - bounds.origin.x,
                                text_size,
                            ),
                            extend: false,
                            click_count: 0,
                            text_bounds: bounds,
                        },
                        window,
                        cx,
                    );
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if !matches!(phase, DispatchPhase::Bubble) || event.button != MouseButton::Left
                    {
                        return;
                    }
                    up_handler(
                        &TextPointerEvent {
                            phase: TextPointerPhase::Up,
                            byte_offset: 0,
                            extend: false,
                            click_count: event.click_count,
                            text_bounds: up_bounds.get().unwrap_or_default(),
                        },
                        window,
                        cx,
                    );
                });
            },
        )
        .absolute()
        .left_0()
        .top_0()
        .size_full(),
    )
}

/// 框内后缀图标：固定在右侧，不挤占文本。
fn suffix_icon_child(icon: icons::IconName) -> gpui::AnyElement {
    div()
        .ml_auto()
        .flex_shrink_0()
        .flex()
        .items_center()
        .child(icons::icon(icon, 13.).text_color(theme::muted_text()))
        .into_any_element()
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
    use gpui::{FocusHandle, Pixels, TestAppContext, px};

    use super::{TextInput, byte_offset_for_x, theme};

    /// 等宽假字体:ASCII 10px、非 ASCII 20px,用来覆盖 CJK / emoji 的边界换算。
    fn wide_measure(text: &str) -> Pixels {
        px(text
            .chars()
            .map(|character| if character.is_ascii() { 10. } else { 20. })
            .sum::<f32>())
    }

    fn ascii_measure(text: &str) -> Pixels {
        px(text.chars().count() as f32 * 10.)
    }

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
        assert!(input.on_pointer_select.is_none());
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
            .on_key_down(|_, _, _| {})
            .on_pointer_select(|_, _, _| {});

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
        assert!(input.on_pointer_select.is_some());
    }

    #[test]
    fn byte_offset_for_x_handles_empty_and_out_of_range_x() {
        assert_eq!(byte_offset_for_x("", px(0.), ascii_measure), 0);
        assert_eq!(byte_offset_for_x("", px(40.), ascii_measure), 0);
        assert_eq!(byte_offset_for_x("abc", px(-8.), ascii_measure), 0);
        assert_eq!(byte_offset_for_x("abc", px(0.), ascii_measure), 0);
        assert_eq!(byte_offset_for_x("abc", px(30.), ascii_measure), 3);
        assert_eq!(byte_offset_for_x("abc", px(999.), ascii_measure), 3);
    }

    #[test]
    fn byte_offset_for_x_picks_nearest_boundary_and_ties_go_right() {
        // "abc" 每个字符 10px,边界宽度为 0 / 10 / 20 / 30。
        assert_eq!(byte_offset_for_x("abc", px(4.), ascii_measure), 0);
        assert_eq!(byte_offset_for_x("abc", px(6.), ascii_measure), 1);
        // 正好落在边界上取该边界;等距取右。
        assert_eq!(byte_offset_for_x("abc", px(10.), ascii_measure), 1);
        assert_eq!(byte_offset_for_x("abc", px(15.), ascii_measure), 2);
    }

    #[test]
    fn byte_offset_for_x_never_splits_a_codepoint() {
        // "a中😀b" 的边界是 0 / 1 / 4 / 8 / 9。
        let text = "a中😀b";
        assert_eq!(byte_offset_for_x(text, px(15.), wide_measure), 1);
        assert_eq!(byte_offset_for_x(text, px(20.), wide_measure), 4);
        assert_eq!(byte_offset_for_x(text, px(50.), wide_measure), 8);
        for x in 0..60 {
            let offset = byte_offset_for_x(text, px(x as f32), wide_measure);
            assert!(
                text.is_char_boundary(offset),
                "x = {x} produced mid-char offset {offset}"
            );
        }
    }

    #[test]
    fn byte_offset_for_x_maps_cjk_and_emoji_prefixes() {
        assert_eq!(byte_offset_for_x("中文", px(5.), wide_measure), 0);
        assert_eq!(byte_offset_for_x("中文", px(25.), wide_measure), 3);
        // 等距落在两个汉字之间时取右侧边界。
        assert_eq!(byte_offset_for_x("中文", px(30.), wide_measure), 6);
        assert_eq!(byte_offset_for_x("😀a", px(8.), wide_measure), 0);
        assert_eq!(byte_offset_for_x("😀a", px(10.), wide_measure), 4);
        assert_eq!(byte_offset_for_x("😀a", px(25.), wide_measure), 5);
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

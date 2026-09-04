// Copyright (c) 2026 Crossh contributors.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 无样式按钮行为（语义元素）：稳定 id、焦点、键盘激活、可访问性。
//!
//! 零默认视觉：不输出颜色、内外边距、圆角、阴影，表现由应用层根据
//! readers（[`BaseButton::is_selected`] / [`BaseButton::is_disabled`]）适配，
//! 或用 [`BaseButton::apply_to`] 把行为套在应用层自己的 stateful div 上。
//! `hover` / `active` 完全交给 GPUI 原生，本模块不自造悬停状态。
//!
//! 键盘激活由 GPUI 原生 Enter / Space → click 合成拥有（down/up 配对、
//! 焦点代际戳、组合键拒绝），到达时以 [`gpui::ClickEvent::Keyboard`]
//! 走同一 `on_click` 通路；本模块不再自设 `on_key_down` 与原生语义打架。

use std::rc::Rc;

use gpui::{
    App, ClickEvent, Div, ElementId, FocusHandle, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, Stateful, StatefulInteractiveElement, Styled, Window, div,
};

/// 点击回调。
type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// 激活回调（无事件载荷）：鼠标点击与键盘激活都会触发，位于 `on_click` 之后。
pub type ButtonPress = Rc<dyn Fn(&mut Window, &mut App)>;

/// 无样式按钮：只承载行为，表现由应用层决定。
///
/// 受控值：调用方传入当前 `selected` / `disabled` / `loading`，基础库只做
/// 状态机与事件转发，永不改应用状态。优先级：builder 链 < selected <
/// disabled-last（[`BaseButton::is_disabled`] 为真时去 handler、去焦点、无指针）。
#[derive(IntoElement)]
pub struct BaseButton {
    id: ElementId,
    label: Option<SharedString>,
    selected: bool,
    disabled: bool,
    loading: bool,
    on_click: Option<ClickHandler>,
    on_activate: Option<ButtonPress>,
}

impl BaseButton {
    /// 以稳定 id 创建（跨帧焦点身份即此 id）。
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: None,
            selected: false,
            disabled: false,
            loading: false,
            on_click: None,
            on_activate: None,
        }
    }

    /// 可读标签（无标签的纯图标按钮由适配层补 tooltip，本模块 debug 断言提醒）。
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// 选中态（表现由适配层决定，本模块仅携带并暴露）。
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// 禁用（与 `loading` 取或后生效，见 [`BaseButton::is_disabled`]）。
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// 加载中（视为禁用的一种，表现由适配层决定）。
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// 点击回调：鼠标点击与原生键盘合成点击都会到达。
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// 激活回调：每次激活（鼠标或键盘）在 `on_click` 之后触发一次。
    pub fn on_activate(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_activate = Some(Rc::new(handler));
        self
    }

    /// 是否选中。
    pub fn is_selected(&self) -> bool {
        self.selected
    }

    /// 是否禁用：`disabled || loading`。为真时无 handler、无焦点跟踪、无指针。
    pub fn is_disabled(&self) -> bool {
        self.disabled || self.loading
    }

    /// 是否加载中。
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// 按 id 取跨帧稳定的焦点句柄（与 [`BaseButton::apply_to`] 用同一 key）。
    pub fn focus_handle(&self, window: &mut Window, cx: &mut App) -> FocusHandle {
        window
            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone()
    }
    /// 把行为套在应用层自己的 div 上：挂 id、跟踪焦点、按 disabled 门控
    /// handler 与指针。返回的 stateful div 由应用层继续加样式。
    ///
    /// 原生键盘合成只在元素挂了 click 监听时 armed：即使只设了
    /// `on_activate` 也会注册一个转发用的 click 监听，保证键盘可达。
    pub fn apply_to(self, element: Div, focus: &FocusHandle) -> Stateful<Div> {
        let BaseButton {
            id,
            label: _,
            selected: _,
            disabled,
            loading,
            on_click,
            on_activate,
        } = self;
        let mut element = element.id(id);
        if disabled || loading {
            return element;
        }
        element = element
            .track_focus(&focus.clone().tab_stop(true).tab_index(0))
            .cursor_pointer();
        match (on_click, on_activate) {
            (Some(on_click), Some(on_activate)) => {
                element = element.on_click(move |event, window, cx| {
                    on_click(event, window, cx);
                    on_activate(window, cx);
                });
            }
            (Some(on_click), None) => {
                element = element.on_click(move |event, window, cx| {
                    on_click(event, window, cx);
                });
            }
            (None, Some(on_activate)) => {
                element = element.on_click(move |_, window, cx| {
                    on_activate(window, cx);
                });
            }
            (None, None) => {}
        }
        element
    }
}

impl RenderOnce for BaseButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        debug_assert!(
            self.label.is_some() || self.on_click.is_none(),
            "icon-only BaseButton should get a tooltip from the adapter layer"
        );
        let label = self.label.clone();
        let focus = self.focus_handle(window, cx);
        let mut element = self.apply_to(
            div().flex().flex_shrink_0().items_center().justify_center(),
            &focus,
        );
        if let Some(label) = label {
            element = element.child(label);
        }
        element
    }
}

#[cfg(test)]
mod tests {
    use gpui::{
        AnyWindowHandle, AppContext, Context, InputEvent, IntoElement, KeyDownEvent, KeyUpEvent,
        Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement, Pixels,
        Point, Render, Styled, TestAppContext, Window, div, point, px, size,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::BaseButton;

    #[test]
    fn disabled_is_disjunction_of_disabled_and_loading() {
        assert!(!BaseButton::new("a").is_disabled());
        assert!(BaseButton::new("a").disabled(true).is_disabled());
        assert!(BaseButton::new("a").loading(true).is_disabled());
        assert!(
            BaseButton::new("a")
                .disabled(true)
                .loading(true)
                .is_disabled()
        );
        assert!(!BaseButton::new("a").disabled(false).is_disabled());
    }

    #[test]
    fn builders_and_readers() {
        let button = BaseButton::new("a").label("Save").selected(true);
        assert!(button.is_selected());
        assert!(!button.is_loading());
        assert!(!button.is_disabled());
        assert!(BaseButton::new("a").loading(true).is_loading());
    }

    // ── 行为契约（GPUI harness）：鼠标与原生键盘合成点击走同一通路，
    // 每次激活触发 on_click（若设）后触发 on_activate（若设）一次 ──

    #[derive(Clone, Default)]
    struct ButtonHost {
        clicks: Rc<RefCell<usize>>,
        activates: Rc<RefCell<usize>>,
        disabled: bool,
    }

    impl Render for ButtonHost {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let clicks = self.clicks.clone();
            let activates = self.activates.clone();
            div().size_full().child(
                BaseButton::new("base-btn")
                    .label("Activate me for keyboard reach")
                    .disabled(self.disabled)
                    .on_click(move |_, window, _| {
                        *clicks.borrow_mut() += 1;
                        window.refresh();
                    })
                    .on_activate(move |window, _| {
                        *activates.borrow_mut() += 1;
                        window.refresh();
                    }),
            )
        }
    }

    fn window_of(host: &gpui::WindowHandle<ButtonHost>) -> AnyWindowHandle {
        (*host).into()
    }

    fn click(window: AnyWindowHandle, cx: &mut TestAppContext, at: Point<Pixels>) {
        cx.update_window(window, |_, window, cx| {
            window.dispatch_event(
                MouseDownEvent {
                    position: at,
                    modifiers: Modifiers::default(),
                    button: MouseButton::Left,
                    click_count: 1,
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
            window.dispatch_event(
                MouseUpEvent {
                    position: at,
                    modifiers: Modifiers::default(),
                    button: MouseButton::Left,
                    click_count: 1,
                }
                .to_platform_input(),
                cx,
            );
        })
        .unwrap();
        cx.run_until_parked();
    }

    fn press(window: AnyWindowHandle, cx: &mut TestAppContext, key: &str) {
        // 原生语义：down/up 配对才合成一次点击。
        for down in [true, false] {
            let input = if down {
                KeyDownEvent {
                    keystroke: Keystroke::parse(key).unwrap(),
                    is_held: false,
                    prefer_character_input: false,
                }
                .to_platform_input()
            } else {
                KeyUpEvent {
                    keystroke: Keystroke::parse(key).unwrap(),
                }
                .to_platform_input()
            };
            cx.update_window(window, |_, window, cx| {
                window.dispatch_event(input, cx);
            })
            .unwrap();
        }
        cx.run_until_parked();
    }

    fn counts(window: &gpui::WindowHandle<ButtonHost>, cx: &mut TestAppContext) -> (usize, usize) {
        window
            .update(cx, |this, _, _| {
                (*this.clicks.borrow(), *this.activates.borrow())
            })
            .unwrap()
    }

    #[gpui::test]
    fn click_fires_click_then_activate_once(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(800.), px(600.)), |_, _| ButtonHost::default());
        cx.run_until_parked();
        click(window_of(&window), cx, point(px(20.), px(8.)));
        assert_eq!(counts(&window, cx), (1, 1));
    }

    #[gpui::test]
    fn native_enter_and_space_activate_once_each(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(800.), px(600.)), |_, _| ButtonHost::default());
        cx.run_until_parked();
        // 点击先聚焦（mousedown 默认聚焦 track_focus 元素）。
        click(window_of(&window), cx, point(px(20.), px(8.)));
        press(window_of(&window), cx, "enter");
        assert_eq!(counts(&window, cx), (2, 2));
        press(window_of(&window), cx, "space");
        assert_eq!(counts(&window, cx), (3, 3));
    }

    #[gpui::test]
    fn modified_key_does_not_activate(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(800.), px(600.)), |_, _| ButtonHost::default());
        cx.run_until_parked();
        click(window_of(&window), cx, point(px(20.), px(8.)));
        press(window_of(&window), cx, "ctrl-enter");
        assert_eq!(counts(&window, cx), (1, 1));
    }

    #[gpui::test]
    fn disabled_button_ignores_mouse_and_keyboard(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(800.), px(600.)), |_, _| ButtonHost {
            disabled: true,
            ..Default::default()
        });
        cx.run_until_parked();
        click(window_of(&window), cx, point(px(20.), px(8.)));
        press(window_of(&window), cx, "enter");
        assert_eq!(counts(&window, cx), (0, 0));
    }
}

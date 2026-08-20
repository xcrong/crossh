//! 无状态 Select 下拉：触发器 + 视口钳制的面板 + scrim。
//!
//! 贴合现有 `Button`/`Toggle` 的 `RenderOnce` + builder 范式：父视图持有
//! `selected` 与 `is_open`，组件仅负责渲染与事件转发。

use std::rc::Rc;

use gpui::{
    Anchor, App, ClickEvent, ElementId, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window, anchored,
    deferred, div, point, prelude::FluentBuilder, px,
};

use crossh_ui::icons;

use crate::theme;

// ── 常量：仅参考 context_menu 的思路，自行定义避免直接依赖 ──
// 触发器未采集时的回退宽度
const SELECT_FALLBACK_WIDTH: f32 = 216.0;
// 面板最大高度（max_h）
const SELECT_MAX_HEIGHT: f32 = 240.0;
// 单行高度
const SELECT_ITEM_HEIGHT: f32 = 28.0;
// 触发器高度
const SELECT_TRIGGER_HEIGHT: f32 = 32.0;
// 触发器与面板的垂直间距
const SELECT_TRIGGER_GAP: f32 = 4.0;

/// 单个选项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectOption {
    /// 稳定 id，用于 element id 派生
    pub id: String,
    /// 显示文本
    pub label: SharedString,
    /// 是否禁用（不可点）
    pub disabled: bool,
}

impl SelectOption {
    /// 新建可用选项
    pub fn new(id: impl Into<String>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            disabled: false,
        }
    }

    /// 设置是否禁用
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

type ToggleHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type SelectHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// 无状态 Select：渲染触发器与可选的下拉面板。
#[derive(IntoElement)]
pub struct Select {
    id: ElementId,
    options: Vec<SelectOption>,
    selected_index: Option<usize>,
    placeholder: Option<SharedString>,
    disabled: bool,
    is_open: bool,
    on_toggle: Option<ToggleHandler>,
    on_select: Option<SelectHandler>,
}

impl Select {
    /// 以 id 创建
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            options: Vec::new(),
            selected_index: None,
            placeholder: None,
            disabled: false,
            is_open: false,
            on_toggle: None,
            on_select: None,
        }
    }

    /// 选项列表
    pub fn options(mut self, options: Vec<SelectOption>) -> Self {
        self.options = options;
        self
    }

    /// 当前选中索引
    pub fn selected(mut self, index: Option<usize>) -> Self {
        self.selected_index = index;
        self
    }

    /// 未选中时的占位文本
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// 是否禁用整个控件
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// 面板是否展开（受控）
    pub fn is_open(mut self, is_open: bool) -> Self {
        self.is_open = is_open;
        self
    }

    /// 触发器/遮罩点击时的切换回调
    pub fn on_toggle(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle = Some(Rc::new(handler));
        self
    }

    /// 选中某行时的回调
    pub fn on_select(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }
}

/// 在面板内寻找下一个可用选项（跳过 disabled），支持循环。
fn next_enabled_index(
    options: &[SelectOption],
    current: Option<usize>,
    direction: i32,
) -> Option<usize> {
    if options.is_empty() {
        return None;
    }
    // 若当前无选中，方向向下取首个可用，向上取末个可用
    if current.is_none() {
        if direction > 0 {
            for (idx, opt) in options.iter().enumerate() {
                if !opt.disabled {
                    return Some(idx);
                }
            }
        } else {
            for (idx, opt) in options.iter().enumerate().rev() {
                if !opt.disabled {
                    return Some(idx);
                }
            }
        }
        return None;
    }
    let current = current.unwrap();
    if current >= options.len() {
        return None;
    }
    let len = options.len() as i32;
    let mut idx = current as i32;
    for _ in 0..options.len() {
        idx = (idx + direction).rem_euclid(len);
        if let Some(opt) = options.get(idx as usize)
            && !opt.disabled
        {
            return Some(idx as usize);
        }
    }
    None
}

impl RenderOnce for Select {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let disabled = self.disabled;
        let is_open = self.is_open && !disabled;
        let placeholder = self.placeholder.clone();
        let selected_index = self.selected_index;
        let on_toggle = self.on_toggle.clone();
        let on_select = self.on_select.clone();
        let options = self.options.clone();

        // 焦点句柄（与 Button 相同的 keyed_state 模式）
        let focus = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let focused = focus.is_focused(window);

        // 显示文本：选中 label 或 placeholder
        let display_label: Option<SharedString> =
            selected_index.and_then(|idx| options.get(idx).map(|opt| opt.label.clone()));

        // 触发器
        let mut trigger = div()
            .id(self.id.clone())
            .h(px(SELECT_TRIGGER_HEIGHT))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .relative()
            .bg(theme::canvas())
            .border_1()
            .border_color(theme::border_strong())
            .rounded(px(theme::RADIUS_SM))
            .text_xs()
            .text_color(theme::text())
            .when(!disabled, |this| {
                this.cursor_pointer()
                    .hover(|style| style.border_color(theme::focus_ring()))
                    .track_focus(&focus.tab_stop(true).tab_index(0))
            })
            .when(disabled, |this| {
                this.cursor_default()
                    .bg(theme::surface())
                    .border_color(theme::border())
                    .text_color(theme::faint_text())
            })
            .when(focused && !disabled, |this| {
                this.border_color(theme::focus_ring())
            });

        // 键盘处理：Enter/Space 触发 toggle，Esc 关闭，Arrow 导航
        let on_toggle_for_key = on_toggle.clone();
        let on_select_for_key = on_select.clone();
        let options_for_key = options.clone();
        trigger = trigger.on_key_down(move |event: &KeyDownEvent, window, cx| {
            if disabled {
                return;
            }
            let key = event.keystroke.key.to_lowercase();
            // 兼容 " " / "space" / "enter"
            if key == "enter" || key == " " || key == "space" {
                if let Some(handler) = &on_toggle_for_key {
                    handler(&ClickEvent::default(), window, cx);
                }
            } else if key == "escape" {
                if is_open && let Some(handler) = &on_toggle_for_key {
                    handler(&ClickEvent::default(), window, cx);
                }
            } else if key == "arrowdown" || key == "down" {
                if is_open && let Some(handler) = &on_select_for_key {
                    let next = next_enabled_index(&options_for_key, selected_index, 1);
                    if let Some(idx) = next {
                        handler(idx, window, cx);
                    }
                }
            } else if (key == "arrowup" || key == "up")
                && is_open
                && let Some(handler) = &on_select_for_key
            {
                let next = next_enabled_index(&options_for_key, selected_index, -1);
                if let Some(idx) = next {
                    handler(idx, window, cx);
                }
            }
        });

        if !disabled && let Some(handler) = on_toggle.clone() {
            let handler = handler.clone();
            trigger = trigger.on_click(move |event, window, cx| {
                handler(event, window, cx);
            });
        }

        // 触发器内容：label/placeholder + chevron
        let label_content: gpui::AnyElement = if let Some(label) = display_label {
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(label)
                .into_any_element()
        } else if let Some(ph) = placeholder.clone() {
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(theme::faint_text())
                .child(ph)
                .into_any_element()
        } else {
            div().flex_1().into_any_element()
        };

        trigger = trigger
            .child(label_content)
            .child(icons::icon(icons::IconName::ChevronDown, 14.).text_color(theme::muted_text()));

        // 外层容器：触发器 + （可选）遮罩与面板
        let mut outer = div().flex().flex_col().child(trigger);

        if is_open {
            // 全屏 scrim：浮层覆盖整个窗口（snap_to_window 会把锚定在触发器
            // 上的视口尺寸遮罩对齐回窗口原点），点击任意处（含触发器自身）
            // 关闭面板并阻止冒泡，避免触发 trigger 的 on_click 二次翻转。
            let viewport = window.viewport_size();
            let scrim_id = ElementId::Name(format!("{}-scrim", self.id).into());
            let mut scrim = div()
                .id(scrim_id)
                .w(viewport.width)
                .h(viewport.height)
                .occlude();
            if let Some(handler) = on_toggle.clone() {
                scrim = scrim.on_click(move |event, window, cx| {
                    handler(event, window, cx);
                    cx.stop_propagation();
                });
            }
            let scrim_layer = deferred(anchored().snap_to_window().child(scrim)).priority(0);

            // 下拉面板：deferred 浮层锚定在触发器正下方，越界时由
            // snap_to_window_with_margin 钳制回视口（替代原手写 clamp）。
            let panel_id = ElementId::Name(format!("{}-panel", self.id).into());
            let mut panel = div()
                .id(panel_id)
                .w(px(SELECT_FALLBACK_WIDTH))
                .max_h(px(SELECT_MAX_HEIGHT))
                .p_1()
                .flex()
                .flex_col()
                .gap_1()
                .overflow_y_scroll()
                .bg(theme::overlay())
                .border_1()
                .border_color(theme::border_strong())
                .rounded(px(theme::RADIUS_SM))
                .shadow_md();

            for (idx, option) in options.iter().enumerate() {
                let selected = Some(idx) == selected_index;
                let opt_disabled = option.disabled;
                let label = option.label.clone();
                let row_id = ElementId::Name(format!("{}-option-{}", self.id, option.id).into());
                let mut row = div()
                    .id(row_id)
                    .h(px(SELECT_ITEM_HEIGHT))
                    .flex_shrink_0()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(theme::RADIUS_SM))
                    .text_xs()
                    .text_color(if opt_disabled {
                        theme::faint_text()
                    } else {
                        theme::text()
                    })
                    .when(selected && !opt_disabled, |this| {
                        this.bg(theme::accent_soft())
                    })
                    .when(opt_disabled, |this| this.cursor_default())
                    .when(!opt_disabled, |this| {
                        this.cursor_pointer()
                            .hover(|style| style.bg(theme::raised()))
                    });

                if !opt_disabled && let Some(handler) = on_select.clone() {
                    row = row.on_click(move |_event, window, cx| {
                        // 阻止冒泡：面板是触发器的子元素，若不拦截，
                        // 行点击会继续冒泡到触发器的 on_click 二次翻转。
                        cx.stop_propagation();
                        handler(idx, window, cx);
                    });
                }

                // 选中标识
                if selected {
                    row = row.child(icons::icon(icons::IconName::Check, 13.).text_color(
                        if opt_disabled {
                            theme::faint_text()
                        } else {
                            theme::accent()
                        },
                    ));
                } else {
                    row = row.child(div().w(px(13.)).h(px(13.)));
                }

                row = row.child(div().flex_1().min_w_0().truncate().child(label));

                panel = panel.child(row);
            }

            let panel_layer = deferred(
                anchored()
                    .anchor(Anchor::TopLeft)
                    .offset(point(
                        px(0.),
                        px(SELECT_TRIGGER_HEIGHT + SELECT_TRIGGER_GAP),
                    ))
                    .snap_to_window_with_margin(px(8.))
                    .child(panel),
            )
            .priority(1);

            outer = outer.child(scrim_layer).child(panel_layer);
        }

        outer
    }
}

#[cfg(test)]
mod tests {
    use super::{Select, SelectOption, next_enabled_index};
    use gpui::prelude::*;
    use gpui::{
        Context, ElementId, InputEvent, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent,
        Pixels, Point, TestAppContext, Window, div, point, px, size,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn select_builder_keeps_fields() {
        let opts = vec![
            SelectOption::new("a", "Alpha"),
            SelectOption::new("b", "Beta").disabled(true),
        ];
        let select = Select::new("test-select")
            .options(opts.clone())
            .selected(Some(0))
            .placeholder("请选择")
            .disabled(true)
            .is_open(true)
            .on_toggle(|_, _, _| {})
            .on_select(|_, _, _| {});

        assert_eq!(select.id, ElementId::Name("test-select".into()));
        assert_eq!(select.options, opts);
        assert_eq!(select.selected_index, Some(0));
        assert_eq!(select.placeholder.as_deref(), Some("请选择"));
        assert!(select.disabled);
        assert!(select.is_open);
        assert!(select.on_toggle.is_some());
        assert!(select.on_select.is_some());
    }

    #[test]
    fn next_enabled_skips_disabled_and_wraps() {
        let opts = vec![
            SelectOption::new("a", "A"),
            SelectOption::new("b", "B").disabled(true),
            SelectOption::new("c", "C"),
        ];
        // 当前 0，向下应跳过 1 到 2
        assert_eq!(next_enabled_index(&opts, Some(0), 1), Some(2));
        // 2 向下应回到 0（循环）
        assert_eq!(next_enabled_index(&opts, Some(2), 1), Some(0));
        // 0 向上应到 2
        assert_eq!(next_enabled_index(&opts, Some(0), -1), Some(2));
        // 无选中时向下取首个可用
        assert_eq!(next_enabled_index(&opts, None, 1), Some(0));
        // 全禁用返回 None
        let all_disabled = vec![
            SelectOption::new("a", "A").disabled(true),
            SelectOption::new("b", "B").disabled(true),
        ];
        assert_eq!(next_enabled_index(&all_disabled, Some(0), 1), None);
        assert_eq!(next_enabled_index(&all_disabled, None, 1), None);
    }

    #[test]
    fn select_option_disabled_builder() {
        let opt = SelectOption::new("x", "X").disabled(true);
        assert!(opt.disabled);
        assert_eq!(opt.id, "x");
        assert_eq!(opt.label.as_ref(), "X");
    }

    // ── 行为契约 ──────────────────────────────────────────────────────────
    // 回归用例：点击外部关闭、点击已选/首项触发 on_select 并关闭、打开时
    // 点击触发器关闭（不二次翻转）。坐标假设：触发器在窗口左上角 (0,0) 起、
    // 高度 32px；面板锚定在触发器正下方 36px，宽度 216px。
    //
    // 宿主状态放在 Rc<RefCell>：事件回调不更新 root view（GPUI 中窗口根的
    // 事件派发运行在其更新上下文中，对同一实体再 update 会 re-entrant
    // panic），只写共享状态并 `window.refresh()` 触发重绘。

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    struct HostState {
        is_open: bool,
        toggles: usize,
        selects: Vec<usize>,
    }

    #[derive(Clone)]
    struct SelectHost {
        state: Rc<RefCell<HostState>>,
    }

    impl SelectHost {
        fn view(state: HostState) -> Self {
            Self {
                state: Rc::new(RefCell::new(state)),
            }
        }
    }

    impl Render for SelectHost {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let options = vec![
                SelectOption::new("auto", "Auto detect"),
                SelectOption::new("editor-a", "Editor A"),
                SelectOption::new("editor-b", "Editor B"),
            ];
            let open = self.state.borrow().is_open;
            let selected = self.state.borrow().selects.last().copied();
            let state = self.state.clone();
            let toggle_state = self.state.clone();
            div().size_full().child(
                Select::new("sel")
                    .options(options)
                    .selected(selected)
                    .placeholder("Choose editor")
                    .is_open(open)
                    .on_toggle(move |_ev, window, _cx| {
                        let mut state = toggle_state.borrow_mut();
                        state.toggles += 1;
                        state.is_open = !state.is_open;
                        drop(state);
                        window.refresh();
                    })
                    .on_select(move |idx, window, _cx| {
                        let mut state = state.borrow_mut();
                        state.selects.push(idx);
                        state.is_open = false;
                        drop(state);
                        window.refresh();
                    }),
            )
        }
    }

    #[allow(clippy::type_complexity)]
    fn click(
        window: &gpui::WindowHandle<SelectHost>,
        cx: &mut TestAppContext,
        position: Point<Pixels>,
    ) {
        window
            .update(cx, |_, window, cx| {
                window.dispatch_event(
                    MouseDownEvent {
                        position,
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
                        position,
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

    fn snapshot(window: &gpui::WindowHandle<SelectHost>, cx: &mut TestAppContext) -> HostState {
        window
            .update(cx, |this, _, _| this.state.borrow().clone())
            .unwrap()
    }

    #[gpui::test]
    fn clicking_outside_dismisses_open_select(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(800.), px(600.)), |_, _| {
            SelectHost::view(HostState {
                is_open: true,
                ..Default::default()
            })
        });
        cx.run_until_parked();
        assert!(snapshot(&window, cx).is_open);

        // 点击触发器与面板之外的空白区域
        click(&window, cx, point(px(700.), px(500.)));

        let host = snapshot(&window, cx);
        assert!(!host.is_open, "点击外部后面板应关闭");
        assert_eq!(host.toggles, 1, "外部点击只应关闭一次，不应二次翻转");
    }

    #[gpui::test]
    fn clicking_trigger_while_open_closes_without_reopening(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(800.), px(600.)), |_, _| {
            SelectHost::view(HostState {
                is_open: true,
                ..Default::default()
            })
        });
        cx.run_until_parked();

        // 点击触发器自身（命中拦截遮罩）
        click(&window, cx, point(px(400.), px(16.)));

        let host = snapshot(&window, cx);
        assert!(!host.is_open, "打开时点击触发器应关闭面板");
        assert_eq!(host.toggles, 1, "只应关闭一次，不能冒泡到触发器二次翻转");
    }

    #[gpui::test]
    fn clicking_option_selects_and_closes(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(800.), px(600.)), |_, _| {
            SelectHost::view(HostState {
                is_open: true,
                ..Default::default()
            })
        });
        cx.run_until_parked();

        // 点击面板内第一个选项（触发器左上角 + 36px 下方、第 0 行中心）
        click(&window, cx, point(px(100.), px(54.)));

        let host = snapshot(&window, cx);
        assert_eq!(host.selects, vec![0], "点击首项应回调 on_select(0)");
        assert!(!host.is_open, "on_select 后调用方关闭面板");
        assert_eq!(host.toggles, 0, "点击选项不应冒泡为触发器 toggle");
    }

    #[gpui::test]
    fn clicking_trigger_opens_closed_select(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(800.), px(600.)), |_, _| {
            SelectHost::view(HostState::default())
        });
        cx.run_until_parked();
        assert!(!snapshot(&window, cx).is_open);

        click(&window, cx, point(px(400.), px(16.)));

        let host = snapshot(&window, cx);
        assert!(host.is_open, "关闭时点击触发器应打开面板");
        assert_eq!(host.toggles, 1);
    }
}

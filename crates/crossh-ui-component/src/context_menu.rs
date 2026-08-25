//! 轻量右键上下文菜单：类型 + 定位 + 渲染。
//!
//! 自 `crossh-ui` 抽取的通用实现，归属组件库后由 `GitView` / `AppShell` /
//! `Terminal` / `Sftp` 等宿主共享。`crossh-ui` 的 `Menu`/`MenuItem` 仅用于
//! macOS 菜单栏，这里按既有样式自建：全屏 scrim（点击即关闭）+ 定位菜单。
//! 菜单状态由各拥有者持有，渲染时作为根 div 的最后一个 child 挂载，保证 z 序最高。

use std::rc::Rc;

use crossh_ui::{icons, theme};
use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels,
    Point, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

/// 菜单固定宽度；定位钳制时按此估算。
pub const CONTEXT_MENU_WIDTH: f32 = 216.0;
/// 单个菜单项高度。
const ITEM_HEIGHT: f32 = 28.0;
/// 分隔线行高。
const SEPARATOR_HEIGHT: f32 = 8.0;
/// 分组标题高度。
const SECTION_HEADER_HEIGHT: f32 = 24.0;
/// 菜单上下内边距。
const MENU_PADDING: f32 = 8.0;

/// 单个菜单项。
#[derive(Clone)]
pub struct MenuItem<A> {
    /// 稳定 id（用于 hit-test 定位）。
    pub id: String,
    /// 显示文案（已 i18n）。
    pub label: String,
    /// 右侧快捷键提示文字。
    pub shortcut_hint: Option<String>,
    pub disabled: bool,
    /// 危险操作（删除类）用红色。
    pub danger: bool,
    pub action: A,
}

/// 菜单条目：项或分隔线。
#[derive(Clone)]
pub enum MenuEntry<A> {
    /// 不可点击的视觉分组标题。
    SectionHeader(String),
    Item(MenuItem<A>),
    /// A menu item with a persistent on/off check mark.
    CheckedItem {
        item: MenuItem<A>,
        checked: bool,
    },
    Separator,
}

/// 打开的上下文菜单快照。
#[derive(Clone)]
pub struct ContextMenuState<A> {
    /// 鼠标按下时的窗口坐标。
    pub position: Point<Pixels>,
    pub entries: Vec<MenuEntry<A>>,
}

/// 按条目数量估算菜单高度（定位钳制用）。
pub fn estimate_menu_height<A>(entries: &[MenuEntry<A>]) -> f32 {
    MENU_PADDING * 2.0
        + entries
            .iter()
            .map(|entry| match entry {
                MenuEntry::Item(_) | MenuEntry::CheckedItem { .. } => ITEM_HEIGHT,
                MenuEntry::Separator => SEPARATOR_HEIGHT,
                MenuEntry::SectionHeader(_) => SECTION_HEADER_HEIGHT,
            })
            .sum::<f32>()
}

/// 把菜单位置钳制到窗口内：默认向右下展开，越界则向左上翻转。
pub fn clamp_menu_position<A>(
    position: Point<Pixels>,
    window: &Window,
    entries: &[MenuEntry<A>],
) -> Point<Pixels> {
    let viewport = window.viewport_size();
    let mut x = position.x.as_f32();
    let mut y = position.y.as_f32();
    if x + CONTEXT_MENU_WIDTH > viewport.width.as_f32() {
        x = (x - CONTEXT_MENU_WIDTH).max(0.0);
        if x + CONTEXT_MENU_WIDTH > viewport.width.as_f32() {
            x = (viewport.width.as_f32() - CONTEXT_MENU_WIDTH).max(0.0);
        }
    }
    let height = estimate_menu_height(entries)
        .min((viewport.height.as_f32() - MENU_PADDING * 2.0).max(ITEM_HEIGHT));
    if y + height > viewport.height.as_f32() {
        y = (y - height).max(0.0);
        if y + height > viewport.height.as_f32() {
            y = (viewport.height.as_f32() - height - MENU_PADDING).max(0.0);
        }
    }
    Point::new(px(x.round()), px(y.round()))
}

/// 渲染 scrim + 菜单。scrim 覆盖拥有者根 div，点击任意处（左/右键）关闭。
///
/// `position` 为窗口坐标（即 `MouseDownEvent.position`）；`anchor` 是拥有者
/// 根 div 在窗口坐标系中的原点（AppShell 传 (0,0)），用于换算菜单的相对定位。
pub fn render_context_menu<A: Clone + 'static, T: 'static>(
    state: &ContextMenuState<A>,
    anchor: Point<Pixels>,
    window: &mut Window,
    cx: &mut Context<T>,
    on_action: impl Fn(&mut T, A, &mut Window, &mut Context<T>) + 'static,
    on_dismiss: impl Fn(&mut T, &mut Context<T>) + 'static,
) -> AnyElement {
    let position = clamp_menu_position(state.position, window, &state.entries);
    let relative = Point::new(position.x - anchor.x, position.y - anchor.y);
    let max_height = (window.viewport_size().height.as_f32() - MENU_PADDING * 2.0).max(ITEM_HEIGHT);
    let on_action = Rc::new(on_action);
    let on_dismiss = Rc::new(on_dismiss);

    let scrim = div()
        .id("ctx-scrim")
        .absolute()
        .left_0()
        .top_0()
        .size_full()
        .occlude()
        .on_click({
            let on_dismiss = on_dismiss.clone();
            cx.listener(move |this, _ev, _window, cx| {
                on_dismiss(this, cx);
                cx.stop_propagation();
            })
        })
        .on_mouse_down(MouseButton::Right, {
            let on_dismiss = on_dismiss.clone();
            cx.listener(move |this, _ev, _window, cx| {
                on_dismiss(this, cx);
                cx.stop_propagation();
            })
        });

    let mut menu = div()
        .id("ctx-menu")
        .absolute()
        .left(px(relative.x.as_f32().round()))
        .top(px(relative.y.as_f32().round()))
        .w(px(CONTEXT_MENU_WIDTH))
        .max_h(px(max_height))
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

    for entry in &state.entries {
        let (item, checked, checkable) = match entry {
            MenuEntry::SectionHeader(label) => {
                menu = menu.child(
                    div()
                        .h(px(SECTION_HEADER_HEIGHT))
                        .flex_shrink_0()
                        .px_2()
                        .flex()
                        .items_center()
                        .text_xs()
                        .text_color(theme::faint_text())
                        .child(SharedString::from(label.clone())),
                );
                continue;
            }
            MenuEntry::Separator => {
                menu = menu.child(
                    div()
                        .h(px(1.))
                        .flex_shrink_0()
                        .mx_2()
                        .my_1()
                        .bg(theme::border()),
                );
                continue;
            }
            MenuEntry::Item(item) => (item, false, false),
            MenuEntry::CheckedItem { item, checked } => (item, *checked, true),
        };

        let id = item.id.clone();
        let action = item.action.clone();
        let label = item.label.clone();
        let hint = item.shortcut_hint.clone();
        let disabled = item.disabled;
        let danger = item.danger;
        let mut row = div()
            .id(SharedString::from(format!("ctx-item-{id}")))
            .h(px(ITEM_HEIGHT))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .text_xs()
            .text_color(if disabled {
                theme::faint_text()
            } else if danger {
                theme::danger()
            } else {
                theme::text()
            });
        if disabled {
            row = row.cursor_default();
        } else {
            row = row
                .cursor_pointer()
                .hover(|s| s.bg(theme::raised()))
                .on_click({
                    let on_action = on_action.clone();
                    cx.listener(move |this, _ev, window, cx| {
                        on_action(this, action.clone(), window, cx);
                    })
                });
        }
        if checkable {
            if checked {
                row = row.child(
                    icons::icon(icons::IconName::Check, 13.).text_color(if disabled {
                        theme::faint_text()
                    } else {
                        theme::accent()
                    }),
                );
            } else {
                row = row.child(div().w(px(13.)).h(px(13.)));
            }
        }
        row = row.child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(SharedString::from(label)),
        );
        if let Some(hint) = hint {
            row = row.child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(theme::faint_text())
                    .child(SharedString::from(hint)),
            );
        }
        menu = menu.child(row);
    }

    div()
        .absolute()
        .left_0()
        .top_0()
        .size_full()
        .child(scrim)
        .child(menu)
        .into_any_element()
}

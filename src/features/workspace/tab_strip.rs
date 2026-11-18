//! 顶部标签条：活跃项目的 tab、状态点、右键菜单与拖拽重排。
//!
//! 标题、默认命令与 cwd 全部来自快照；本模块只组装视图与菜单动作。

use gpui::{
    AnyElement, AppContext, ClickEvent, Context, ElementId, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, SharedString, StatefulInteractiveElement, Styled,
    Window, div, px,
};

use crate::features::workspace::projection;
use crate::features::workspace::shell::AppShell;
use crate::features::workspace::status::{
    local_tab_dot_color, program_status_dot_color, program_status_progress_suffix,
    program_status_tooltip,
};
use crossh_core::i18n;
use crossh_session::workspace::{ProjectId, TabId};
use crossh_terminal_view::TerminalItem as _;
use crossh_ui::context_menu::ShellMenuAction;
use crossh_ui::{icons, theme};
use crossh_ui_component::context_menu::{MenuEntry, MenuItem};
use crossh_ui_component::{
    Button, ButtonSize, ButtonVariant, DragLocalTab, TabItem, TabStrip, Tooltip,
};

// 容器不绑定 click；标签名与关闭按钮分别绑定，避免事件叠加。
#[allow(clippy::too_many_arguments)]
fn render_tab_chip<M, S, C>(
    cx: &mut Context<AppShell>,
    container_id: impl Into<gpui::ElementId>,
    dot_color: gpui::Rgba,
    label: impl Into<SharedString>,
    tooltip: Option<String>,
    is_active: bool,
    leading: Option<AnyElement>,
    label_id: impl Into<ElementId>,
    menu: M,
    on_select: S,
    close_id: impl Into<gpui::ElementId>,
    on_close: C,
    tab_id: TabId,
) -> AnyElement
where
    M: Fn(&MouseDownEvent, &mut AppShell, &mut Window, &mut Context<AppShell>) + 'static,
    S: Fn(&ClickEvent, &mut AppShell, &mut Window, &mut Context<AppShell>) + 'static,
    C: Fn(&ClickEvent, &mut AppShell, &mut Window, &mut Context<AppShell>) + 'static,
{
    let label: SharedString = label.into();
    let mut tab = TabItem::new(container_id, label.clone());
    if let Some(tooltip) = tooltip {
        tab = tab.tooltip(tooltip);
    }
    tab = tab
        .drag_tab(DragLocalTab::new(tab_id.0, false, label))
        .on_drop_tab(cx.listener(move |this, drag: &DragLocalTab, _window, cx| {
            this.reorder_tab(TabId(drag.session_id()), tab_id, cx);
        }))
        .label_id(label_id)
        .active(is_active)
        .dot_color(dot_color)
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, ev: &MouseDownEvent, window, cx| menu(ev, this, window, cx)),
        )
        .on_select(
            cx.listener(move |this, ev: &ClickEvent, window, cx| on_select(ev, this, window, cx)),
        )
        .child(
            Button::new(close_id)
                .size(ButtonSize::Icon(px(24.)))
                .variant(ButtonVariant::Ghost)
                .icon(icons::icon(icons::IconName::X, 13.).text_color(theme::muted_text()))
                .tooltip(i18n::text("tooltip.close_tab"))
                .on_click(
                    cx.listener(move |this, ev: &ClickEvent, w, cx| on_close(ev, this, w, cx)),
                ),
        );
    if let Some(leading) = leading {
        tab = tab.leading_icon(leading);
    }
    tab.into_any_element()
}

pub(crate) fn tab_menu_entries(
    tab_id: TabId,
    project_id: ProjectId,
    is_active: bool,
    single: bool,
    cwd: std::path::PathBuf,
    default_command: Option<String>,
    is_command_running: bool,
) -> Vec<MenuEntry<ShellMenuAction>> {
    let mut entries = vec![MenuEntry::Item(MenuItem {
        id: "switch".into(),
        label: i18n::text("context_menu.switch"),
        shortcut_hint: None,
        disabled: is_active,
        danger: false,
        action: ShellMenuAction::ActivateTab(tab_id.0),
    })];
    entries.push(MenuEntry::Item(MenuItem {
        id: "new-tab".into(),
        label: i18n::text("context_menu.new_tab"),
        shortcut_hint: None,
        disabled: false,
        danger: false,
        action: ShellMenuAction::CreateTab(project_id.0),
    }));
    entries.push(MenuEntry::Item(MenuItem {
        id: "rename".into(),
        label: i18n::text("context_menu.rename_tab"),
        shortcut_hint: None,
        disabled: false,
        danger: false,
        action: ShellMenuAction::RenameTab(tab_id.0),
    }));
    entries.push(MenuEntry::Item(MenuItem {
        id: "edit-default-command".into(),
        label: i18n::text("context_menu.edit_default_command"),
        shortcut_hint: None,
        disabled: false,
        danger: false,
        action: ShellMenuAction::EditDefaultCommand(tab_id.0),
    }));
    let has_command = default_command
        .as_ref()
        .is_some_and(|cmd| !cmd.trim().is_empty());
    entries.push(MenuEntry::Item(MenuItem {
        id: "reload-default-command".into(),
        label: i18n::text("context_menu.reload_default_command"),
        shortcut_hint: None,
        disabled: !has_command || is_command_running,
        danger: false,
        action: ShellMenuAction::ReloadDefaultCommand(tab_id.0),
    }));
    entries.push(MenuEntry::Item(MenuItem {
        id: "clear-default-command".into(),
        label: i18n::text("context_menu.clear_default_command"),
        shortcut_hint: None,
        disabled: !has_command,
        danger: false,
        action: ShellMenuAction::ClearDefaultCommand(tab_id.0),
    }));
    entries.extend([
        MenuEntry::Item(MenuItem {
            id: "close".into(),
            label: i18n::text("context_menu.close"),
            shortcut_hint: None,
            disabled: false,
            danger: false,
            action: ShellMenuAction::CloseTab(tab_id.0),
        }),
        MenuEntry::Item(MenuItem {
            id: "close-others".into(),
            label: i18n::text("context_menu.close_others"),
            shortcut_hint: None,
            disabled: single,
            danger: false,
            action: ShellMenuAction::CloseOtherTabs {
                project_id: project_id.0,
                keep: tab_id.0,
            },
        }),
        MenuEntry::Separator,
        MenuEntry::Item(MenuItem {
            id: "copy-path".into(),
            label: i18n::text("context_menu.copy_path"),
            shortcut_hint: None,
            disabled: false,
            danger: false,
            action: ShellMenuAction::CopyText(cwd.to_string_lossy().to_string()),
        }),
        MenuEntry::Item(MenuItem {
            id: "reveal-finder".into(),
            label: i18n::text("context_menu.reveal_in_finder"),
            shortcut_hint: None,
            disabled: false,
            danger: false,
            action: ShellMenuAction::RevealInFinder(cwd),
        }),
    ]);
    entries
}

pub(super) fn render_tab_strip(shell: &AppShell, cx: &mut Context<AppShell>) -> impl IntoElement {
    let mut strip = TabStrip::new("tab-strip").track_scroll(&shell.tab_scroll);

    if let Some(project) = shell.workspace.active_project() {
        let project_id = project.id;
        let single = project.tabs.len() == 1;
        for (index, tab) in project.tabs.iter().enumerate() {
            let tab_id = tab.id;
            let is_active = project.active_tab == Some(tab_id);
            let view = tab
                .active_pane
                .and_then(|pane_id| tab.layout.find_pane(pane_id))
                .and_then(|pane| shell.workspace.pane_view(pane.id))
                .or_else(|| {
                    tab.layout
                        .leaves()
                        .find_map(|pane| shell.workspace.pane_view(pane.id))
                });
            let state = view.map(|view| view.state(cx));
            let command_running = view.is_some_and(|view| view.is_command_running(cx));
            let program_status = view.and_then(|view| view.program_status(cx));
            let label = projection::tab_title(tab, index);
            let label = match program_status_progress_suffix(&program_status) {
                Some(suffix) => format!("{label}{suffix}"),
                None => label,
            };
            let cwd = tab
                .active_pane
                .and_then(|pane_id| tab.layout.find_pane(pane_id))
                .or_else(|| tab.layout.leaves().next())
                .map(|pane| pane.cwd.clone())
                .unwrap_or_default();
            let default_command = tab.default_command.clone();
            let has_default_command = default_command.is_some();
            let tooltip = program_status_tooltip(&program_status);
            let leading = has_default_command.then(|| {
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(icons::icon(icons::IconName::Play, 8.).text_color(theme::muted_text()))
                    .into_any_element()
            });
            strip = strip.child(render_tab_chip(
                cx,
                ("tab-container", tab_id.0),
                program_status_dot_color(&program_status)
                    .unwrap_or_else(|| local_tab_dot_color(&state, command_running)),
                label,
                tooltip,
                is_active,
                leading,
                ("tab", tab_id.0),
                move |ev: &MouseDownEvent, this, _window, cx| {
                    let entries = tab_menu_entries(
                        tab_id,
                        project_id,
                        is_active,
                        single,
                        cwd.clone(),
                        default_command.clone(),
                        command_running,
                    );
                    this.open_context_menu(ev.position, entries, cx);
                },
                move |_ev: &ClickEvent, this, _window, cx| {
                    this.activate_tab(tab_id, cx);
                },
                ("tab-close", tab_id.0),
                move |_ev: &ClickEvent, this, window, cx| {
                    this.request_close_tab(tab_id, window, cx);
                },
                tab_id,
            ));
        }
    }

    strip.child(
        div()
            .id("new-tab")
            .flex_shrink_0()
            .w(px(28.))
            .h(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(theme::RADIUS_SM))
            .cursor_pointer()
            .bg(theme::raised())
            .border_1()
            .border_color(theme::border())
            .text_color(theme::accent())
            .hover(|s| {
                s.bg(theme::accent())
                    .border_color(theme::accent())
                    .text_color(theme::canvas())
            })
            .tooltip(|_window, cx| {
                cx.new(|_| Tooltip::new(i18n::text("tooltip.new_terminal")))
                    .into()
            })
            .child(
                icons::icon(icons::IconName::Plus, 15.)
                    .text_color(theme::accent())
                    .hover(|s| s.text_color(theme::canvas())),
            )
            .on_click(cx.listener(|this, _ev, _window, cx| {
                this.new_tab(cx);
            })),
    )
}

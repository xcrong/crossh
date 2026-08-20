//! 顶部标签条渲染：远程标签、固定/普通本地会话标签与右键菜单。
//!
//! 会话数据与行为在 `shell`/`tabs`，本模块只做视图组装。固定标签
//! （`LocalSession.pin_id.is_some()`）按持久化记录顺序置顶渲染，普通
//! 标签只渲染当前目录内未固定的会话（详见 spec 20260818-local-tab-pin-rename）。

use std::collections::BTreeMap;
use std::path::PathBuf;

use gpui::{
    AnyElement, AppContext, ClickEvent, Context, ElementId, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, SharedString, StatefulInteractiveElement, Styled,
    Window, div, px,
};

use crate::features::workspace::pinned::pinned_tabs_for_project;
use crate::features::workspace::shell::AppShell;
use crate::features::workspace::status::{conn_state_dot_color, local_tab_dot_color};
use crate::features::workspace::view::{ActiveView, LocalSessionId, Tab};
use crate::shared::i18n;
use crossh_ui::context_menu::{MenuEntry, MenuItem, ShellMenuAction};
use crossh_ui::{icons, theme};
use crossh_ui_component::{Button, ButtonSize, ButtonVariant, TabItem, TabStrip, Tooltip};

// 容器不绑定 click；标签名与关闭按钮分别绑定，避免事件叠加。
#[allow(clippy::too_many_arguments)]
fn render_tab_chip<M, S, C>(
    cx: &mut Context<AppShell>,
    container_id: impl Into<gpui::ElementId>,
    dot_color: gpui::Rgba,
    label: impl Into<SharedString>,
    is_active: bool,
    leading: Option<AnyElement>,
    label_id: impl Into<ElementId>,
    menu: M,
    on_select: S,
    close_id: impl Into<gpui::ElementId>,
    on_close: C,
) -> AnyElement
where
    M: Fn(&MouseDownEvent, &mut AppShell, &mut Window, &mut Context<AppShell>) + 'static,
    S: Fn(&ClickEvent, &mut AppShell, &mut Window, &mut Context<AppShell>) + 'static,
    C: Fn(&ClickEvent, &mut AppShell, &mut Window, &mut Context<AppShell>) + 'static,
{
    let mut tab = TabItem::new(container_id, label);
    tab = tab
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

pub(crate) fn local_session_menu_entries(
    session_id: LocalSessionId,
    is_active: bool,
    pinned: bool,
    single: bool,
    cwd: PathBuf,
    default_command: Option<String>,
    is_command_running: bool,
) -> Vec<MenuEntry<ShellMenuAction>> {
    let mut entries = vec![MenuEntry::Item(MenuItem {
        id: "switch".into(),
        label: i18n::text("context_menu.switch"),
        shortcut_hint: None,
        disabled: is_active,
        danger: false,
        action: ShellMenuAction::SelectLocalSession(session_id),
    })];
    if pinned {
        entries.push(MenuEntry::Item(MenuItem {
            id: "unpin".into(),
            label: i18n::text("context_menu.unpin_tab"),
            shortcut_hint: None,
            disabled: false,
            danger: false,
            action: ShellMenuAction::UnpinLocalSession(session_id),
        }));
        entries.push(MenuEntry::Item(MenuItem {
            id: "rename".into(),
            label: i18n::text("context_menu.rename_tab"),
            shortcut_hint: None,
            disabled: false,
            danger: false,
            action: ShellMenuAction::RenameLocalSession(session_id),
        }));
        entries.push(MenuEntry::Item(MenuItem {
            id: "edit-default-command".into(),
            label: i18n::text("context_menu.edit_default_command"),
            shortcut_hint: None,
            disabled: false,
            danger: false,
            action: ShellMenuAction::EditDefaultCommand(session_id),
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
            action: ShellMenuAction::ReloadDefaultCommand(session_id),
        }));
        entries.push(MenuEntry::Item(MenuItem {
            id: "clear-default-command".into(),
            label: i18n::text("context_menu.clear_default_command"),
            shortcut_hint: None,
            disabled: !has_command,
            danger: false,
            action: ShellMenuAction::ClearDefaultCommand(session_id),
        }));
    } else {
        entries.push(MenuEntry::Item(MenuItem {
            id: "pin".into(),
            label: i18n::text("context_menu.pin_tab"),
            shortcut_hint: None,
            disabled: false,
            danger: false,
            action: ShellMenuAction::PinLocalSession(session_id),
        }));
    }
    entries.extend([
        MenuEntry::Item(MenuItem {
            id: "close".into(),
            label: i18n::text("context_menu.close"),
            shortcut_hint: None,
            disabled: false,
            danger: false,
            action: ShellMenuAction::CloseLocalSession(session_id),
        }),
        MenuEntry::Item(MenuItem {
            id: "close-others".into(),
            label: i18n::text("context_menu.close_others"),
            shortcut_hint: None,
            disabled: single,
            danger: false,
            action: ShellMenuAction::CloseOtherLocalSessions(session_id),
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

    match shell.workspace.active_view {
        Some(ActiveView::RemoteTab(active_idx)) => {
            for idx in 0..shell.workspace.sessions.remote_tabs.len() {
                if shell
                    .workspace
                    .is_split_secondary(ActiveView::RemoteTab(idx))
                {
                    continue;
                }
                let tab = &shell.workspace.sessions.remote_tabs[idx];
                let is_active = active_idx == idx;
                let state = shell.connections.state_for_key(&tab.host_key, cx);
                let alias = tab_label(tab, cx);
                let single = shell.workspace.sessions.remote_tabs.len() == 1;
                let target = tab.target.clone();
                strip = strip.child(render_tab_chip(
                    cx,
                    ("remote-tab-container", idx),
                    conn_state_dot_color(&state),
                    alias,
                    is_active,
                    None,
                    ("remote-tab", idx),
                    move |ev: &MouseDownEvent, this, _window, cx| {
                        let mut entries = vec![MenuEntry::Item(MenuItem {
                            id: "switch".into(),
                            label: i18n::text("context_menu.switch"),
                            shortcut_hint: None,
                            disabled: is_active,
                            danger: false,
                            action: ShellMenuAction::SelectRemoteTab(idx),
                        })];
                        entries.extend([
                            MenuEntry::Item(MenuItem {
                                id: "close".into(),
                                label: i18n::text("context_menu.close"),
                                shortcut_hint: None,
                                disabled: false,
                                danger: false,
                                action: ShellMenuAction::CloseRemoteTab(idx),
                            }),
                            MenuEntry::Item(MenuItem {
                                id: "close-others".into(),
                                label: i18n::text("context_menu.close_others"),
                                shortcut_hint: None,
                                disabled: single,
                                danger: false,
                                action: ShellMenuAction::CloseOtherRemoteTabs(idx),
                            }),
                            MenuEntry::Item(MenuItem {
                                id: "close-all".into(),
                                label: i18n::text("context_menu.close_all"),
                                shortcut_hint: None,
                                disabled: false,
                                danger: false,
                                action: ShellMenuAction::CloseAllRemoteTabs,
                            }),
                            MenuEntry::Separator,
                            MenuEntry::Item(MenuItem {
                                id: "copy-target".into(),
                                label: i18n::text("context_menu.copy_target"),
                                shortcut_hint: None,
                                disabled: false,
                                danger: false,
                                action: ShellMenuAction::CopyText(target.clone()),
                            }),
                        ]);
                        this.open_context_menu(ev.position, entries, cx);
                    },
                    move |_ev: &ClickEvent, this, _window, cx| {
                        this.switch_remote_tab(idx, cx);
                    },
                    ("remote-tab-close", idx),
                    move |_ev: &ClickEvent, this, w, cx| {
                        this.request_close_remote_tab(idx, w, cx);
                    },
                ));
            }
        }
        Some(ActiveView::LocalSession(active_session_id)) => {
            // 固定会话只在所属项目视图内置顶（契约 1 Rev-1）：按当前
            // 活动项目过滤持久化记录再映射到会话；当前目录的普通会话
            // 在其后渲染，已固定的会话不重复出现。
            let pinned_by_id = shell
                .workspace
                .sessions
                .local_sessions
                .iter()
                .filter_map(|(session_id, session)| {
                    session.pin_id.map(|pin_id| (pin_id, *session_id))
                })
                .collect::<BTreeMap<_, _>>();
            let pinned_ids = shell
                .local_dir_for_session(active_session_id)
                .map(|dir| {
                    pinned_tabs_for_project(
                        &shell.workspace_settings.pinned_local_tabs,
                        &dir.project_dir,
                    )
                    .iter()
                    .filter_map(|tab| pinned_by_id.get(&tab.pin_id).copied())
                    .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let session_ids = shell
                .local_dir_for_session(active_session_id)
                .map(|dir| dir.sessions.clone())
                .unwrap_or_default();

            for (idx, session_id) in pinned_ids.iter().copied().enumerate() {
                if shell
                    .workspace
                    .is_split_secondary(ActiveView::LocalSession(session_id))
                {
                    continue;
                }
                let is_active = active_session_id == session_id;
                let Some(session) = shell.workspace.sessions.local_sessions.get(&session_id) else {
                    continue;
                };
                let state = session.terminal.read(cx).state.clone();
                let command_running = session.terminal.read(cx).is_command_running(cx);
                let fallback = format!("ses{}", idx + 1);
                let label = session
                    .custom_name
                    .clone()
                    .unwrap_or_else(|| session.terminal.read(cx).tab_title(&fallback));
                let cwd = session.cwd.clone();
                let default_command = session.default_command.clone();
                let has_default_command = default_command.is_some();
                let single = session_ids.len() == 1;
                let leading = {
                    let mut leading =
                        div().flex().items_center().gap_1().child(
                            icons::icon(icons::IconName::Pin, 11.).text_color(theme::accent()),
                        );
                    if has_default_command {
                        leading = leading.child(
                            icons::icon(icons::IconName::Play, 8.).text_color(theme::muted_text()),
                        );
                    }
                    leading.into_any_element()
                };
                strip = strip.child(render_tab_chip(
                    cx,
                    ("pinned-tab-container", session_id),
                    local_tab_dot_color(&Some(state), command_running),
                    label,
                    is_active,
                    Some(leading),
                    ("pinned-tab", session_id),
                    move |ev: &MouseDownEvent, this, _window, cx| {
                        let entries = local_session_menu_entries(
                            session_id,
                            is_active,
                            true,
                            single,
                            cwd.clone(),
                            default_command.clone(),
                            command_running,
                        );
                        this.open_context_menu(ev.position, entries, cx);
                    },
                    move |_ev: &ClickEvent, this, _window, cx| {
                        this.select_local_session(session_id, cx);
                    },
                    ("pinned-tab-close", session_id),
                    move |_ev: &ClickEvent, this, w, cx| {
                        this.request_close_local_session(session_id, w, cx);
                    },
                ));
            }

            for (idx, session_id) in session_ids.iter().copied().enumerate() {
                if shell
                    .workspace
                    .is_split_secondary(ActiveView::LocalSession(session_id))
                    || shell
                        .workspace
                        .sessions
                        .local_sessions
                        .get(&session_id)
                        .is_some_and(|session| session.pin_id.is_some())
                {
                    continue;
                }
                let is_active = active_session_id == session_id;
                let state = shell
                    .workspace
                    .sessions
                    .local_sessions
                    .get(&session_id)
                    .map(|session| session.terminal.read(cx).state.clone());
                let command_running = shell
                    .workspace
                    .sessions
                    .local_sessions
                    .get(&session_id)
                    .is_some_and(|session| session.terminal.read(cx).is_command_running(cx));
                let fallback = format!("ses{}", idx + 1);
                let label = match shell.workspace.sessions.local_sessions.get(&session_id) {
                    Some(session) => session.terminal.read(cx).tab_title(&fallback),
                    None => fallback,
                };
                let cwd = shell
                    .workspace
                    .sessions
                    .local_sessions
                    .get(&session_id)
                    .map(|session| session.cwd.clone())
                    .unwrap_or_default();
                let single = session_ids.len() == 1;
                strip = strip.child(render_tab_chip(
                    cx,
                    ("local-tab-container", session_id),
                    local_tab_dot_color(&state, command_running),
                    label,
                    is_active,
                    None,
                    ("local-tab", session_id),
                    move |ev: &MouseDownEvent, this, _window, cx| {
                        let entries = local_session_menu_entries(
                            session_id,
                            is_active,
                            false,
                            single,
                            cwd.clone(),
                            None,
                            command_running,
                        );
                        this.open_context_menu(ev.position, entries, cx);
                    },
                    move |_ev: &ClickEvent, this, _window, cx| {
                        this.select_local_session(session_id, cx);
                    },
                    ("local-tab-close", session_id),
                    move |_ev: &ClickEvent, this, w, cx| {
                        this.request_close_local_session(session_id, w, cx);
                    },
                ));
            }
        }
        None => {}
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
            .on_click(cx.listener(|this, _ev, window, cx| {
                this.new_tab(window, cx);
            })),
    )
}

fn tab_label(tab: &Tab, cx: &mut Context<AppShell>) -> String {
    tab.pane.title(cx)
}

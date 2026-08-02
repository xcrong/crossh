//! 工作区：标签条 + 终端/SFTP/转发主区，以及会话/标签的数据类型。

use std::collections::BTreeMap;
use std::path::PathBuf;

use gpui::{
    AnyElement, AppContext, Context, Entity, FontWeight, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, SharedString, StatefulInteractiveElement, Styled,
    div, hsla, px, rgb,
};

use crate::i18n;
use crate::ui::app_shell::AppShell;
use crate::ui::context_menu::{MenuEntry, MenuItem, ShellMenuAction};
use crate::ui::settings::render_settings_page;
use crate::ui::terminal_view::ConnState;
use crate::ui::{ForwardPane, SftpPane, TerminalView, icons, theme};

/// 一个标签内承载的面板。
pub enum Pane {
    Terminal(Entity<TerminalView>),
    Sftp(Entity<SftpPane>),
    Forward(Entity<ForwardPane>),
}

/// 一个远程终端/SFTP 标签。
pub struct Tab {
    /// 重新打开终端时使用的原始目标（别名或 user@host:port）。
    pub target: String,
    pub alias: String,
    pub host_key: String,
    pub pane: Pane,
}

pub type LocalSessionId = u64;

/// 当前主区正在展示的工作区。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveView {
    RemoteTab(usize),
    LocalSession(LocalSessionId),
}

pub struct LocalSession {
    pub cwd: PathBuf,
    pub terminal: Entity<TerminalView>,
}

pub struct LocalDir {
    pub cwd: PathBuf,
    pub sessions: Vec<LocalSessionId>,
    pub active_session: Option<LocalSessionId>,
}

/// 主区：设置页或标签条 + 内容区。
pub fn render_main(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    if shell.settings_open {
        return render_settings_page(shell, cx);
    }

    let mut pane = div()
        .flex_1()
        .min_h_0()
        .h_full()
        .bg(theme::canvas())
        .flex()
        .flex_col();

    // 标签条。
    pane = pane.child(render_tab_strip(shell, cx));

    // 终端/SFTP 区。
    let mut content = div().flex_1().min_h_0().relative();
    let active_pane = match shell.active_view {
        Some(ActiveView::RemoteTab(idx)) => shell.remote_tabs.get(idx).map(|tab| match &tab.pane {
            Pane::Terminal(t) => t.clone().into_any_element(),
            Pane::Sftp(s) => s.clone().into_any_element(),
            Pane::Forward(f) => f.clone().into_any_element(),
        }),
        Some(ActiveView::LocalSession(session_id)) => shell
            .local_sessions
            .get(&session_id)
            .map(|session| session.terminal.clone().into_any_element()),
        None => None,
    };
    if let Some(active_pane) = active_pane {
        content = content.child(active_pane);
    } else {
        content = content.child(render_empty_state(cx));
    }

    if let Some(status) = &shell.status {
        content = content.child(
            div()
                .absolute()
                .bottom_2()
                .left_2()
                .px_3()
                .py_1()
                .rounded(px(theme::RADIUS_SM))
                .border_1()
                .border_color(theme::border_strong())
                .bg(theme::raised())
                .text_xs()
                .text_color(theme::text())
                .child(SharedString::from(status.clone())),
        );
    }
    pane = pane.child(content);
    pane.into_any_element()
}

fn render_empty_state(cx: &mut Context<AppShell>) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(340.))
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .w(px(56.))
                        .h(px(56.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(14.))
                        .bg(theme::accent_soft())
                        .child(
                            icons::icon(icons::IconName::Terminal, 28.).text_color(theme::accent()),
                        ),
                )
                .child(
                    div()
                        .mt_2()
                        .text_lg()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::text())
                        .child(SharedString::from(i18n::text("empty_state.title"))),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme::muted_text())
                        .child(SharedString::from(i18n::text("empty_state.description"))),
                )
                .child(
                    div()
                        .id("empty-new-project")
                        .mt_3()
                        .px_3()
                        .h(px(32.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(theme::RADIUS_SM))
                        .cursor_pointer()
                        .bg(theme::accent())
                        .text_color(theme::canvas())
                        .hover(|style| style.bg(rgb(0x82e3bf)))
                        .child(
                            icons::icon(icons::IconName::FolderOpen, 14.)
                                .text_color(theme::canvas()),
                        )
                        .child(SharedString::from(i18n::text("project.open")))
                        .on_click(cx.listener(|this, _ev, _window, cx| {
                            this.choose_project_directory(cx);
                        })),
                ),
        )
        .into_any_element()
}

fn render_tab_strip(shell: &AppShell, cx: &mut Context<AppShell>) -> impl IntoElement {
    let mut strip = div()
        .flex()
        .flex_row()
        .h(px(theme::TAB_HEIGHT))
        .px_2()
        .gap_1()
        .items_center()
        .bg(theme::surface())
        .border_b_1()
        .border_color(theme::border());

    match shell.active_view {
        Some(ActiveView::RemoteTab(active_idx)) => {
            for idx in 0..shell.remote_tabs.len() {
                let tab = &shell.remote_tabs[idx];
                let is_active = active_idx == idx;
                let state = shell.pool.state_for_key(&tab.host_key, cx);
                let alias = tab_label(tab, cx);
                // 容器不绑定 click；标签名与关闭按钮分别绑定，避免事件叠加。
                let mut container = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h(px(28.))
                    .px_1()
                    .rounded(px(theme::RADIUS_SM));
                if is_active {
                    container = container.bg(theme::accent_soft());
                } else {
                    container = container.hover(|style| style.bg(theme::raised()));
                }
                let container = container
                    .id(("remote-tab-container", idx))
                    .on_mouse_down(MouseButton::Right, {
                        let target = tab.target.clone();
                        let single = shell.remote_tabs.len() == 1;
                        cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                            let entries = vec![
                                MenuEntry::Item(MenuItem {
                                    id: "switch".into(),
                                    label: i18n::text("context_menu.switch"),
                                    shortcut_hint: None,
                                    disabled: is_active,
                                    danger: false,
                                    action: ShellMenuAction::SelectRemoteTab(idx),
                                }),
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
                            ];
                            this.open_context_menu(ev.position, entries, cx);
                        })
                    })
                    .child(
                        div()
                            .id(("remote-tab", idx))
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .py_1()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(theme::text())
                            .hover(|s| s.text_color(theme::accent()))
                            .child(
                                div()
                                    .w(px(6.))
                                    .h(px(6.))
                                    .rounded_full()
                                    .bg(tab_badge_color(&state)),
                            )
                            .child(SharedString::from(alias))
                            .on_click(cx.listener(move |this, _ev, _w, cx| {
                                this.switch_remote_tab(idx, cx);
                            })),
                    )
                    .child(
                        div()
                            .id(("remote-tab-close", idx))
                            .w(px(24.))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(theme::RADIUS_SM))
                            .cursor_pointer()
                            .text_color(theme::muted_text())
                            .hover(|s| s.bg(theme::raised()).text_color(theme::danger()))
                            .tooltip(|_window, cx| {
                                cx.new(|_| crate::ui::widgets::LocalPathTooltip {
                                    path: SharedString::from(i18n::text("tooltip.close_tab")),
                                })
                                .into()
                            })
                            .child(
                                icons::icon(icons::IconName::X, 13.)
                                    .text_color(theme::muted_text())
                                    .hover(|s| s.text_color(theme::danger())),
                            )
                            .on_click(cx.listener(move |this, _ev, _w, cx| {
                                this.close_remote_tab(idx, cx);
                            })),
                    );
                strip = strip.child(container);
            }
        }
        Some(ActiveView::LocalSession(active_session_id)) => {
            let session_ids = shell
                .local_dir_for_session(active_session_id)
                .map(|dir| dir.sessions.clone())
                .unwrap_or_default();
            for (idx, session_id) in session_ids.iter().copied().enumerate() {
                let is_active = active_session_id == session_id;
                let state = shell
                    .local_sessions
                    .get(&session_id)
                    .map(|session| session.terminal.read(cx).state.clone());
                let label = format!("ses{}", idx + 1);
                let mut container = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h(px(28.))
                    .px_1()
                    .rounded(px(theme::RADIUS_SM));
                if is_active {
                    container = container.bg(theme::accent_soft());
                } else {
                    container = container.hover(|style| style.bg(theme::raised()));
                }
                let container = container
                    .id(("local-tab-container", session_id))
                    .on_mouse_down(MouseButton::Right, {
                        let cwd = shell
                            .local_sessions
                            .get(&session_id)
                            .map(|session| session.cwd.clone())
                            .unwrap_or_default();
                        let single = session_ids.len() == 1;
                        cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                            let entries = vec![
                                MenuEntry::Item(MenuItem {
                                    id: "switch".into(),
                                    label: i18n::text("context_menu.switch"),
                                    shortcut_hint: None,
                                    disabled: is_active,
                                    danger: false,
                                    action: ShellMenuAction::SelectLocalSession(session_id),
                                }),
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
                                    action: ShellMenuAction::CopyText(
                                        cwd.to_string_lossy().to_string(),
                                    ),
                                }),
                                MenuEntry::Item(MenuItem {
                                    id: "reveal-finder".into(),
                                    label: i18n::text("context_menu.reveal_in_finder"),
                                    shortcut_hint: None,
                                    disabled: false,
                                    danger: false,
                                    action: ShellMenuAction::RevealInFinder(cwd.clone()),
                                }),
                            ];
                            this.open_context_menu(ev.position, entries, cx);
                        })
                    })
                    .child(
                        div()
                            .id(("local-tab", session_id))
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .py_1()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(theme::text())
                            .hover(|s| s.text_color(theme::accent()))
                            .child(
                                div()
                                    .w(px(6.))
                                    .h(px(6.))
                                    .rounded_full()
                                    .bg(tab_badge_color(&state)),
                            )
                            .child(SharedString::from(label))
                            .on_click(cx.listener(move |this, _ev, _w, cx| {
                                this.select_local_session(session_id, cx);
                            })),
                    )
                    .child(
                        div()
                            .id(("local-tab-close", session_id))
                            .w(px(24.))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(theme::RADIUS_SM))
                            .cursor_pointer()
                            .text_color(theme::muted_text())
                            .hover(|s| s.bg(theme::raised()).text_color(theme::danger()))
                            .tooltip(|_window, cx| {
                                cx.new(|_| crate::ui::widgets::LocalPathTooltip {
                                    path: SharedString::from(i18n::text("tooltip.close_tab")),
                                })
                                .into()
                            })
                            .child(
                                icons::icon(icons::IconName::X, 13.)
                                    .text_color(theme::muted_text())
                                    .hover(|s| s.text_color(theme::danger())),
                            )
                            .on_click(cx.listener(move |this, _ev, _w, cx| {
                                this.close_local_session(session_id, cx);
                            })),
                    );
                strip = strip.child(container);
            }
        }
        None => {}
    }

    strip.child(div().flex_1()).child(
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
            .bg(theme::accent_soft())
            .border_1()
            .border_color(theme::border_strong())
            .text_color(theme::accent())
            .hover(|s| {
                s.bg(theme::accent())
                    .border_color(theme::accent())
                    .text_color(theme::canvas())
            })
            .tooltip(|_window, cx| {
                cx.new(|_| crate::ui::widgets::LocalPathTooltip {
                    path: SharedString::from(i18n::text("tooltip.new_terminal")),
                })
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

fn tab_badge_color(state: &Option<ConnState>) -> gpui::Hsla {
    match state {
        Some(ConnState::Connecting) => hsla(0.13, 0.8, 0.6, 1.),
        Some(ConnState::Connected) => hsla(0.33, 0.7, 0.5, 1.),
        Some(ConnState::Error(_)) => hsla(0., 0.8, 0.55, 1.),
        Some(ConnState::Closed) => hsla(0., 0., 0.35, 1.),
        None => hsla(0., 0., 0.35, 1.),
    }
}

fn tab_label(tab: &Tab, cx: &mut Context<AppShell>) -> String {
    match &tab.pane {
        Pane::Terminal(terminal) => terminal
            .read(cx)
            .title()
            .filter(|title| !title.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| tab.alias.clone()),
        Pane::Sftp(_) => format!("{} ({})", tab.alias, i18n::text("tab.sftp")),
        Pane::Forward(_) => format!("{} ({})", tab.alias, i18n::text("tab.forward")),
    }
}

/// 把会话按 cwd 重建目录视图：同一目录的会话合并，保留上一次的活动会话。
/// `remembered` 是最近打开过的本地目录（无活动会话），合并进来后仍显示在侧栏。
pub fn rebuild_local_dirs(
    previous: &BTreeMap<PathBuf, LocalDir>,
    sessions: impl IntoIterator<Item = (LocalSessionId, PathBuf)>,
    remembered: impl IntoIterator<Item = PathBuf>,
    active_local_session: Option<LocalSessionId>,
) -> BTreeMap<PathBuf, LocalDir> {
    let mut next = BTreeMap::new();
    for cwd in remembered {
        next.entry(cwd.clone()).or_insert_with(|| LocalDir {
            cwd,
            sessions: Vec::new(),
            active_session: None,
        });
    }
    for (session_id, cwd) in sessions {
        next.entry(cwd.clone())
            .or_insert_with(|| LocalDir {
                cwd,
                sessions: Vec::new(),
                active_session: None,
            })
            .sessions
            .push(session_id);
    }

    for (cwd, dir) in &mut next {
        let previous_active = previous.get(cwd).and_then(|old| old.active_session);
        dir.active_session = active_local_session
            .filter(|id| dir.sessions.contains(id))
            .or_else(|| previous_active.filter(|id| dir.sessions.contains(id)))
            .or_else(|| dir.sessions.first().copied());
    }
    next
}

/// 多会话目录取「最活跃」的状态用于侧栏/徽标。
pub fn preferred_state(left: ConnState, right: ConnState) -> ConnState {
    if state_priority(&right) > state_priority(&left) {
        right
    } else {
        left
    }
}

fn state_priority(state: &ConnState) -> u8 {
    match state {
        ConnState::Connected => 4,
        ConnState::Connecting => 3,
        ConnState::Error(_) => 2,
        ConnState::Closed => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_directories_keep_sessions_isolated() {
        let previous = BTreeMap::from([
            (
                PathBuf::from("/Users/me/one"),
                LocalDir {
                    cwd: PathBuf::from("/Users/me/one"),
                    sessions: vec![1, 2],
                    active_session: Some(2),
                },
            ),
            (
                PathBuf::from("/Users/me/two"),
                LocalDir {
                    cwd: PathBuf::from("/Users/me/two"),
                    sessions: vec![3],
                    active_session: Some(3),
                },
            ),
        ]);
        let current = vec![
            (1, PathBuf::from("/Users/me/one")),
            (2, PathBuf::from("/Users/me/two")),
            (3, PathBuf::from("/Users/me/two")),
        ];

        let dirs = rebuild_local_dirs(&previous, current, Vec::new(), Some(2));
        assert_eq!(dirs[&PathBuf::from("/Users/me/one")].sessions, vec![1]);
        assert_eq!(
            dirs[&PathBuf::from("/Users/me/one")].active_session,
            Some(1)
        );
        assert_eq!(dirs[&PathBuf::from("/Users/me/two")].sessions, vec![2, 3]);
        assert_eq!(
            dirs[&PathBuf::from("/Users/me/two")].active_session,
            Some(2)
        );
    }

    #[test]
    fn remembered_dirs_stay_without_live_sessions() {
        let previous = BTreeMap::new();
        let remembered = vec![
            PathBuf::from("/Users/me/one"),
            PathBuf::from("/Users/me/two"),
        ];
        let current = vec![(1, PathBuf::from("/Users/me/one"))];

        let dirs = rebuild_local_dirs(&previous, current, remembered, Some(1));
        assert_eq!(dirs[&PathBuf::from("/Users/me/one")].sessions, vec![1]);
        assert_eq!(
            dirs[&PathBuf::from("/Users/me/two")].sessions,
            Vec::<LocalSessionId>::new()
        );
        assert_eq!(dirs[&PathBuf::from("/Users/me/two")].active_session, None);
    }

    #[test]
    fn project_group_prefers_a_live_session_state() {
        assert_eq!(
            preferred_state(ConnState::Closed, ConnState::Connecting),
            ConnState::Connecting
        );
        assert_eq!(
            preferred_state(ConnState::Connecting, ConnState::Connected),
            ConnState::Connected
        );
        assert_eq!(
            preferred_state(ConnState::Connected, ConnState::Error("failed".into())),
            ConnState::Connected
        );
    }
}

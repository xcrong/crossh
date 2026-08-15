//! 侧栏：主机搜索框、Local/Active/Bank 分组列表、宽度拖拽。

use std::collections::BTreeMap;
use std::path::Path;

use gpui::{
    AnyElement, AppContext, Context, FontWeight, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div,
    px,
};

use crate::features::connections::HostEntry;
use crate::features::terminal::ConnState;
use crate::features::workspace::shell::AppShell;
use crate::features::workspace::status::conn_state_dot_color;
use crate::features::workspace::view::{ActiveView, LocalDir};
use crate::shared::i18n::{self};
use crossh_ui::context_menu::{MenuEntry, MenuItem, ShellMenuAction};
use crossh_ui::widgets::{ime_input_canvas, text_caret};
use crossh_ui::{icons, theme};
use crossh_ui_component::{Avatar, AvatarKind, SplitResizer, StatusDot, Tooltip};

const TRANSPARENT: gpui::Rgba = gpui::Rgba {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.0,
};

fn host_entry_matches(entry: &HostEntry, query: &str) -> bool {
    entry.alias.to_ascii_lowercase().contains(query)
        || entry.detail.to_ascii_lowercase().contains(query)
}

/// 侧栏整体布局：标题栏（含设置）+ 搜索框 + 分组列表 + 宽度拖拽。
pub fn render_sidebar(shell: &AppShell, window: &Window, cx: &mut Context<AppShell>) -> AnyElement {
    let query = shell.host_query.trim().to_ascii_lowercase();
    let search_focus = shell.host_focus.clone();
    let search_value = shell.host_query.clone();
    let search_ime = shell.host_ime_marked_text.clone();
    let input_entity = cx.entity();
    let active_remote_key = match shell.workspace.active_view {
        Some(ActiveView::RemoteTab(idx)) => shell
            .workspace
            .sessions
            .remote_tabs
            .get(idx)
            .map(|tab| tab.host_key.clone()),
        _ => None,
    };
    let mut project_dirs: Vec<&LocalDir> = shell
        .workspace
        .sessions
        .local_dirs
        .values()
        .filter(|dir| local_dir_matches_query(dir, &query))
        .collect();
    // 活跃目录优先，其余按「最近打开」顺序（未被记录的排在最后）。
    project_dirs.sort_by_key(|dir| {
        let recency = shell
            .workspace_settings
            .recent_dirs
            .iter()
            .position(|project_dir| project_dir == &dir.project_dir);
        (!dir.sessions.is_empty(), recency.unwrap_or(usize::MAX))
    });
    let mut project_name_counts = BTreeMap::new();
    for dir in shell.workspace.sessions.local_dirs.values() {
        *project_name_counts
            .entry(local_dir_name_key(&dir.project_dir))
            .or_insert(0usize) += 1;
    }
    let project_query = matches!(
        query.as_str(),
        "local" | "project" | "projects" | "本地" | "项目"
    );
    let show_projects = query.is_empty() || project_query || !project_dirs.is_empty();

    let mut active_entries = Vec::new();
    let mut bank_entries = Vec::new();
    for (idx, entry) in shell.connections.entries().iter().enumerate() {
        if !query.is_empty() && !host_entry_matches(entry, &query) {
            continue;
        }
        let state = shell.connections.state_for_key(&entry.key, cx);
        let row = (idx, entry.clone(), state);
        if is_active_connection(&row.2) {
            active_entries.push(row);
        } else {
            bank_entries.push(row);
        }
    }

    let active_count = active_entries.len();
    let bank_count = bank_entries.len();
    let project_count = if show_projects { project_dirs.len() } else { 0 };
    let mut active_list = div().id("active-host-list").flex().flex_col().gap_1();
    if active_entries.is_empty() {
        active_list = active_list.child(render_host_group_empty(i18n::text(
            "sidebar.no_active_connections",
        )));
    } else {
        for (idx, entry, state) in active_entries {
            let selected = active_remote_key.as_deref() == Some(entry.key.as_str());
            active_list = active_list.child(render_host_entry(idx, &entry, state, selected, cx));
        }
    }

    let mut bank_list = div().id("bank-host-list").flex().flex_col().gap_1();
    if bank_entries.is_empty() {
        bank_list = bank_list.child(render_host_group_empty(i18n::text(
            "sidebar.no_hosts_in_bank",
        )));
    } else {
        for (idx, entry, state) in bank_entries {
            let selected = active_remote_key.as_deref() == Some(entry.key.as_str());
            bank_list = bank_list.child(render_host_entry(idx, &entry, state, selected, cx));
        }
    }

    let mut project_list = div().id("project-list").flex().flex_col().gap_1();
    if project_dirs.is_empty() {
        project_list =
            project_list.child(render_host_group_empty(i18n::text("sidebar.no_projects")));
    } else {
        for (idx, dir) in project_dirs.iter().enumerate() {
            let selected = is_active_local_dir(shell, dir);
            let duplicate_name = project_name_counts
                .get(&local_dir_name_key(&dir.project_dir))
                .is_some_and(|count| *count > 1);
            project_list = project_list.child(render_local_dir(
                idx,
                dir,
                selected,
                duplicate_name,
                shell,
                cx,
            ));
        }
    }

    // Searching should reveal matching hosts even when their group is collapsed.
    let active_collapsed = shell.active_collapsed && query.is_empty();
    let bank_collapsed = shell.bank_collapsed && query.is_empty();
    let active_group = render_host_group(
        HostGroupSpec {
            id: "active",
            icon: icons::IconName::Server,
            title: i18n::text("sidebar.active"),
            count: active_count,
            collapsed: active_collapsed,
            children: active_list.into_any_element(),
            toggle: AppShell::toggle_active_group,
            action: None,
        },
        cx,
    );
    let bank_group = render_host_group(
        HostGroupSpec {
            id: "bank",
            icon: icons::IconName::Server,
            title: i18n::text("sidebar.bank"),
            count: bank_count,
            collapsed: bank_collapsed,
            children: bank_list.into_any_element(),
            toggle: AppShell::toggle_bank_group,
            action: None,
        },
        cx,
    );
    let projects_group = if show_projects {
        Some(render_host_group(
            HostGroupSpec {
                id: "projects",
                icon: icons::IconName::FolderOpen,
                title: i18n::text("sidebar.local"),
                count: project_count,
                collapsed: shell.projects_collapsed && query.is_empty(),
                children: project_list.into_any_element(),
                toggle: AppShell::toggle_projects_group,
                action: Some(AppShell::choose_project_directory),
            },
            cx,
        ))
    } else {
        None
    };

    let mut list = div()
        .id("host-list")
        .track_scroll(&shell.sidebar_scroll)
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap_3()
        .px_3()
        .py_3()
        .overflow_y_scroll();
    if let Some(projects_group) = projects_group {
        list = list.child(projects_group);
    }
    list = list.child(active_group).child(bank_group);

    let search_focused = search_focus.is_focused(window);
    let mut search_content = div()
        .min_w_0()
        .flex_1()
        .flex()
        .items_center()
        .overflow_x_hidden();
    if search_value.is_empty() {
        if search_focused {
            search_content = search_content.child(text_caret(px(16.)));
        }
        if search_ime.is_empty() {
            search_content = search_content.child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .child(SharedString::from(i18n::text("sidebar.search_placeholder"))),
            );
        } else {
            search_content = search_content.child(
                div()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .underline()
                    .text_decoration_color(theme::accent())
                    .child(SharedString::from(search_ime.clone())),
            );
        }
    } else {
        search_content = search_content.child(
            div()
                .min_w_0()
                .flex_shrink_0()
                .whitespace_nowrap()
                .child(SharedString::from(search_value)),
        );
        if search_focused {
            search_content = search_content.child(text_caret(px(16.)));
        }
        if !search_ime.is_empty() {
            search_content = search_content.child(
                div()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .underline()
                    .text_decoration_color(theme::accent())
                    .child(SharedString::from(search_ime)),
            );
        }
    }
    let search = div()
        .id("host-search")
        .mx_2()
        .mb_2()
        .h(px(32.))
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .bg(theme::surface())
        .border_1()
        .border_color(theme::border_strong())
        .rounded(px(theme::RADIUS_SM))
        .relative()
        .text_xs()
        .text_color(if shell.host_query.is_empty() {
            theme::faint_text()
        } else {
            theme::text()
        })
        .track_focus(&search_focus)
        .focus(|style| style.border_color(theme::focus_ring()))
        .on_click({
            let search_focus = search_focus.clone();
            move |_ev, window, cx| window.focus(&search_focus, cx)
        })
        .on_key_down(cx.listener(AppShell::handle_host_search_key))
        .child(icons::icon(icons::IconName::Search, 14.).text_color(theme::muted_text()))
        .child(search_content)
        .child(ime_input_canvas(search_focus, input_entity));

    let width = shell
        .sidebar_width
        .get()
        .clamp(theme::SIDEBAR_MIN_WIDTH, theme::SIDEBAR_MAX_WIDTH);
    let resizer = SplitResizer::new(
        "sidebar-resize",
        shell.sidebar_dragging.clone(),
        shell.sidebar_width.clone(),
    )
    .min_width(theme::SIDEBAR_MIN_WIDTH)
    .max_width(theme::SIDEBAR_MAX_WIDTH)
    .line();

    let titlebar = div()
        .relative()
        .h(px(theme::TITLEBAR_HEIGHT))
        .flex_shrink_0()
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .border_b_1()
        .border_color(theme::border())
        .child(
            div()
                .w(px(24.))
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::RADIUS_SM))
                .bg(theme::accent_soft())
                .border_1()
                .border_color(theme::border_strong())
                .child(icons::logo(20.)),
        )
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::text())
                .child(SharedString::from("crossh")),
        )
        .child(div().flex_1());
    let sidebar_root = div()
        .relative()
        .flex_shrink_0()
        .w(px(width))
        .h_full()
        .flex()
        .flex_col()
        .bg(theme::sidebar())
        .border_r_1()
        .border_color(theme::border())
        .child(
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(titlebar)
                .child(search)
                .child(list),
        )
        .child(resizer);
    sidebar_root.into_any_element()
}

/// 收起主机栏时保留活跃项目与连接主机，便于直接切换工作目标。
pub fn render_sidebar_rail(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let active_remote_key = match shell.workspace.active_view {
        Some(ActiveView::RemoteTab(idx)) => shell
            .workspace
            .sessions
            .remote_tabs
            .get(idx)
            .map(|tab| tab.host_key.clone()),
        _ => None,
    };
    let mut activity = div()
        .id("sidebar-rail-activity")
        .w_full()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .overflow_y_scroll();
    for dir in shell
        .workspace
        .sessions
        .local_dirs
        .values()
        .filter(|dir| !dir.sessions.is_empty())
    {
        let project_dir = dir.project_dir.clone();
        let label = local_dir_name(&project_dir);
        let project_id = project_dir.to_string_lossy().to_string();
        let avatar = Avatar::new(&label).kind(AvatarKind::Project);
        let selected = is_active_local_dir(shell, dir);
        activity = activity.child(
            div()
                .id(SharedString::from(format!(
                    "sidebar-rail-project-{project_id}"
                )))
                .w(px(30.))
                .h(px(30.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::RADIUS_SM))
                .cursor_pointer()
                .border_1()
                .border_color(if selected {
                    theme::accent()
                } else {
                    TRANSPARENT
                })
                .bg(if selected {
                    theme::accent_soft()
                } else {
                    TRANSPARENT
                })
                .hover(move |style| {
                    style.bg(if selected {
                        theme::accent_soft()
                    } else {
                        theme::surface()
                    })
                })
                .tooltip(move |_window, cx| cx.new(|_| Tooltip::new(label.clone())).into())
                .child(avatar)
                .on_click(cx.listener(move |this, _ev, _window, cx| {
                    this.activate_local_dir(project_dir.clone(), cx);
                })),
        );
    }
    for (index, entry) in shell.connections.entries().iter().enumerate() {
        if !is_active_connection(&shell.connections.state_for_key(&entry.key, cx)) {
            continue;
        }
        let alias = entry.alias.clone();
        let avatar = Avatar::new(&alias).kind(AvatarKind::Host);
        let selected = active_remote_key.as_deref() == Some(entry.key.as_str());
        activity = activity.child(
            div()
                .id(("sidebar-rail-host", index))
                .w(px(30.))
                .h(px(30.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::RADIUS_SM))
                .cursor_pointer()
                .border_1()
                .border_color(if selected {
                    theme::accent()
                } else {
                    TRANSPARENT
                })
                .bg(if selected {
                    theme::accent_soft()
                } else {
                    TRANSPARENT
                })
                .hover(move |style| {
                    style.bg(if selected {
                        theme::accent_soft()
                    } else {
                        theme::surface()
                    })
                })
                .tooltip(move |_window, cx| cx.new(|_| Tooltip::new(alias.clone())).into())
                .child(avatar)
                .on_click(cx.listener(move |this, _ev, _window, cx| {
                    this.open_host(index, cx);
                })),
        );
    }

    let add_entries = rail_add_menu_entries(
        shell
            .workspace_settings
            .recent_dirs
            .iter()
            .filter(|path| shell.workspace.sessions.local_dirs.contains_key(*path))
            .map(std::path::PathBuf::as_path),
        shell
            .connections
            .entries()
            .iter()
            .enumerate()
            .map(|(index, entry)| (index, entry.alias.as_str())),
    );

    div()
        .id("sidebar-rail")
        .w(px(theme::SIDEBAR_RAIL_WIDTH))
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .items_center()
        .py_2()
        .bg(theme::sidebar())
        .border_r_1()
        .border_color(theme::border())
        .child(
            div()
                .w(px(30.))
                .h(px(30.))
                .mb_2()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::RADIUS_SM))
                .bg(theme::accent_soft())
                .border_1()
                .border_color(theme::border_strong())
                .child(icons::logo(19.)),
        )
        .child(activity)
        .child(
            div()
                .id("sidebar-rail-add")
                .w(px(30.))
                .h(px(30.))
                .mt_1()
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::RADIUS_SM))
                .cursor_pointer()
                .text_color(theme::muted_text())
                .border_1()
                .border_color(theme::border_strong())
                .hover(|style| style.bg(theme::surface()).text_color(theme::text()))
                .tooltip(|_window, cx| {
                    cx.new(|_| Tooltip::new(i18n::text("tooltip.open_target")))
                        .into()
                })
                .child(
                    icons::icon(icons::IconName::Plus, 15.)
                        .text_color(theme::muted_text())
                        .hover(|style| style.text_color(theme::text())),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                        this.open_context_menu(ev.position, add_entries.clone(), cx);
                    }),
                ),
        )
        .into_any_element()
}

fn rail_add_menu_entries<'a>(
    recent_dirs: impl IntoIterator<Item = &'a Path>,
    hosts: impl IntoIterator<Item = (usize, &'a str)>,
) -> Vec<MenuEntry<ShellMenuAction>> {
    let recent_dirs = recent_dirs.into_iter().collect::<Vec<_>>();
    let hosts = hosts.into_iter().collect::<Vec<_>>();
    let mut entries = vec![MenuEntry::SectionHeader(i18n::text(
        "rail_add.open_project",
    ))];
    entries.push(MenuEntry::Item(MenuItem {
        id: "rail-open-local-project".into(),
        label: i18n::text("rail_add.open_local_project"),
        shortcut_hint: Some("⌘O".into()),
        disabled: false,
        danger: false,
        action: ShellMenuAction::ChooseLocalProject,
    }));
    if !recent_dirs.is_empty() {
        entries.extend([
            MenuEntry::Separator,
            MenuEntry::SectionHeader(i18n::text("empty_state.recent_projects")),
        ]);
    }
    entries.extend(recent_dirs.into_iter().enumerate().map(|(index, path)| {
        MenuEntry::Item(MenuItem {
            id: format!("rail-open-project-{index}"),
            label: local_dir_name(path),
            shortcut_hint: None,
            disabled: false,
            danger: false,
            action: ShellMenuAction::ActivateLocalProject(path.to_path_buf()),
        })
    }));
    if !hosts.is_empty() {
        entries.extend([
            MenuEntry::Separator,
            MenuEntry::SectionHeader(i18n::text("empty_state.saved_hosts")),
        ]);
    }
    entries.extend(hosts.into_iter().map(|(index, alias)| {
        MenuEntry::Item(MenuItem {
            id: format!("rail-open-host-{index}"),
            label: alias.to_owned(),
            shortcut_hint: None,
            disabled: false,
            danger: false,
            action: ShellMenuAction::OpenHost(index),
        })
    }));
    entries
}

fn local_dir_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

fn local_dir_name_key(path: &Path) -> String {
    local_dir_name(path).to_ascii_lowercase()
}

#[cfg(test)]
mod rail_add_menu_tests {
    use super::rail_add_menu_entries;
    use crossh_ui::context_menu::{MenuEntry, ShellMenuAction};
    use std::path::Path;

    #[test]
    fn add_menu_matches_empty_state_resource_groups() {
        let projects = [Path::new("/workspace/alpha"), Path::new("/workspace/beta")];
        let entries = rail_add_menu_entries(projects, [(3, "staging"), (7, "production")]);

        assert!(matches!(
            &entries[4],
            MenuEntry::Item(item) if item.label == "alpha" && matches!(item.action, ShellMenuAction::ActivateLocalProject(_))
        ));
        assert!(matches!(
            &entries[5],
            MenuEntry::Item(item) if item.label == "beta" && matches!(item.action, ShellMenuAction::ActivateLocalProject(_))
        ));
        assert!(matches!(entries[0], MenuEntry::SectionHeader(_)));
        assert!(matches!(
            &entries[1],
            MenuEntry::Item(item) if matches!(item.action, ShellMenuAction::ChooseLocalProject)
        ));
        assert!(matches!(entries[2], MenuEntry::Separator));
        assert!(matches!(entries[3], MenuEntry::SectionHeader(_)));
        assert!(matches!(entries[6], MenuEntry::Separator));
        assert!(matches!(entries[7], MenuEntry::SectionHeader(_)));
        assert!(matches!(
            &entries[8],
            MenuEntry::Item(item) if item.label == "staging" && matches!(item.action, ShellMenuAction::OpenHost(3))
        ));
        assert!(matches!(
            &entries[9],
            MenuEntry::Item(item) if item.label == "production" && matches!(item.action, ShellMenuAction::OpenHost(7))
        ));
    }

    #[test]
    fn add_menu_keeps_all_hosts_for_the_scrollable_popover() {
        let hosts = (0..12)
            .map(|index| (index, format!("host-{index}")))
            .collect::<Vec<_>>();
        let entries = rail_add_menu_entries(
            std::iter::empty(),
            hosts.iter().map(|(index, alias)| (*index, alias.as_str())),
        );

        let host_actions = entries
            .iter()
            .filter(|entry| {
                matches!(entry, MenuEntry::Item(item) if matches!(item.action, ShellMenuAction::OpenHost(_)))
            })
            .count();
        assert_eq!(host_actions, 12);
    }

    #[test]
    fn add_menu_keeps_all_recent_projects_for_the_scrollable_popover() {
        let projects = (0..12)
            .map(|index| std::path::PathBuf::from(format!("/workspace/project-{index}")))
            .collect::<Vec<_>>();
        let entries = rail_add_menu_entries(
            projects.iter().map(std::path::PathBuf::as_path),
            std::iter::empty(),
        );

        let project_actions = entries
            .iter()
            .filter(|entry| {
                matches!(entry, MenuEntry::Item(item) if matches!(item.action, ShellMenuAction::ActivateLocalProject(_)))
            })
            .count();
        assert_eq!(project_actions, 12);
    }

    #[test]
    fn add_menu_has_no_trailing_separator_without_saved_hosts() {
        let entries = rail_add_menu_entries([Path::new("/workspace/alpha")], std::iter::empty());

        assert!(matches!(
            entries.last(),
            Some(MenuEntry::Item(item)) if matches!(item.action, ShellMenuAction::ActivateLocalProject(_))
        ));
    }
}

fn local_dir_label(path: &Path, duplicate_name: bool) -> String {
    let name = local_dir_name(path);
    if !duplicate_name {
        return name;
    }

    path.parent()
        .and_then(Path::file_name)
        .and_then(|parent| parent.to_str())
        .filter(|parent| !parent.is_empty())
        .map(|parent| format!("{name} · {parent}"))
        .unwrap_or(name)
}

fn local_dir_matches_query(dir: &LocalDir, query: &str) -> bool {
    query.is_empty()
        || matches!(query, "local" | "project" | "projects" | "本地" | "项目")
        || dir
            .project_dir
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains(query)
}

fn is_active_local_dir(shell: &AppShell, dir: &LocalDir) -> bool {
    matches!(shell.workspace.active_view, Some(ActiveView::LocalSession(session_id)) if dir.sessions.contains(&session_id))
}

fn local_dir_state(shell: &AppShell, dir: &LocalDir, cx: &Context<AppShell>) -> Option<ConnState> {
    dir.sessions
        .iter()
        .filter_map(|id| shell.workspace.sessions.local_sessions.get(id))
        .map(|session| session.terminal.read(cx).state.clone())
        .reduce(crate::features::workspace::view::preferred_state)
}

fn render_local_dir(
    idx: usize,
    dir: &LocalDir,
    selected: bool,
    duplicate_name: bool,
    shell: &AppShell,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let project_dir = dir.project_dir.clone();
    let project_dir_for_row = project_dir.clone();
    let project_dir_for_new = project_dir.clone();
    let label = local_dir_label(&project_dir, duplicate_name);
    let tooltip_path = SharedString::from(project_dir.to_string_lossy().to_string());
    let count = dir.sessions.len();
    let state = local_dir_state(shell, dir, cx);
    let folder_color = if selected {
        theme::accent()
    } else {
        match state {
            Some(ConnState::Connected) => theme::accent(),
            Some(ConnState::Connecting) => theme::warning(),
            Some(ConnState::Error(_)) => theme::danger(),
            _ => theme::muted_text(),
        }
    };
    let mut row = div()
        .id(("local-group", idx))
        .flex_shrink_0()
        .h(px(theme::ROW_HEIGHT))
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(theme::RADIUS_SM))
        .text_sm()
        .cursor_pointer();
    if selected {
        row = row
            .bg(theme::accent_soft())
            .border_l_2()
            .border_color(theme::accent());
    }
    row = row
        .hover(|s| s.bg(theme::surface()))
        .tooltip(move |_window, cx| {
            let path = tooltip_path.clone();
            cx.new(|_| Tooltip::new(path)).into()
        })
        .on_click(cx.listener(move |this, _ev, _window, cx| {
            this.activate_local_dir(project_dir_for_row.clone(), cx);
        }))
        .on_mouse_down(MouseButton::Right, {
            let cwd_menu = project_dir.clone();
            cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                let mut entries = vec![
                    MenuEntry::Item(MenuItem {
                        id: "open-terminal".into(),
                        label: i18n::text("context_menu.open_local_terminal"),
                        shortcut_hint: None,
                        disabled: false,
                        danger: false,
                        action: ShellMenuAction::OpenLocalTerminal(cwd_menu.clone()),
                    }),
                    MenuEntry::Item(MenuItem {
                        id: "reveal-finder".into(),
                        label: i18n::text("context_menu.reveal_in_finder"),
                        shortcut_hint: None,
                        disabled: false,
                        danger: false,
                        action: ShellMenuAction::RevealInFinder(cwd_menu.clone()),
                    }),
                ];
                if count == 0 {
                    entries.push(MenuEntry::Separator);
                    entries.push(MenuEntry::Item(MenuItem {
                        id: "forget-dir".into(),
                        label: i18n::text("context_menu.forget_dir"),
                        shortcut_hint: None,
                        disabled: false,
                        danger: false,
                        action: ShellMenuAction::ForgetLocalDir(cwd_menu.clone()),
                    }));
                }
                this.open_context_menu(ev.position, entries, cx);
            })
        })
        .child(icons::icon(icons::IconName::FolderOpen, 15.).text_color(folder_color))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_color(theme::text())
                .child(SharedString::from(label)),
        )
        .child(
            div()
                .min_w(px(18.))
                .text_xs()
                .text_color(theme::muted_text())
                .child(SharedString::from(format!("{count}"))),
        );
    // 仅有历史记录（无活动会话）的目录提供「从最近记录移除」按钮。
    if count == 0 {
        row = row.child(
            div()
                .id(("local-forget", idx))
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
                    cx.new(|_| Tooltip::new(i18n::text("tooltip.forget_dir")))
                        .into()
                })
                .child(
                    icons::icon(icons::IconName::X, 14.)
                        .text_color(theme::muted_text())
                        .hover(|s| s.text_color(theme::danger())),
                )
                .on_click(cx.listener(move |this, _ev, _window, cx| {
                    cx.stop_propagation();
                    this.forget_local_dir(project_dir.clone(), cx);
                })),
        );
    }
    row.child(
        div()
            .id(("local-new", idx))
            .w(px(24.))
            .h(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(theme::RADIUS_SM))
            .cursor_pointer()
            .text_color(theme::muted_text())
            .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
            .tooltip(|_window, cx| {
                cx.new(|_| Tooltip::new(i18n::text("tooltip.new_terminal")))
                    .into()
            })
            .child(
                icons::icon(icons::IconName::Plus, 14.)
                    .text_color(theme::muted_text())
                    .hover(|s| s.text_color(theme::text())),
            )
            .on_click(cx.listener(move |this, _ev, _window, cx| {
                cx.stop_propagation();
                this.open_local_session(
                    project_dir_for_new.clone(),
                    project_dir_for_new.clone(),
                    cx,
                );
            })),
    )
    .into_any_element()
}

fn render_host_entry(
    idx: usize,
    entry: &HostEntry,
    state: Option<ConnState>,
    selected: bool,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let alias = entry.alias.clone();
    let detail = entry.detail.clone();
    let badge = state_badge(&state);

    let mut entry_div = div()
        .id(("host-entry", idx))
        .flex_shrink_0()
        .min_h(px(theme::ROW_HEIGHT))
        .px_2()
        .py_1()
        .rounded(px(theme::RADIUS_SM))
        .text_sm()
        .cursor_pointer();
    if selected {
        entry_div = entry_div
            .bg(theme::accent_soft())
            .border_l_2()
            .border_color(theme::accent());
    }
    entry_div = entry_div
        .hover(|s| s.bg(theme::surface()))
        .on_click(cx.listener(move |this, _ev, _window, cx| {
            this.open_host(idx, cx);
        }))
        .on_mouse_down(MouseButton::Right, {
            let detail_menu = detail.clone();
            let alias_menu = alias.clone();
            cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                let entries = vec![
                    MenuEntry::Item(MenuItem {
                        id: "open-terminal".into(),
                        label: i18n::text("context_menu.open_terminal"),
                        shortcut_hint: None,
                        disabled: false,
                        danger: false,
                        action: ShellMenuAction::OpenHost(idx),
                    }),
                    MenuEntry::Item(MenuItem {
                        id: "open-sftp".into(),
                        label: i18n::text("context_menu.open_sftp"),
                        shortcut_hint: None,
                        disabled: false,
                        danger: false,
                        action: ShellMenuAction::OpenSftp(idx),
                    }),
                    MenuEntry::Item(MenuItem {
                        id: "open-forward".into(),
                        label: i18n::text("context_menu.open_forward"),
                        shortcut_hint: None,
                        disabled: false,
                        danger: false,
                        action: ShellMenuAction::OpenForward(idx),
                    }),
                    MenuEntry::Separator,
                    MenuEntry::Item(MenuItem {
                        id: "copy-target".into(),
                        label: i18n::text("context_menu.copy_target"),
                        shortcut_hint: None,
                        disabled: false,
                        danger: false,
                        action: ShellMenuAction::CopyText(detail_menu.clone()),
                    }),
                    MenuEntry::Item(MenuItem {
                        id: "copy-alias".into(),
                        label: i18n::text("context_menu.copy_alias"),
                        shortcut_hint: None,
                        disabled: false,
                        danger: false,
                        action: ShellMenuAction::CopyText(alias_menu.clone()),
                    }),
                ];
                this.open_context_menu(ev.position, entries, cx);
            })
        })
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    icons::icon(icons::IconName::Server, 15.).text_color(if selected {
                        theme::accent()
                    } else {
                        theme::muted_text()
                    }),
                )
                .child(StatusDot::new(conn_state_dot_color(&state)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(theme::text())
                        .child(SharedString::from(alias)),
                )
                .child(
                    div()
                        .id(("sftp-btn", idx))
                        .w(px(24.))
                        .h(px(24.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(theme::RADIUS_SM))
                        .cursor_pointer()
                        .text_color(theme::muted_text())
                        .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
                        .tooltip(|_window, cx| {
                            cx.new(|_| Tooltip::new(i18n::text("tooltip.open_sftp")))
                                .into()
                        })
                        .child(
                            icons::icon(icons::IconName::Folder, 14.)
                                .text_color(theme::muted_text())
                                .hover(|s| s.text_color(theme::text())),
                        )
                        .on_click(cx.listener(move |this, _ev, _w, cx| {
                            cx.stop_propagation();
                            this.open_sftp(idx, cx);
                        })),
                )
                .child(
                    div()
                        .id(("fwd-btn", idx))
                        .w(px(24.))
                        .h(px(24.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(theme::RADIUS_SM))
                        .cursor_pointer()
                        .text_color(theme::muted_text())
                        .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
                        .tooltip(|_window, cx| {
                            cx.new(|_| Tooltip::new(i18n::text("tooltip.port_forwarding")))
                                .into()
                        })
                        .child(
                            icons::icon(icons::IconName::ArrowLeftRight, 14.)
                                .text_color(theme::muted_text())
                                .hover(|s| s.text_color(theme::text())),
                        )
                        .on_click(cx.listener(move |this, _ev, _w, cx| {
                            cx.stop_propagation();
                            this.open_forward(idx, cx);
                        })),
                ),
        )
        .child(
            div()
                .ml(px(23.))
                .text_xs()
                .text_color(match state {
                    Some(ConnState::Connected) => theme::accent(),
                    Some(ConnState::Connecting) => theme::warning(),
                    Some(ConnState::Error(_)) => theme::danger(),
                    _ => theme::faint_text(),
                })
                .child(SharedString::from(format!("{badge}{detail}"))),
        );
    entry_div.into_any_element()
}

/// 侧栏分组的渲染参数。
struct HostGroupSpec {
    id: &'static str,
    icon: icons::IconName,
    title: String,
    count: usize,
    collapsed: bool,
    children: AnyElement,
    toggle: fn(&mut AppShell, &mut Context<AppShell>),
    action: Option<fn(&mut AppShell, &mut Context<AppShell>)>,
}

fn render_host_group(spec: HostGroupSpec, cx: &mut Context<AppShell>) -> AnyElement {
    let HostGroupSpec {
        id,
        icon,
        title,
        count,
        collapsed,
        children,
        toggle,
        action,
    } = spec;
    let caret = if collapsed {
        icons::IconName::ChevronRight
    } else {
        icons::IconName::ChevronDown
    };
    let mut header = div()
        .id(format!("host-group-header-{id}"))
        .h(px(30.))
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .text_xs()
        .text_color(theme::muted_text())
        .hover(|s| s.bg(theme::surface()).text_color(theme::text()))
        .on_click(cx.listener(move |this, _ev, _window, cx| toggle(this, cx)))
        .child(icons::icon(caret, 13.).text_color(theme::faint_text()))
        .child(icons::icon(icon, 13.).text_color(theme::faint_text()))
        .child(
            div()
                .flex_1()
                .font_weight(FontWeight::MEDIUM)
                .child(SharedString::from(title)),
        )
        .child(
            div()
                .min_w(px(20.))
                .h(px(18.))
                .px_1()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(theme::raised())
                .text_xs()
                .text_color(theme::muted_text())
                .child(SharedString::from(count.to_string())),
        );

    if let Some(action) = action {
        header = header.child(
            div()
                .id(format!("host-group-action-{id}"))
                .w(px(24.))
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::RADIUS_SM))
                .cursor_pointer()
                .text_color(theme::muted_text())
                .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
                .tooltip(|_window, cx| {
                    cx.new(|_| Tooltip::new(i18n::text("tooltip.new_project")))
                        .into()
                })
                .child(
                    icons::icon(icons::IconName::Plus, 14.)
                        .text_color(theme::muted_text())
                        .hover(|s| s.text_color(theme::text())),
                )
                .on_click(cx.listener(move |this, _ev, _window, cx| {
                    cx.stop_propagation();
                    action(this, cx);
                })),
        );
    }

    let mut group = div()
        .id(format!("host-group-{id}"))
        .flex()
        .flex_col()
        .flex_shrink_0()
        .child(header);
    if !collapsed {
        group = group.child(children);
    }
    group.into_any_element()
}

fn render_host_group_empty(label: String) -> AnyElement {
    div()
        .px_2()
        .py_2()
        .rounded(px(theme::RADIUS_SM))
        .text_xs()
        .text_color(theme::faint_text())
        .child(SharedString::from(label))
        .into_any_element()
}

fn is_active_connection(state: &Option<ConnState>) -> bool {
    matches!(state, Some(ConnState::Connected))
}

/// 连接状态徽标文字。
fn state_badge(state: &Option<ConnState>) -> String {
    match state {
        None => String::new(),
        Some(ConnState::Connecting) => i18n::text("connection.connecting_with_separator"),
        Some(ConnState::Connected) => i18n::text("connection.connected_with_separator"),
        Some(ConnState::Error(_)) => i18n::text("connection.error_with_separator"),
        Some(ConnState::Closed) => i18n::text("connection.closed_with_separator"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn only_connected_hosts_are_active() {
        assert!(is_active_connection(&Some(ConnState::Connected)));
        assert!(!is_active_connection(&Some(ConnState::Connecting)));
        assert!(!is_active_connection(&Some(ConnState::Closed)));
        assert!(!is_active_connection(&Some(ConnState::Error(
            "failed".to_string()
        ))));
        assert!(!is_active_connection(&None));
    }

    #[test]
    fn project_search_matches_directory_view() {
        let dir = LocalDir {
            project_dir: PathBuf::from("/Users/me/projects/crossh"),
            sessions: vec![1, 2],
            active_session: Some(1),
        };
        assert!(local_dir_matches_query(&dir, ""));
        assert!(local_dir_matches_query(&dir, "local"));
        assert!(local_dir_matches_query(&dir, "project"));
        assert!(local_dir_matches_query(&dir, "projects"));
        assert!(local_dir_matches_query(&dir, "projects/crossh"));
        assert!(!local_dir_matches_query(&dir, "unrelated"));
    }

    #[test]
    fn project_labels_prefer_directory_name_and_disambiguate_duplicates() {
        let path = Path::new("/Users/me/Code/crossh");
        assert_eq!(local_dir_name(path), "crossh");
        assert_eq!(
            local_dir_name_key(Path::new("/Users/me/Code/Crossh")),
            "crossh"
        );
        assert_eq!(local_dir_label(path, false), "crossh");
        assert_eq!(local_dir_label(path, true), "crossh · Code");
        assert_eq!(local_dir_label(Path::new("/"), true), "/");
    }
}

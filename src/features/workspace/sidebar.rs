//! 侧栏：项目搜索、本地目录列表、宽度拖拽。

use std::collections::BTreeMap;
use std::path::Path;

use gpui::{
    AnyElement, AppContext, Context, FontWeight, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div,
    px,
};

use crate::features::terminal::ConnState;
use crate::features::workspace::shell::AppShell;
use crate::features::workspace::state::preferred_state;
use crate::features::workspace::view::{ActiveView, LocalDir};
use crate::shared::i18n::{self};
use crossh_core::terminal::path_display_name;
use crossh_ui::context_menu::ShellMenuAction;
use crossh_ui::{icons, theme};
use crossh_ui_component::context_menu::{MenuEntry, MenuItem};
use crossh_ui_component::{
    Avatar, AvatarKind, Button, ButtonSize, ButtonVariant, Hint, Rail, SidePanel, TextInput,
    Tooltip, rail_avatar, scroll_y,
};

/// 侧栏整体布局：标题栏（含设置）+ 搜索框 + 分组列表 + 宽度拖拽。
pub fn render_sidebar(
    shell: &AppShell,
    _window: &Window,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let query = shell.search_query.trim().to_ascii_lowercase();
    let search_focus = shell.search_focus.clone();
    let search_ime = shell.search_ime_marked_text.clone();
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
        (dir.sessions.is_empty(), recency.unwrap_or(usize::MAX))
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

    let mut project_list = div().id("project-list").flex().flex_col().gap_1();
    if project_dirs.is_empty() {
        project_list = project_list.child(
            Hint::new(i18n::text("sidebar.no_projects"))
                .padding_x(px(8.))
                .padding_y(px(8.))
                .radius(px(theme::RADIUS_SM)),
        );
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

    let mut list = scroll_y(&shell.sidebar_scroll)
        .id("host-list")
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap_3()
        .px_3()
        .py_3();
    if show_projects {
        list = list.child(project_list);
    }

    let search = div()
        .id("host-search-wrap")
        .mx_2()
        .mb_2()
        .flex()
        .items_center()
        .gap_2()
        .cursor_text()
        .on_click({
            let focus = search_focus.clone();
            move |_ev, window, cx| window.focus(&focus, cx)
        })
        .child(icons::icon(icons::IconName::Search, 14.).text_color(theme::muted_text()))
        .child(
            TextInput::new("host-search", search_focus.clone())
                .value(shell.search_query.clone())
                .placeholder(i18n::text("sidebar.search_placeholder"))
                .ime_marked_text(search_ime)
                .text_color(if shell.search_query.is_empty() {
                    theme::faint_text()
                } else {
                    theme::text()
                })
                .bg(theme::surface())
                .flex_1()
                .entity(cx.entity())
                .on_key_down(cx.listener(AppShell::handle_search_key)),
        );

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
    SidePanel::left(
        "sidebar-resize",
        shell.sidebar_width.clone(),
        shell.sidebar_dragging.clone(),
    )
    .min_width(theme::SIDEBAR_MIN_WIDTH)
    .max_width(theme::SIDEBAR_MAX_WIDTH)
    .bg(theme::sidebar())
    .border_color(theme::border())
    .line()
    .child(
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(titlebar)
            .child(search)
            .child(list),
    )
    .into_any_element()
}

/// 收起主机栏时保留活跃项目与连接主机，便于直接切换工作目标。
pub fn render_sidebar_rail(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let mut project_name_counts = BTreeMap::new();
    for dir in shell.workspace.sessions.local_dirs.values() {
        *project_name_counts
            .entry(local_dir_name_key(&dir.project_dir))
            .or_insert(0usize) += 1;
    }
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
        let duplicate = project_name_counts
            .get(&local_dir_name_key(&project_dir))
            .copied()
            .unwrap_or(0)
            > 1;
        let avatar_source = if duplicate {
            local_dir_label(&project_dir, true)
        } else {
            label.clone()
        };
        let project_id = project_dir.to_string_lossy().to_string();
        let tooltip_label = local_dir_label(&project_dir, duplicate);
        let tooltip = SharedString::from(format!("{tooltip_label} — {project_id}"));
        let avatar = Avatar::new(&avatar_source).kind(AvatarKind::Project);
        let selected = is_active_local_dir(shell, dir);
        activity = activity.child(rail_avatar(
            SharedString::from(format!("sidebar-rail-project-{project_id}")),
            avatar,
            tooltip,
            selected,
            cx.listener(move |this, _ev, _window, cx| {
                this.activate_local_dir(project_dir.clone(), cx);
            }),
        ));
    }
    let add_entries = rail_add_menu_entries(
        shell
            .workspace_settings
            .recent_dirs
            .iter()
            .filter(|path| shell.workspace.sessions.local_dirs.contains_key(*path))
            .map(std::path::PathBuf::as_path),
    );

    Rail::left("sidebar-rail", theme::SIDEBAR_RAIL_WIDTH)
        .bg(theme::sidebar())
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
) -> Vec<MenuEntry<ShellMenuAction>> {
    let recent_dirs = recent_dirs.into_iter().collect::<Vec<_>>();
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
    entries
}

fn local_dir_name(path: &Path) -> String {
    path_display_name(path)
}

fn local_dir_name_key(path: &Path) -> String {
    local_dir_name(path).to_ascii_lowercase()
}

#[cfg(test)]
mod rail_add_menu_tests {
    use super::rail_add_menu_entries;
    use crossh_ui::context_menu::ShellMenuAction;
    use crossh_ui_component::context_menu::MenuEntry;
    use std::path::Path;

    #[test]
    fn add_menu_matches_empty_state_resource_groups() {
        let projects = [Path::new("/workspace/alpha"), Path::new("/workspace/beta")];
        let entries = rail_add_menu_entries(projects);

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
        assert_eq!(entries.len(), 6);
    }

    #[test]
    fn add_menu_keeps_all_recent_projects_for_the_scrollable_popover() {
        let projects = (0..12)
            .map(|index| std::path::PathBuf::from(format!("/workspace/project-{index}")))
            .collect::<Vec<_>>();
        let entries = rail_add_menu_entries(projects.iter().map(std::path::PathBuf::as_path));

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
        let entries = rail_add_menu_entries([Path::new("/workspace/alpha")]);

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
        .reduce(preferred_state)
}

pub(crate) fn local_dir_stop_button_visible(count: usize) -> bool {
    count > 0
}

pub(crate) fn local_dir_forget_button_visible(count: usize) -> bool {
    count == 0
}

pub(crate) fn build_local_dir_context_menu_entries(
    project_dir: std::path::PathBuf,
    count: usize,
) -> Vec<MenuEntry<ShellMenuAction>> {
    let mut entries = vec![
        MenuEntry::Item(MenuItem {
            id: "open-terminal".into(),
            label: i18n::text("context_menu.open_local_terminal"),
            shortcut_hint: None,
            disabled: false,
            danger: false,
            action: ShellMenuAction::OpenLocalTerminal(project_dir.clone()),
        }),
        MenuEntry::Item(MenuItem {
            id: "reveal-finder".into(),
            label: i18n::text("context_menu.reveal_in_finder"),
            shortcut_hint: None,
            disabled: false,
            danger: false,
            action: ShellMenuAction::RevealInFinder(project_dir.clone()),
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
            action: ShellMenuAction::ForgetLocalDir(project_dir.clone()),
        }));
    } else {
        entries.push(MenuEntry::Separator);
        entries.push(MenuEntry::Item(MenuItem {
            id: "stop-project".into(),
            label: i18n::text("context_menu.stop_project"),
            shortcut_hint: None,
            disabled: false,
            danger: false,
            action: ShellMenuAction::StopLocalProject(project_dir),
        }));
    }
    entries
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
    let project_dir_for_stop = project_dir.clone();
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
                let entries = build_local_dir_context_menu_entries(cwd_menu.clone(), count);
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
    if local_dir_stop_button_visible(count) {
        row = row.child(
            Button::new(SharedString::from(format!("local-stop-{idx}")))
                .size(ButtonSize::Icon(px(24.)))
                .variant(ButtonVariant::Ghost)
                .icon(icons::icon(icons::IconName::Square, 14.).text_color(theme::muted_text()))
                .tooltip(i18n::text("tooltip.stop_project"))
                .on_click(cx.listener(move |this, _ev, window, cx| {
                    cx.stop_propagation();
                    this.stop_local_project(project_dir_for_stop.clone(), window, cx);
                })),
        );
    }
    if local_dir_forget_button_visible(count) {
        row = row.child(
            Button::new(("local-forget", idx))
                .size(ButtonSize::Icon(px(24.)))
                .variant(ButtonVariant::Ghost)
                .icon(icons::icon(icons::IconName::X, 14.).text_color(theme::muted_text()))
                .tooltip(i18n::text("tooltip.forget_dir"))
                .on_click(cx.listener(move |this, _ev, _window, cx| {
                    cx.stop_propagation();
                    this.forget_local_dir(project_dir.clone(), cx);
                })),
        );
    }
    row.child(
        Button::new(("local-new", idx))
            .size(ButtonSize::Icon(px(24.)))
            .variant(ButtonVariant::Ghost)
            .icon(icons::icon(icons::IconName::Plus, 14.).text_color(theme::muted_text()))
            .tooltip(i18n::text("tooltip.new_terminal"))
            .on_click(cx.listener(move |this, _ev, _window, cx| {
                cx.stop_propagation();
                let _ = this.open_local_session(
                    project_dir_for_new.clone(),
                    project_dir_for_new.clone(),
                    cx,
                );
            })),
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

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

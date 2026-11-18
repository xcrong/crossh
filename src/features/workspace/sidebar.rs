//! 侧栏：项目搜索、项目列表、宽度拖拽与收起 rail。
//!
//! 项目顺序严格取快照 `projects`；活跃项目来自 `snapshot.active_project`。

use std::collections::BTreeMap;
use std::path::Path;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, AppContext, Context, FontWeight, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div,
    px,
};

use crate::features::workspace::projection;
use crate::features::workspace::shell::AppShell;
use crate::features::workspace::state::preferred_state;
use crate::features::workspace::status::conn_state_dot_color;
use crossh_core::i18n::{self};
use crossh_core::terminal::path_display_name;
use crossh_session::workspace::ProjectSnapshot;
use crossh_terminal_view::TerminalItem as _;
use crossh_ui::context_menu::ShellMenuAction;
use crossh_ui::{icons, theme};
use crossh_ui_component::context_menu::{MenuEntry, MenuItem};
use crossh_ui_component::{
    Avatar, AvatarKind, Button, ButtonVariant, Hint, Rail, SidePanel, Tooltip, filter_row,
    filter_text_input, rail_avatar, scroll_y,
};

/// 侧栏整体布局：标题栏（含设置）+ 搜索框 + 分组列表 + 宽度拖拽。
pub fn render_sidebar(
    shell: &AppShell,
    _window: &Window,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let query = shell.search_query.value.trim().to_ascii_lowercase();
    let search_focus = shell.search_focus.clone();
    let projects = shell
        .workspace
        .snapshot
        .projects
        .iter()
        .filter(|project| project_matches_query(project, &query))
        .collect::<Vec<_>>();
    let mut name_counts = BTreeMap::new();
    for project in &shell.workspace.snapshot.projects {
        *name_counts
            .entry(project_display_name(project).to_ascii_lowercase())
            .or_insert(0usize) += 1;
    }

    let mut project_list = div().id("project-list").flex().flex_col().gap_1();
    if projects.is_empty() {
        project_list = project_list.child(
            Hint::new(i18n::text("sidebar.no_projects"))
                .padding_x(px(8.))
                .padding_y(px(8.))
                .radius(px(theme::RADIUS_SM)),
        );
    } else {
        for project in &projects {
            let duplicate = name_counts
                .get(&project_display_name(project).to_ascii_lowercase())
                .is_some_and(|count| *count > 1);
            project_list = project_list.child(render_project_row(project, duplicate, shell, cx));
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
    list = list.child(project_list);

    let footer = div()
        .flex_shrink_0()
        .border_t_1()
        .border_color(theme::border())
        .px_2()
        .py_2()
        .child(
            Button::new("sidebar-open-project")
                .variant(ButtonVariant::Secondary)
                .icon(icons::icon(icons::IconName::FolderOpen, 14.))
                .label(i18n::text("rail_add.open_local_project"))
                .full_width()
                .on_click(cx.listener(|this, _ev, _window, cx| {
                    this.choose_project_directory(cx);
                })),
        );

    let search = filter_row("host-search-wrap").child(
        filter_text_input(
            "host-search",
            search_focus.clone(),
            shell.search_query.value.clone(),
            i18n::text("sidebar.search_placeholder"),
            shell.search_query.ime_marked_text.clone(),
            shell.search_query.selection(),
            shell.search_query.cursor,
        )
        .entity(cx.entity())
        .on_key_down(cx.listener(AppShell::handle_search_key))
        .on_pointer_select(cx.listener(AppShell::handle_search_pointer)),
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
            .child(list)
            .child(footer),
    )
    .into_any_element()
}

/// 收起主机栏时保留活跃项目，便于直接切换工作目标。
pub fn render_sidebar_rail(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let mut name_counts = BTreeMap::new();
    for project in &shell.workspace.snapshot.projects {
        *name_counts
            .entry(project_display_name(project).to_ascii_lowercase())
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
    for project in &shell.workspace.snapshot.projects {
        let root = project.root.clone();
        let label = project_display_name(project);
        let duplicate = name_counts
            .get(&label.to_ascii_lowercase())
            .copied()
            .unwrap_or(0)
            > 1;
        let avatar_source = if duplicate {
            project_label(project, true)
        } else {
            label
        };
        let project_id = project.id;
        let tooltip = SharedString::from(format!(
            "{} — {}",
            project_label(project, duplicate),
            root.display()
        ));
        let avatar = Avatar::new(&avatar_source).kind(AvatarKind::Project);
        let selected = shell.workspace.snapshot.active_project == Some(project_id);
        activity = activity.child(rail_avatar(
            SharedString::from(format!("sidebar-rail-project-{}", project_id.0)),
            avatar,
            tooltip,
            selected,
            cx.listener(move |this, _ev, _window, cx| {
                this.open_project_entry(project_id, cx);
            }),
        ));
    }
    let add_entries = rail_add_menu_entries(&shell.workspace.snapshot.projects);

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

fn rail_add_menu_entries(projects: &[ProjectSnapshot]) -> Vec<MenuEntry<ShellMenuAction>> {
    let mut entries = vec![
        MenuEntry::SectionHeader(i18n::text("rail_add.open_project")),
        MenuEntry::Item(MenuItem {
            id: "rail-open-local-project".into(),
            label: i18n::text("rail_add.open_local_project"),
            shortcut_hint: Some("⌘O".into()),
            disabled: false,
            danger: false,
            action: ShellMenuAction::ChooseLocalProject,
        }),
    ];
    if !projects.is_empty() {
        entries.push(MenuEntry::Separator);
        entries.push(MenuEntry::SectionHeader(i18n::text(
            "empty_state.recent_projects",
        )));
        entries.extend(projects.iter().map(|project| {
            MenuEntry::Item(MenuItem {
                id: format!("rail-open-project-{}", project.id.0),
                label: project_display_name(project),
                shortcut_hint: None,
                disabled: false,
                danger: false,
                action: ShellMenuAction::ActivateProject(project.id.0),
            })
        }));
    }
    entries
}

fn project_display_name(project: &ProjectSnapshot) -> String {
    project
        .name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| path_display_name(&project.root))
}

fn project_label(project: &ProjectSnapshot, duplicate: bool) -> String {
    if !duplicate {
        return project_display_name(project);
    }
    let parent = project
        .root
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty());
    match parent {
        Some(parent) => format!("{parent}/{}", project_display_name(project)),
        None => project_display_name(project),
    }
}

fn project_matches_query(project: &ProjectSnapshot, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let root = project.root.to_string_lossy().to_ascii_lowercase();
    root.contains(query)
        || project_display_name(project)
            .to_ascii_lowercase()
            .contains(query)
}

fn project_dot_color(project: &ProjectSnapshot, shell: &AppShell, cx: &gpui::App) -> gpui::Rgba {
    if !project.available {
        return theme::danger();
    }
    let state = project
        .tabs
        .iter()
        .flat_map(|tab| tab.layout.leaves())
        .filter_map(|pane| shell.workspace.pane_view(pane.id))
        .map(|view| view.state(cx))
        .reduce(preferred_state);
    conn_state_dot_color(&state)
}

fn render_project_row(
    project: &ProjectSnapshot,
    duplicate: bool,
    shell: &AppShell,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let project_id = project.id;
    let selected = shell.workspace.snapshot.active_project == Some(project_id);
    let label = project_label(project, duplicate);
    let detail = project.root.to_string_lossy().into_owned();
    let cwd = project.root.clone();
    let menu_entries = vec![
        MenuEntry::Item(MenuItem {
            id: "activate-project".into(),
            label: i18n::text("context_menu.switch"),
            shortcut_hint: None,
            disabled: selected,
            danger: false,
            action: ShellMenuAction::ActivateProject(project_id.0),
        }),
        MenuEntry::Item(MenuItem {
            id: "new-tab".into(),
            label: i18n::text("context_menu.new_tab"),
            shortcut_hint: None,
            disabled: false,
            danger: false,
            action: ShellMenuAction::CreateTab(project_id.0),
        }),
        MenuEntry::Separator,
        MenuEntry::Item(MenuItem {
            id: "stop-project".into(),
            label: i18n::text("context_menu.stop_project"),
            shortcut_hint: None,
            disabled: project.tabs.is_empty(),
            danger: false,
            action: ShellMenuAction::StopProject(project_id.0),
        }),
        MenuEntry::Item(MenuItem {
            id: "forget-project".into(),
            label: i18n::text("context_menu.forget_project"),
            shortcut_hint: None,
            disabled: false,
            danger: true,
            action: ShellMenuAction::ForgetProject(project_id.0),
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
    ];

    let dot = div()
        .w(px(7.))
        .h(px(7.))
        .flex_shrink_0()
        .rounded_full()
        .bg(project_dot_color(project, shell, cx));
    let pane_count = projection::project_pane_count(project);
    let tab_count = project.tabs.len();

    div()
        .id(("project-row", project_id.0))
        .w_full()
        .min_h(px(44.))
        .px_2()
        .py_1()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .when(selected, |row| row.bg(theme::accent_soft()))
        .hover(|style| style.bg(theme::surface()))
        .tooltip(move |_window, cx| {
            cx.new(|_| Tooltip::new(SharedString::from(detail.clone())))
                .into()
        })
        .child(dot)
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .text_color(theme::text())
                        .child(SharedString::from(label)),
                )
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(theme::faint_text())
                        .child(SharedString::from(format!(
                            "{} tabs · {} panes",
                            tab_count, pane_count
                        ))),
                ),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                this.open_context_menu(ev.position, menu_entries.clone(), cx);
            }),
        )
        .on_click(cx.listener(move |this, _ev, _window, cx| {
            this.open_project_entry(project_id, cx);
        }))
        .into_any_element()
}

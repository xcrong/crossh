//! 侧栏：主机搜索框、Local/Active/Bank 分组列表、语言菜单、宽度拖拽。

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;

use gpui::{
    AnyElement, AppContext, Bounds, Context, FontWeight, InteractiveElement, IntoElement,
    MouseButton, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, SharedString,
    StatefulInteractiveElement, Styled, canvas, div, px,
};

use crate::config::SshConfig;
use crate::i18n::{self, LanguagePreference};
use crate::ssh::ConnectionPool;
use crate::ui::app_shell::AppShell;
use crate::ui::terminal_view::ConnState;
use crate::ui::widgets::LocalPathTooltip;
use crate::ui::workspace::{ActiveView, LocalDir};
use crate::ui::{icons, theme};

/// 主机列表条目：别名 + 详情 + 池键（用于查连接状态）。
#[derive(Clone)]
pub struct HostEntry {
    pub alias: String,
    pub detail: String,
    pub key: String,
}

/// 构建主机列表条目，过滤掉纯通配的默认块（如 `Host *`），并解析出池键。
pub fn build_entries(config: &SshConfig) -> Vec<HostEntry> {
    let mut out = Vec::new();
    for h in config.hosts() {
        let alias = h.alias().to_string();
        if alias == "*" || alias.starts_with('!') {
            continue;
        }
        // resolve 以合并默认块（User/Port 等），得到准确池键与详情。
        let resolved = config.resolve(&alias);
        let detail = format!(
            "{}@{}:{}",
            resolved.user.as_deref().unwrap_or(""),
            resolved.effective_host(),
            resolved.effective_port()
        );
        let key = ConnectionPool::key_for(&resolved);
        out.push(HostEntry { alias, detail, key });
    }
    out
}

fn host_entry_matches(entry: &HostEntry, query: &str) -> bool {
    entry.alias.to_ascii_lowercase().contains(query)
        || entry.detail.to_ascii_lowercase().contains(query)
}

/// 侧栏整体布局：标题栏（含语言切换/设置）+ 搜索框 + 分组列表 + 宽度拖拽。
pub fn render_sidebar(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let query = shell.host_query.trim().to_ascii_lowercase();
    let search_focus = shell.host_focus.clone();
    let search_value = shell.host_query.clone();
    let active_remote_key = match shell.active_view {
        Some(ActiveView::RemoteTab(idx)) => {
            shell.remote_tabs.get(idx).map(|tab| tab.host_key.clone())
        }
        _ => None,
    };
    let project_dirs: Vec<&LocalDir> = shell
        .local_dirs
        .values()
        .filter(|dir| local_dir_matches_query(dir, &query))
        .collect();
    let mut project_name_counts = BTreeMap::new();
    for dir in shell.local_dirs.values() {
        *project_name_counts
            .entry(local_dir_name_key(&dir.cwd))
            .or_insert(0usize) += 1;
    }
    let project_query = matches!(
        query.as_str(),
        "local" | "project" | "projects" | "本地" | "项目"
    );
    let show_projects = query.is_empty() || project_query || !project_dirs.is_empty();

    let mut active_entries = Vec::new();
    let mut bank_entries = Vec::new();
    for (idx, entry) in shell.entries.iter().enumerate() {
        if !query.is_empty() && !host_entry_matches(entry, &query) {
            continue;
        }
        let state = shell.pool.state_for_key(&entry.key, cx);
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
    let visible_count = active_count + bank_count + project_count;

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
                .get(&local_dir_name_key(&dir.cwd))
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
        .gap_2()
        .px_2()
        .py_2()
        .overflow_y_scroll();
    if let Some(projects_group) = projects_group {
        list = list.child(projects_group);
    }
    list = list.child(active_group).child(bank_group);

    let search_placeholder = if search_value.is_empty() {
        i18n::text("sidebar.search_placeholder")
    } else {
        search_value
    };
    let search = div()
        .id("host-search")
        .mx_2()
        .mb_2()
        .h(px(32.))
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .bg(theme::canvas())
        .border_1()
        .border_color(theme::border())
        .rounded(px(theme::RADIUS_SM))
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
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .child(SharedString::from(search_placeholder)),
        );

    let list_footer = if visible_count == 0 && shell.entries.is_empty() && !show_projects {
        i18n::text("sidebar.no_ssh_hosts")
    } else if visible_count == 0 {
        i18n::text("sidebar.no_matches")
    } else {
        rust_i18n::t!("sidebar.entry_count", count = visible_count).to_string()
    };

    let width = shell
        .sidebar_width
        .get()
        .clamp(theme::SIDEBAR_MIN_WIDTH, theme::SIDEBAR_MAX_WIDTH);
    let container: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
    let backing = canvas(
        {
            let container = container.clone();
            move |bounds, _window, _cx| container.set(Some(bounds))
        },
        {
            let container = container.clone();
            let width_cell = shell.sidebar_width.clone();
            let dragging = shell.sidebar_dragging.clone();
            move |_bounds, _state, window, _cx| {
                window.on_mouse_event({
                    let container = container.clone();
                    let width_cell = width_cell.clone();
                    let dragging = dragging.clone();
                    move |ev: &MouseMoveEvent, _phase, window, _cx| {
                        if !dragging.get() {
                            return;
                        }
                        let Some(bounds) = container.get() else {
                            return;
                        };
                        let width = (ev.position.x - bounds.origin.x)
                            .as_f32()
                            .clamp(theme::SIDEBAR_MIN_WIDTH, theme::SIDEBAR_MAX_WIDTH);
                        width_cell.set(width);
                        window.refresh();
                    }
                });
                window.on_mouse_event({
                    let dragging = dragging.clone();
                    move |_ev: &MouseUpEvent, _phase, window, _cx| {
                        if dragging.replace(false) {
                            window.refresh();
                        }
                    }
                });
            }
        },
    )
    .absolute()
    .size_full();

    let resizing = shell.sidebar_dragging.get();
    let resize_handle = div()
        .id("sidebar-resize")
        .absolute()
        .top_0()
        .right(px(-4.))
        .w(px(8.))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_col_resize()
        .child(
            div()
                .w(px(1.))
                .h_full()
                .bg(if resizing {
                    theme::accent()
                } else {
                    theme::border()
                })
                .hover(|style| style.bg(theme::accent())),
        )
        .on_mouse_down(MouseButton::Left, {
            let dragging = shell.sidebar_dragging.clone();
            move |_ev, window, _cx| {
                dragging.set(true);
                window.refresh();
            }
        });

    let mut titlebar = div()
        .relative()
        .h(px(theme::TITLEBAR_HEIGHT))
        .flex_shrink_0()
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .child(icons::icon(icons::IconName::Terminal, 15.).text_color(theme::accent()))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::text())
                .child(SharedString::from("crossh")),
        )
        .child(div().flex_1())
        .child(
            div()
                .id("language-toggle")
                .h(px(24.))
                .px_2()
                .flex()
                .items_center()
                .gap_1()
                .rounded(px(theme::RADIUS_SM))
                .cursor_pointer()
                .text_xs()
                .text_color(theme::muted_text())
                .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
                .child(SharedString::from(i18n::language_short_label(
                    shell.language_preference.resolve(),
                )))
                .child(icons::icon(icons::IconName::ChevronDown, 11.).text_color(theme::muted_text()).hover(|s| s.text_color(theme::text())))
                .on_click(cx.listener(|this, _ev, _window, cx| {
                    this.toggle_language_menu(cx);
                })),
        )
        .child(
            div()
                .id("settings-toggle")
                .w(px(24.))
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::RADIUS_SM))
                .cursor_pointer()
                .text_color(if shell.settings_open {
                    theme::accent()
                } else {
                    theme::muted_text()
                })
                .bg(if shell.settings_open {
                    theme::accent_soft()
                } else {
                    theme::sidebar()
                })
                .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
                .tooltip(|_window, cx| {
                    cx.new(|_| LocalPathTooltip {
                        path: SharedString::from(i18n::text("tooltip.settings")),
                    })
                    .into()
                })
                .child(icons::icon(icons::IconName::Settings, 14.).text_color(if shell.settings_open {
                    theme::accent()
                } else {
                    theme::muted_text()
                }).hover(|s| s.text_color(theme::text())))
                .on_click(cx.listener(|this, _ev, _window, cx| {
                    this.toggle_settings(cx);
                })),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme::faint_text())
                .child(SharedString::from(format!("{visible_count}"))),
        );
    if shell.language_menu_open {
        titlebar = titlebar.child(render_language_menu(shell, cx));
    }

    div()
        .relative()
        .flex_shrink_0()
        .w(px(width))
        .h_full()
        .flex()
        .flex_col()
        .bg(theme::sidebar())
        .border_r_1()
        .border_color(theme::border())
        .child(backing)
        .child(
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(titlebar)
                .child(search)
                .child(list)
                .child(
                    div()
                        .flex_shrink_0()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(theme::border())
                        .text_xs()
                        .text_color(theme::faint_text())
                        .child(SharedString::from(list_footer)),
                ),
        )
        .child(resize_handle)
        .into_any_element()
}

fn render_language_menu(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let current = shell.language_preference;
    let mut menu = div()
        .id("language-menu")
        .absolute()
        .top(px(theme::TITLEBAR_HEIGHT - 2.))
        .right(px(8.))
        .w(px(168.))
        .p_1()
        .flex()
        .flex_col()
        .gap_1()
        .bg(theme::raised())
        .border_1()
        .border_color(theme::border_strong())
        .rounded(px(theme::RADIUS_SM))
        .shadow_md();

    for preference in LanguagePreference::ALL {
        let selected = preference == current;
        let option_id = format!("language-option-{:?}", preference);
        let option = div()
            .id(option_id)
            .h(px(28.))
            .px_2()
            .flex()
            .items_center()
            .rounded(px(theme::RADIUS_SM))
            .cursor_pointer()
            .text_xs()
            .text_color(if selected {
                theme::accent()
            } else {
                theme::text()
            })
            .bg(if selected {
                theme::accent_soft()
            } else {
                theme::raised()
            })
            .hover(|s| s.bg(theme::surface()).text_color(theme::text()))
            .child(SharedString::from(i18n::preference_label(preference)))
            .on_click(cx.listener(move |this, _ev, _window, cx| {
                this.set_language(preference, cx);
            }));
        menu = menu.child(option);
    }
    menu.into_any_element()
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
            .cwd
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains(query)
}

fn is_active_local_dir(shell: &AppShell, dir: &LocalDir) -> bool {
    matches!(shell.active_view, Some(ActiveView::LocalSession(session_id)) if dir.sessions.contains(&session_id))
}

fn local_dir_state(shell: &AppShell, dir: &LocalDir, cx: &Context<AppShell>) -> Option<ConnState> {
    dir.sessions
        .iter()
        .filter_map(|id| shell.local_sessions.get(id))
        .map(|session| session.terminal.read(cx).state.clone())
        .reduce(crate::ui::workspace::preferred_state)
}

fn render_local_dir(
    idx: usize,
    dir: &LocalDir,
    selected: bool,
    duplicate_name: bool,
    shell: &AppShell,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let cwd = dir.cwd.clone();
    let cwd_for_new = cwd.clone();
    let label = local_dir_label(&cwd, duplicate_name);
    let tooltip_path = SharedString::from(cwd.to_string_lossy().to_string());
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
    row.hover(|s| s.bg(theme::surface()))
        .tooltip(move |_window, cx| {
            let path = tooltip_path.clone();
            cx.new(|_| LocalPathTooltip { path }).into()
        })
        .on_click(cx.listener(move |this, _ev, _window, cx| {
            this.activate_local_dir(cwd.clone(), cx);
        }))
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
        )
        .child(
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
                    cx.new(|_| LocalPathTooltip {
                        path: SharedString::from(i18n::text("tooltip.new_terminal")),
                    })
                    .into()
                })
                .child(icons::icon(icons::IconName::Plus, 14.).text_color(theme::muted_text()).hover(|s| s.text_color(theme::text())))
                .on_click(cx.listener(move |this, _ev, _window, cx| {
                    cx.stop_propagation();
                    this.open_local_session(cwd_for_new.clone(), cx);
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
                            cx.new(|_| LocalPathTooltip {
                                path: SharedString::from(i18n::text("tooltip.open_sftp")),
                            })
                            .into()
                        })
                        .child(icons::icon(icons::IconName::Folder, 14.).text_color(theme::muted_text()).hover(|s| s.text_color(theme::text())))
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
                            cx.new(|_| LocalPathTooltip {
                                path: SharedString::from(i18n::text("tooltip.port_forwarding")),
                            })
                            .into()
                        })
                        .child(icons::icon(icons::IconName::ArrowLeftRight, 14.).text_color(theme::muted_text()).hover(|s| s.text_color(theme::text())))
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
        .h(px(28.))
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
        .child(
            div()
                .flex_1()
                .font_weight(FontWeight::MEDIUM)
                .child(SharedString::from(title)),
        )
        .child(
            div()
                .text_color(theme::faint_text())
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
                    cx.new(|_| LocalPathTooltip {
                        path: SharedString::from(i18n::text("tooltip.new_project")),
                    })
                    .into()
                })
                .child(icons::icon(icons::IconName::Plus, 14.).text_color(theme::muted_text()).hover(|s| s.text_color(theme::text())))
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
            cwd: PathBuf::from("/Users/me/projects/crossh"),
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

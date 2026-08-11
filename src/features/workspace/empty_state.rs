//! 空工作区的快速启动入口。

use std::path::PathBuf;

use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px,
};

use crate::features::connections::HostEntry;
use crate::features::workspace::shell::AppShell;
use crate::shared::i18n;
use crossh_ui::{icons, theme};

const CONTINUE_ENTRY_LIMIT: usize = 8;
const COMPACT_LAYOUT_BREAKPOINT: f32 = 520.;

#[derive(Clone, Debug, PartialEq, Eq)]
enum ContinueEntry {
    Local(PathBuf),
    Host {
        index: usize,
        alias: String,
        detail: String,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum EmptyStateFilter {
    #[default]
    Local,
    Hosts,
}

fn filtered_continue_entries(
    filter: EmptyStateFilter,
    recent_dirs: &[PathBuf],
    hosts: &[HostEntry],
) -> Vec<ContinueEntry> {
    match filter {
        EmptyStateFilter::Local => recent_dirs
            .iter()
            .take(CONTINUE_ENTRY_LIMIT)
            .cloned()
            .map(ContinueEntry::Local)
            .collect(),
        EmptyStateFilter::Hosts => hosts
            .iter()
            .take(CONTINUE_ENTRY_LIMIT)
            .enumerate()
            .map(|(index, host)| ContinueEntry::Host {
                index,
                alias: host.alias.clone(),
                detail: host.detail.clone(),
            })
            .collect(),
    }
}

fn uses_compact_layout(available_width: gpui::Pixels) -> bool {
    available_width.as_f32() < COMPACT_LAYOUT_BREAKPOINT
}

fn top_padding(compact: bool) -> gpui::Pixels {
    px(if compact { 16. } else { 32. })
}

pub(crate) fn render(
    shell: &AppShell,
    available_width: gpui::Pixels,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let compact = uses_compact_layout(available_width);
    let recent_dirs = shell
        .workspace_settings
        .recent_dirs
        .iter()
        .filter(|path| shell.workspace.sessions.local_dirs.contains_key(*path))
        .cloned()
        .collect::<Vec<_>>();
    let entries = filtered_continue_entries(
        shell.empty_state_filter,
        &recent_dirs,
        shell.connections.entries(),
    );

    let mut filters = div()
        .id("empty-resource-filters")
        .h(px(40.))
        .p_1()
        .flex()
        .gap_1()
        .rounded(px(theme::RADIUS_SM))
        .border_1()
        .border_color(theme::border_strong())
        .bg(theme::surface())
        .child(render_filter_button(
            EmptyStateFilter::Local,
            icons::IconName::FolderOpen,
            "empty_state.local_projects",
            shell.empty_state_filter == EmptyStateFilter::Local,
            cx,
        ))
        .child(render_filter_button(
            EmptyStateFilter::Hosts,
            icons::IconName::Server,
            "empty_state.remote_hosts",
            shell.empty_state_filter == EmptyStateFilter::Hosts,
            cx,
        ));
    filters = if compact {
        filters.w_full().flex_none()
    } else {
        filters.flex_1()
    };
    let mut open_project = div()
        .id("empty-open-folder")
        .h(px(40.))
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .bg(theme::accent())
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme::canvas())
        .hover(|style| style.bg(theme::accent_hover()))
        .child(icons::icon(icons::IconName::FolderOpen, 15.).text_color(theme::canvas()))
        .child(SharedString::from(i18n::text("project.open_folder")))
        .on_click(cx.listener(|this, _ev, _window, cx| {
            this.choose_project_directory(cx);
        }));
    if compact {
        open_project = open_project.w_full();
    } else {
        open_project = open_project.flex_none();
    }
    let actions = if compact {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(filters)
            .child(open_project)
    } else {
        div()
            .w_full()
            .flex()
            .gap_2()
            .child(filters)
            .child(open_project)
    };

    let mut panel = div()
        .w_full()
        .max_w(px(560.))
        .flex()
        .flex_col()
        .items_start();
    panel = if compact { panel.px_4() } else { panel.px_6() };
    panel = panel
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::text())
                .child(SharedString::from(i18n::text("empty_state.title"))),
        )
        .child(
            div()
                .mt_1()
                .mb_5()
                .text_sm()
                .text_color(theme::muted_text())
                .child(SharedString::from(i18n::text("empty_state.description"))),
        )
        .child(actions);

    let list_title = match shell.empty_state_filter {
        EmptyStateFilter::Local => "empty_state.recent_projects",
        EmptyStateFilter::Hosts => "empty_state.saved_hosts",
    };
    let empty_text = match shell.empty_state_filter {
        EmptyStateFilter::Local => "sidebar.no_projects",
        EmptyStateFilter::Hosts => "sidebar.no_ssh_hosts",
    };
    let entries_empty = entries.is_empty();
    let mut list = div().w_full().flex().flex_col().gap_1().child(
        div()
            .mb_1()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme::muted_text())
            .child(SharedString::from(i18n::text(list_title))),
    );
    list = if compact { list.mt_4() } else { list.mt_6() };
    if entries_empty {
        list = list.child(
            div()
                .h(px(42.))
                .px_2()
                .flex()
                .items_center()
                .text_sm()
                .text_color(theme::faint_text())
                .child(SharedString::from(i18n::text(empty_text))),
        );
    } else {
        for (row_index, entry) in entries.into_iter().enumerate() {
            list = list.child(render_continue_entry(row_index, entry, cx));
        }
    }
    panel = panel.child(list);

    let root = div()
        .id("empty-state")
        .size_full()
        .min_h_0()
        .flex()
        .flex_col()
        .items_center()
        .pt(top_padding(compact))
        .pb_4()
        .overflow_y_scroll();
    root.child(panel).into_any_element()
}

fn render_filter_button(
    filter: EmptyStateFilter,
    icon: icons::IconName,
    label_key: &'static str,
    selected: bool,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let id = match filter {
        EmptyStateFilter::Local => "empty-filter-local",
        EmptyStateFilter::Hosts => "empty-filter-hosts",
    };
    let mut button = div()
        .id(id)
        .h_full()
        .flex_1()
        .px_2()
        .flex()
        .items_center()
        .justify_center()
        .gap_2()
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .text_sm()
        .child(icons::icon(icon, 14.).text_color(if selected {
            theme::accent()
        } else {
            theme::muted_text()
        }))
        .child(SharedString::from(i18n::text(label_key)));
    button = if selected {
        button
            .bg(theme::raised())
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme::text())
    } else {
        button
            .text_color(theme::muted_text())
            .hover(|style| style.bg(theme::raised()).text_color(theme::text()))
    };
    button
        .on_click(cx.listener(move |this, _ev, _window, cx| {
            this.set_empty_state_filter(filter, cx);
        }))
        .into_any_element()
}

fn render_continue_entry(
    row_index: usize,
    entry: ContinueEntry,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let (icon, label, detail) = match &entry {
        ContinueEntry::Local(path) => (
            icons::IconName::FolderOpen,
            path.file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| path.to_string_lossy().into_owned()),
            path.to_string_lossy().to_string(),
        ),
        ContinueEntry::Host { alias, detail, .. } => {
            (icons::IconName::Server, alias.clone(), detail.clone())
        }
    };

    div()
        .id(("empty-continue-entry", row_index))
        .w_full()
        .h(px(42.))
        .px_2()
        .flex()
        .items_center()
        .gap_3()
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .hover(|style| style.bg(theme::surface()))
        .child(
            div()
                .w(px(28.))
                .h(px(28.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::RADIUS_SM))
                .bg(theme::accent_soft())
                .child(icons::icon(icon, 14.).text_color(theme::accent())),
        )
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
                        .child(SharedString::from(detail)),
                ),
        )
        .child(icons::icon(icons::IconName::ChevronRight, 14.).text_color(theme::faint_text()))
        .on_click(cx.listener(move |this, _ev, _window, cx| match &entry {
            ContinueEntry::Local(path) => this.activate_local_dir(path.clone(), cx),
            ContinueEntry::Host { index, .. } => this.open_host(*index, cx),
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(alias: &str) -> HostEntry {
        HostEntry {
            alias: alias.into(),
            detail: format!("user@{alias}:22"),
            key: alias.into(),
        }
    }

    #[test]
    fn local_filter_keeps_recency_and_applies_the_limit() {
        let recent_dirs = (0..10)
            .map(|index| PathBuf::from(format!("/project-{index}")))
            .collect::<Vec<_>>();
        let hosts = (0..10)
            .map(|index| host(&format!("host-{index}")))
            .collect::<Vec<_>>();

        let entries = filtered_continue_entries(EmptyStateFilter::Local, &recent_dirs, &hosts);

        assert_eq!(entries.len(), CONTINUE_ENTRY_LIMIT);
        assert_eq!(
            entries[0],
            ContinueEntry::Local(PathBuf::from("/project-0"))
        );
        assert_eq!(
            entries[7],
            ContinueEntry::Local(PathBuf::from("/project-7"))
        );
    }

    #[test]
    fn host_filter_keeps_configuration_order_and_applies_the_limit() {
        let recent_dirs = vec![PathBuf::from("/project")];
        let hosts = (0..10)
            .map(|index| host(&format!("host-{index}")))
            .collect::<Vec<_>>();

        let entries = filtered_continue_entries(EmptyStateFilter::Hosts, &recent_dirs, &hosts);

        assert_eq!(entries.len(), CONTINUE_ENTRY_LIMIT);
        assert_eq!(
            entries.last(),
            Some(&ContinueEntry::Host {
                index: 7,
                alias: "host-7".into(),
                detail: "user@host-7:22".into(),
            })
        );
    }

    #[test]
    fn empty_state_layout_switches_at_available_width() {
        assert!(uses_compact_layout(px(COMPACT_LAYOUT_BREAKPOINT - 1.)));
        assert!(!uses_compact_layout(px(COMPACT_LAYOUT_BREAKPOINT)));
        assert_eq!(top_padding(true), px(16.));
        assert_eq!(top_padding(false), px(32.));
    }

    #[test]
    fn filters_select_exactly_one_resource_kind() {
        let recent_dirs = vec![PathBuf::from("/project")];
        let hosts = vec![host("server")];

        assert_eq!(
            filtered_continue_entries(EmptyStateFilter::Local, &recent_dirs, &hosts),
            vec![ContinueEntry::Local(PathBuf::from("/project"))]
        );
        assert_eq!(
            filtered_continue_entries(EmptyStateFilter::Hosts, &recent_dirs, &hosts),
            vec![ContinueEntry::Host {
                index: 0,
                alias: "server".into(),
                detail: "user@server:22".into(),
            }]
        );
    }
}

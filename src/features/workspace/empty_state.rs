//! 空工作区的快速启动入口。

use std::path::PathBuf;

use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px,
};

use crate::features::workspace::shell::AppShell;
use crate::shared::i18n;
use crossh_ui::{icons, theme};
use crossh_ui_component::{Button, ButtonSize, ButtonVariant};

const CONTINUE_ENTRY_LIMIT: usize = 8;
const COMPACT_LAYOUT_BREAKPOINT: f32 = 520.;

#[derive(Clone, Debug, PartialEq, Eq)]
enum ContinueEntry {
    Local(PathBuf),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum EmptyStateFilter {
    #[default]
    Local,
}

fn filtered_continue_entries(recent_dirs: &[PathBuf]) -> Vec<ContinueEntry> {
    recent_dirs
        .iter()
        .take(CONTINUE_ENTRY_LIMIT)
        .cloned()
        .map(ContinueEntry::Local)
        .collect()
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
    let _ = shell.empty_state_filter;
    let compact = uses_compact_layout(available_width);
    let recent_dirs = shell
        .workspace_settings
        .recent_dirs
        .iter()
        .filter(|path| shell.workspace.sessions.local_dirs.contains_key(*path))
        .cloned()
        .collect::<Vec<_>>();
    let entries = filtered_continue_entries(&recent_dirs);

    let mut open_project = Button::new("empty-open-folder")
        .size(ButtonSize::Large)
        .variant(ButtonVariant::Primary)
        .icon(icons::icon(icons::IconName::FolderOpen, 15.).text_color(theme::canvas()))
        .label(i18n::text("project.open_folder"))
        .on_click(cx.listener(|this, _ev, _window, cx| {
            this.choose_project_directory(cx);
        }));
    if compact {
        open_project = open_project.full_width();
    }
    let actions = div().w_full().flex().child(open_project);

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

    let list_title = "empty_state.recent_projects";
    let empty_text = "sidebar.no_projects";
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

fn render_continue_entry(
    row_index: usize,
    entry: ContinueEntry,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let ContinueEntry::Local(path) = &entry;
    let label = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let detail = path.to_string_lossy().to_string();
    let icon = icons::IconName::FolderOpen;

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
        .on_click(cx.listener(move |this, _ev, _window, cx| {
            let ContinueEntry::Local(path) = &entry;
            this.activate_local_dir(path.clone(), cx);
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_filter_keeps_recency_and_applies_the_limit() {
        let recent_dirs = (0..10)
            .map(|index| PathBuf::from(format!("/project-{index}")))
            .collect::<Vec<_>>();

        let entries = filtered_continue_entries(&recent_dirs);

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
    fn empty_state_layout_switches_at_available_width() {
        assert!(uses_compact_layout(px(COMPACT_LAYOUT_BREAKPOINT - 1.)));
        assert!(!uses_compact_layout(px(COMPACT_LAYOUT_BREAKPOINT)));
        assert_eq!(top_padding(true), px(16.));
        assert_eq!(top_padding(false), px(32.));
    }
}

//! 空工作区的快速启动入口：打开项目与继续最近项目。

use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px,
};

use crate::features::workspace::shell::AppShell;
use crossh_core::i18n;
use crossh_core::terminal::path_display_name;
use crossh_session::workspace::ProjectSnapshot;
use crossh_ui::{icons, theme};
use crossh_ui_component::{Button, ButtonSize, ButtonVariant};

const CONTINUE_ENTRY_LIMIT: usize = 8;
const COMPACT_LAYOUT_BREAKPOINT: f32 = 520.;

fn visible_entry_count(total: usize) -> usize {
    total.min(CONTINUE_ENTRY_LIMIT)
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
    let visible = visible_entry_count(shell.workspace.snapshot.projects.len());
    let projects = shell
        .workspace
        .snapshot
        .projects
        .iter()
        .take(visible)
        .collect::<Vec<_>>();

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

    let mut list = div().w_full().flex().flex_col().gap_1().child(
        div()
            .mb_1()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme::muted_text())
            .child(SharedString::from(i18n::text(
                "empty_state.recent_projects",
            ))),
    );
    list = if compact { list.mt_4() } else { list.mt_6() };
    if projects.is_empty() {
        list = list.child(
            div()
                .h(px(42.))
                .px_2()
                .flex()
                .items_center()
                .text_sm()
                .text_color(theme::faint_text())
                .child(SharedString::from(i18n::text("sidebar.no_projects"))),
        );
    } else {
        for (row_index, project) in projects.into_iter().enumerate() {
            list = list.child(render_continue_entry(row_index, project, cx));
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
    project: &ProjectSnapshot,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let label = project
        .name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| path_display_name(&project.root));
    let detail = project.root.to_string_lossy().into_owned();
    let project_id = project.id;
    let icon = icons::IconName::FolderOpen;
    let available = project.available;

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
                .child(icons::icon(icon, 14.).text_color(if available {
                    theme::accent()
                } else {
                    theme::danger()
                })),
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
            this.open_project_entry(project_id, cx);
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continue_entries_are_capped() {
        assert_eq!(visible_entry_count(0), 0);
        assert_eq!(visible_entry_count(3), 3);
        assert_eq!(visible_entry_count(10), CONTINUE_ENTRY_LIMIT);
    }

    #[test]
    fn empty_state_layout_switches_at_available_width() {
        assert!(uses_compact_layout(px(COMPACT_LAYOUT_BREAKPOINT - 1.)));
        assert!(!uses_compact_layout(px(COMPACT_LAYOUT_BREAKPOINT)));
        assert_eq!(top_padding(true), px(16.));
        assert_eq!(top_padding(false), px(32.));
    }
}

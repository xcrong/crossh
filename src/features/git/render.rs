//! Git 窗口布局与视觉渲染。

use std::path::Path;

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement,
    ListHorizontalSizingBehavior, ListSizingBehavior, MouseButton, MouseDownEvent, ParentElement,
    Pixels, Point, Render, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, px, uniform_list,
};

use crate::shared::i18n;
use crossh_core::git::{ChangeStatus, DiffLine, DiffLineKind, FileChange};
use crossh_core::git_conflict::ConflictResolution;
use crossh_core::git_history::{CommitDetail, CommitFileChange, CommitSummary};
use crossh_ui::context_menu::render_context_menu;
use crossh_ui::widgets::{ime_input_canvas, marked_text_span, text_caret, text_span};
use crossh_ui::{icons, theme};
use crossh_ui_component::{
    Badge, BadgeTone, Button, ButtonSize, ButtonVariant, Hint, SplitResizer, scroll_y,
};

use super::editor::CommitEditor;
use super::history::{HistoryDetailState, HistoryListState};
use super::model::{
    CHANGES_PANE_MAX_WIDTH, CHANGES_PANE_MIN_WIDTH, CompactPage, clamp_changes_pane_width,
    uses_compact_git_layout,
};
use super::session::{ChangeKey, DiffState, OperationState};
use super::window::GitWindow;
use super::{
    BackToChanges, CommitChanges, GIT_CHANGES_CONTEXT, GIT_HISTORY_CONTEXT, GIT_WINDOW_CONTEXT,
    MoveHistoryDown, MoveHistoryUp, MoveSelectionDown, MoveSelectionUp, RefreshChanges,
    SelectAllChanges, ToggleSelectedStage,
};

#[derive(Clone)]
enum ChangeListItem {
    Section {
        staged: bool,
        count: usize,
        paths: Vec<String>,
        collapsed: bool,
    },
    Entry(FileChange),
}

fn change_list_items(
    changes: &[FileChange],
    staged_collapsed: bool,
    working_collapsed: bool,
) -> Vec<ChangeListItem> {
    let staged = changes
        .iter()
        .filter(|change| change.staged)
        .cloned()
        .collect::<Vec<_>>();
    let working = changes
        .iter()
        .filter(|change| !change.staged)
        .cloned()
        .collect::<Vec<_>>();
    let mut items = vec![ChangeListItem::Section {
        staged: true,
        count: staged.len(),
        paths: staged.iter().map(|change| change.path.clone()).collect(),
        collapsed: staged_collapsed,
    }];
    if !staged_collapsed {
        items.extend(staged.into_iter().map(ChangeListItem::Entry));
    }
    items.push(ChangeListItem::Section {
        staged: false,
        count: working.len(),
        paths: working.iter().map(|change| change.path.clone()).collect(),
        collapsed: working_collapsed,
    });
    if !working_collapsed {
        items.extend(working.into_iter().map(ChangeListItem::Entry));
    }
    items
}

impl Render for GitWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.compact_layout = uses_compact_git_layout(window.viewport_size().width.into());
        let body = if self.is_history_page() {
            if self.compact_layout {
                match self.compact_page {
                    CompactPage::History => self.render_history_list(true, cx),
                    CompactPage::HistoryDetail => self.render_history_detail(true, cx),
                    CompactPage::Changes
                    | CompactPage::Diff
                    | CompactPage::Branches
                    | CompactPage::Stashes => {
                        unreachable!()
                    }
                }
            } else {
                self.render_history_body(cx)
            }
        } else if self.is_branch_page() {
            self.render_branch_list(self.compact_layout, cx)
        } else if self.is_stash_page() {
            self.render_stash_list(self.compact_layout, cx)
        } else if self.compact_layout {
            match self.compact_page {
                CompactPage::Changes => self.render_changes_pane(true, window, cx),
                CompactPage::Diff => self.render_diff_pane(true, window, cx),
                CompactPage::History
                | CompactPage::HistoryDetail
                | CompactPage::Branches
                | CompactPage::Stashes => {
                    unreachable!()
                }
            }
        } else {
            self.render_standard_body(window, cx)
        };

        let mut root = div()
            .id("git-window")
            .key_context(GIT_WINDOW_CONTEXT)
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .on_action(cx.listener(|this, _: &CommitChanges, _window, cx| {
                this.commit_changes(cx);
            }))
            .on_action(cx.listener(|this, _: &RefreshChanges, _window, cx| {
                this.refresh_current_page(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &BackToChanges, _window, cx| {
                this.back_to_changes(cx);
            }))
            .child(self.render_header(cx))
            .child(self.render_page_tabs(cx))
            .child(body);
        if let Some(menu) = self.context_menu.clone() {
            root = root.child(render_context_menu(
                &menu,
                Point::new(px(0.), px(0.)),
                window,
                cx,
                |this, action, window, cx| this.dispatch_menu_action(action, window, cx),
                |this, cx| this.close_context_menu(cx),
            ));
        }
        root
    }
}

impl GitWindow {
    fn render_page_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let history = self.is_history_page();
        let branches = self.is_branch_page();
        let stashes = self.is_stash_page();
        div()
            .id("git-page-tabs")
            .h(px(34.))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .bg(theme::sidebar())
            .border_b_1()
            .border_color(theme::border())
            .child(
                Button::new("git-page-changes")
                    .size(ButtonSize::Small)
                    .variant(if history || branches || stashes {
                        ButtonVariant::Ghost
                    } else {
                        ButtonVariant::Secondary
                    })
                    .label(i18n::text("git.changes_tab"))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.show_changes(cx);
                    })),
            )
            .child(
                Button::new("git-page-history")
                    .size(ButtonSize::Small)
                    .variant(if history {
                        ButtonVariant::Secondary
                    } else {
                        ButtonVariant::Ghost
                    })
                    .label(i18n::text("git.history_tab"))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.show_history(cx);
                    })),
            )
            .child(
                Button::new("git-page-branches")
                    .size(ButtonSize::Small)
                    .variant(if branches {
                        ButtonVariant::Secondary
                    } else {
                        ButtonVariant::Ghost
                    })
                    .label(i18n::text("git.branches_tab"))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.show_branches(cx);
                    })),
            )
            .child(
                Button::new("git-page-stashes")
                    .size(ButtonSize::Small)
                    .variant(if stashes {
                        ButtonVariant::Secondary
                    } else {
                        ButtonVariant::Ghost
                    })
                    .label(i18n::text("git.stashes_tab"))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.show_stashes(cx);
                    })),
            )
            .into_any_element()
    }

    fn render_standard_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let width = clamp_changes_pane_width(self.changes_pane_width.get());
        let resizer = SplitResizer::new(
            "git-changes-resize",
            self.changes_pane_dragging.clone(),
            self.changes_pane_width.clone(),
        )
        .min_width(CHANGES_PANE_MIN_WIDTH)
        .max_width(CHANGES_PANE_MAX_WIDTH);

        div()
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .child(
                div()
                    .relative()
                    .w(px(width))
                    .flex_shrink_0()
                    .border_r_1()
                    .border_color(theme::border_strong())
                    .child(self.render_changes_pane(false, window, cx))
                    .child(resizer),
            )
            .child(self.render_diff_pane(false, window, cx))
            .into_any_element()
    }

    fn render_history_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let width = clamp_changes_pane_width(self.changes_pane_width.get());
        let resizer = SplitResizer::new(
            "git-history-resize",
            self.changes_pane_dragging.clone(),
            self.changes_pane_width.clone(),
        )
        .min_width(CHANGES_PANE_MIN_WIDTH)
        .max_width(CHANGES_PANE_MAX_WIDTH);
        let list = self.render_history_list(false, cx);
        let detail = self.render_history_detail(false, cx);

        div()
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .child(
                div()
                    .relative()
                    .w(px(width))
                    .flex_shrink_0()
                    .border_r_1()
                    .border_color(theme::border_strong())
                    .child(list)
                    .child(resizer),
            )
            .child(detail)
            .into_any_element()
    }

    fn render_history_list(&mut self, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let focus = self.history_focus.clone();
        let body = match &self.session.history.list_state {
            HistoryListState::Idle | HistoryListState::Loading => {
                Hint::new(i18n::text("git.loading"))
                    .padding_x(px(8.))
                    .padding_y(px(16.))
                    .into_any_element()
            }
            HistoryListState::Error(error) => Hint::new(error.clone())
                .padding_x(px(8.))
                .padding_y(px(16.))
                .into_any_element(),
            HistoryListState::Ready if self.session.history.entries.is_empty() => {
                Hint::new(i18n::text("git.no_history"))
                    .padding_x(px(8.))
                    .padding_y(px(16.))
                    .into_any_element()
            }
            HistoryListState::Ready => {
                let entries = self.session.history.entries.clone();
                let count = entries.len();
                uniform_list(
                    if compact {
                        "git-history-list-compact"
                    } else {
                        "git-history-list"
                    },
                    count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                        entries[range]
                            .iter()
                            .map(|entry| this.render_history_row(entry, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.history_scroll)
                .flex_1()
                .min_h_0()
                .with_sizing_behavior(ListSizingBehavior::Auto)
                .into_any_element()
            }
        };

        div()
            .id(if compact {
                "git-history-compact"
            } else {
                "git-history-pane"
            })
            .key_context(GIT_HISTORY_CONTEXT)
            .track_focus(&focus)
            .tab_stop(true)
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::sidebar())
            .focus(|style| style.border_color(theme::focus_ring()))
            .on_click(move |_event, window, cx| window.focus(&focus, cx))
            .on_action(cx.listener(|this, _: &MoveHistoryUp, _window, cx| {
                this.move_history_selection(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &MoveHistoryDown, _window, cx| {
                this.move_history_selection(1, cx);
            }))
            .child(body)
            .into_any_element()
    }

    fn render_history_row(&self, entry: &CommitSummary, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.session.history.selected.as_deref() == Some(entry.id.as_str());
        let id = entry.id.clone();
        div()
            .id(SharedString::from(format!(
                "git-history-entry-{}",
                entry.id
            )))
            .h(px(66.))
            .w_full()
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_1()
            .cursor_pointer()
            .border_l_2()
            .border_color(if selected {
                theme::accent()
            } else {
                theme::sidebar()
            })
            .bg(if selected {
                theme::raised()
            } else {
                theme::sidebar()
            })
            .hover(|style| style.bg(theme::raised()))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::accent())
                            .child(SharedString::from(entry.short_id.clone())),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::text())
                            .child(SharedString::from(entry.subject.clone())),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(theme::faint_text())
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .child(SharedString::from(entry.author.clone())),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(theme::muted_text())
                            .child(SharedString::from(entry.date.clone())),
                    ),
            )
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.select_history_commit(id.clone(), cx);
            }))
            .into_any_element()
    }

    fn render_history_detail(&self, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let selected_subject = self
            .session
            .history
            .selected
            .as_ref()
            .and_then(|id| {
                self.session
                    .history
                    .entries
                    .iter()
                    .find(|entry| &entry.id == id)
            })
            .map(|entry| entry.subject.clone())
            .unwrap_or_else(|| i18n::text("git.commit_detail"));
        let header = div()
            .h(px(42.))
            .flex_shrink_0()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .when(compact, |header| {
                header.child(
                    Button::new("git-back-to-history")
                        .size(ButtonSize::Icon(px(28.)))
                        .variant(ButtonVariant::Ghost)
                        .tooltip(i18n::text("git.back_to_history"))
                        .icon(
                            icons::icon(icons::IconName::ArrowLeft, 14.)
                                .text_color(theme::muted_text()),
                        )
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.back_to_changes(cx);
                        })),
                )
            })
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::text())
                    .child(SharedString::from(selected_subject)),
            );
        let body = match &self.session.history.detail {
            HistoryDetailState::Idle => Hint::new(i18n::text("git.no_selection"))
                .centered()
                .into_any_element(),
            HistoryDetailState::Loading(_) => Hint::new(i18n::text("git.loading"))
                .centered()
                .into_any_element(),
            HistoryDetailState::Error(error) => {
                Hint::new(error.clone()).centered().into_any_element()
            }
            HistoryDetailState::Ready(detail) => self.render_commit_detail(detail),
        };

        div()
            .id(if compact {
                "git-history-detail-compact"
            } else {
                "git-history-detail"
            })
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn render_commit_detail(&self, detail: &CommitDetail) -> AnyElement {
        let files = detail.files.clone();
        let file_count = files.len();
        let file_list = if files.is_empty() {
            Hint::new(i18n::text("git.no_files_changed"))
                .padding_x(px(12.))
                .padding_y(px(12.))
                .into_any_element()
        } else {
            uniform_list(
                "git-commit-files",
                file_count,
                move |range: std::ops::Range<usize>, _window, _cx| {
                    files[range]
                        .iter()
                        .map(render_commit_file_row)
                        .collect::<Vec<_>>()
                },
            )
            .track_scroll(&self.history_detail_scroll)
            .flex_1()
            .min_h_0()
            .with_sizing_behavior(ListSizingBehavior::Auto)
            .into_any_element()
        };
        let body = detail.body.trim();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                scroll_y(&self.history_message_scroll)
                    .id("git-commit-message")
                    .max_h(px(150.))
                    .flex_shrink_0()
                    .overflow_y_scroll()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(theme::text())
                    .child(SharedString::from(if body.is_empty() {
                        detail.summary.subject.clone()
                    } else {
                        body.to_string()
                    })),
            )
            .child(
                div()
                    .h(px(34.))
                    .flex_shrink_0()
                    .px_3()
                    .flex()
                    .items_center()
                    .border_y_1()
                    .border_color(theme::border())
                    .text_xs()
                    .text_color(theme::muted_text())
                    .child(SharedString::from(
                        rust_i18n::t!("git.files_changed", count = file_count).to_string(),
                    )),
            )
            .child(file_list)
            .into_any_element()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let staged_count = self.staged_count();
        let working_count = self.session.changes.len().saturating_sub(staged_count);
        let refresh_loading = if self.is_history_page() {
            self.session.history.list_state.is_loading()
        } else if self.is_branch_page() {
            self.session.branch.list_state.is_loading()
        } else if self.is_stash_page() {
            self.session.stash.list_state.is_loading()
        } else {
            self.session.refresh.in_flight()
        };
        let mut branch = div()
            .id("git-current-branch")
            .min_w_0()
            .flex()
            .items_center()
            .gap_2()
            .child(icons::icon(icons::IconName::GitBranch, 15.).text_color(theme::accent()))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::text())
                    .child(SharedString::from(
                        self.session
                            .status
                            .as_ref()
                            .map(|status| status.branch.clone())
                            .unwrap_or_else(|| self.session.label.clone()),
                    )),
            )
            .cursor_pointer()
            .hover(|style| style.bg(theme::raised()))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.show_branches(cx);
                cx.stop_propagation();
            }));
        if let Some(status) = &self.session.status {
            if status.ahead > 0 {
                branch = branch.child(status_badge(format!("↑{}", status.ahead)));
            }
            if status.behind > 0 {
                branch = branch.child(status_badge(format!("↓{}", status.behind)));
            }
        }

        div()
            .h(px(46.))
            .flex_shrink_0()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .child(branch)
            .child(div().flex_1())
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(theme::muted_text())
                    .child(SharedString::from(format!(
                        "{} {} · {} {}",
                        staged_count,
                        i18n::text("git.staged"),
                        working_count,
                        i18n::text("git.changes")
                    ))),
            )
            .child(
                Button::new("git-pull")
                    .size(ButtonSize::Icon(px(30.)))
                    .variant(ButtonVariant::Ghost)
                    .loading(matches!(self.session.operation, OperationState::Running))
                    .tooltip(i18n::text("git.pull"))
                    .icon(
                        icons::icon(icons::IconName::Download, 14.).text_color(theme::muted_text()),
                    )
                    .disabled(!self.can_pull())
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.pull_changes(cx);
                    })),
            )
            .child(
                Button::new("git-push")
                    .size(ButtonSize::Icon(px(30.)))
                    .variant(ButtonVariant::Ghost)
                    .loading(matches!(self.session.operation, OperationState::Running))
                    .tooltip(i18n::text("git.push"))
                    .icon(icons::icon(icons::IconName::Upload, 14.).text_color(theme::muted_text()))
                    .disabled(!self.can_push())
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.push_changes(cx);
                    })),
            )
            .child(
                Button::new("git-refresh")
                    .size(ButtonSize::Icon(px(30.)))
                    .variant(ButtonVariant::Ghost)
                    .loading(refresh_loading)
                    .tooltip(i18n::text("git.refresh"))
                    .icon(
                        icons::icon(icons::IconName::RefreshCw, 14.)
                            .text_color(theme::muted_text()),
                    )
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.refresh_current_page(cx);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn render_changes_pane(
        &mut self,
        compact: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focus = self.changes_focus.clone();
        let changes_body = if self.session.initial_loading {
            Hint::new(i18n::text("git.loading"))
                .padding_x(px(8.))
                .padding_y(px(16.))
                .into_any_element()
        } else if let Some(error) = &self.session.load_error {
            Hint::new(error.clone())
                .padding_x(px(8.))
                .padding_y(px(16.))
                .into_any_element()
        } else if self.session.changes.is_empty() {
            Hint::new(i18n::text("git.no_changes"))
                .padding_x(px(8.))
                .padding_y(px(16.))
                .into_any_element()
        } else {
            let items = change_list_items(
                &self.session.changes,
                self.staged_collapsed,
                self.working_collapsed,
            );
            let item_count = items.len();
            uniform_list(
                if compact {
                    "git-changes-list-compact"
                } else {
                    "git-changes-list"
                },
                item_count,
                cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                    items[range]
                        .iter()
                        .map(|item| match item {
                            ChangeListItem::Section {
                                staged,
                                count,
                                paths,
                                collapsed,
                            } => this.render_section_header(
                                *staged,
                                *count,
                                paths.clone(),
                                *collapsed,
                                cx,
                            ),
                            ChangeListItem::Entry(entry) => this.render_change_row(entry, cx),
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.changes_scroll)
            .flex_1()
            .min_h_0()
            .with_sizing_behavior(ListSizingBehavior::Auto)
            .into_any_element()
        };

        div()
            .id(if compact {
                "git-changes-compact"
            } else {
                "git-changes-pane"
            })
            .key_context(GIT_CHANGES_CONTEXT)
            .track_focus(&focus)
            .tab_stop(true)
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::sidebar())
            .focus(|style| style.border_color(theme::focus_ring()))
            .on_click(move |_event, window, cx| window.focus(&focus, cx))
            .on_action(cx.listener(|this, _: &MoveSelectionUp, _window, cx| {
                this.move_selection(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &MoveSelectionDown, _window, cx| {
                this.move_selection(1, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSelectedStage, _window, cx| {
                this.toggle_selected_stage(cx);
            }))
            .on_action(cx.listener(|this, _: &SelectAllChanges, _window, cx| {
                this.select_all_changes(cx);
            }))
            .child(self.render_commit_panel(compact, window, cx))
            .child(self.render_selection_toolbar(cx))
            .when_some(
                self.pending_discard.as_ref().map(Vec::len),
                |pane, count| pane.child(self.render_discard_confirmation(count, cx)),
            )
            .child(changes_body)
            .into_any_element()
    }

    fn render_section_header(
        &self,
        staged: bool,
        count: usize,
        paths: Vec<String>,
        collapsed: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = if staged {
            i18n::text("git.staged_changes")
        } else {
            i18n::text("git.changes")
        };
        div()
            .id(if staged {
                "git-section-staged"
            } else {
                "git-section-working"
            })
            .h(px(34.))
            .w_full()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(if staged {
                theme::accent()
            } else {
                theme::text()
            })
            .hover(|style| style.bg(theme::raised()))
            .child(icons::icon(
                if collapsed {
                    icons::IconName::ChevronRight
                } else {
                    icons::IconName::ChevronDown
                },
                13.,
            ))
            .child(SharedString::from(label))
            .child(
                div()
                    .text_color(theme::muted_text())
                    .child(SharedString::from(count.to_string())),
            )
            .child(div().flex_1())
            .child(
                Button::new(if staged {
                    "git-unstage-all"
                } else {
                    "git-stage-all"
                })
                .size(ButtonSize::Icon(px(24.)))
                .variant(ButtonVariant::Ghost)
                .disabled(paths.is_empty())
                .tooltip(if staged {
                    i18n::text("git.unstage_all")
                } else {
                    i18n::text("git.stage_all")
                })
                .icon(
                    icons::icon(
                        if staged {
                            icons::IconName::Minus
                        } else {
                            icons::IconName::Plus
                        },
                        13.,
                    )
                    .text_color(theme::muted_text()),
                )
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    if staged {
                        this.unstage_paths(paths.clone(), cx);
                    } else {
                        this.stage_paths(paths.clone(), cx);
                    }
                    cx.stop_propagation();
                })),
            )
            .on_click(cx.listener(move |this, _event, _window, cx| {
                if staged {
                    this.staged_collapsed = !this.staged_collapsed;
                } else {
                    this.working_collapsed = !this.working_collapsed;
                }
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_change_row(&self, entry: &FileChange, cx: &mut Context<Self>) -> AnyElement {
        let key = ChangeKey::from(entry);
        let selected = self.session.is_selected(&key);
        let staged = entry.staged;
        let path = Path::new(&entry.path);
        let basename = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| entry.path.clone());
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(|parent| parent.to_string_lossy().into_owned());

        div()
            .id(SharedString::from(format!(
                "git-entry-{}-{}",
                if staged { "staged" } else { "working" },
                entry.path
            )))
            .h(px(34.))
            .w_full()
            .pr_2()
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .border_l_2()
            .border_color(if selected {
                status_color(entry.status)
            } else {
                theme::sidebar()
            })
            .bg(if selected {
                theme::raised()
            } else {
                theme::sidebar()
            })
            .hover(|style| style.bg(theme::raised()))
            .child(status_glyph(entry.status))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .items_baseline()
                    .gap_1()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(theme::text())
                            .child(SharedString::from(basename)),
                    )
                    .when_some(parent, |row, parent| {
                        row.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(theme::faint_text())
                                .child(SharedString::from(parent)),
                        )
                    }),
            )
            .when(entry.insertions > 0 || entry.deletions > 0, |row| {
                row.child(
                    div()
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(theme::muted_text())
                        .child(SharedString::from(format!(
                            "+{} −{}",
                            entry.insertions, entry.deletions
                        ))),
                )
            })
            .child(
                Button::new(SharedString::from(format!(
                    "git-toggle-stage-{}-{}",
                    if staged { "staged" } else { "working" },
                    entry.path
                )))
                .size(ButtonSize::Icon(px(24.)))
                .variant(ButtonVariant::Ghost)
                .tooltip(if staged {
                    i18n::text("git.unstage")
                } else {
                    i18n::text("git.stage")
                })
                .icon(
                    icons::icon(
                        if staged {
                            icons::IconName::Minus
                        } else {
                            icons::IconName::Plus
                        },
                        13.,
                    )
                    .text_color(theme::muted_text()),
                )
                .on_click({
                    let path = entry.path.clone();
                    cx.listener(move |this, _event, _window, cx| {
                        if staged {
                            this.unstage_paths(vec![path.clone()], cx);
                        } else {
                            this.stage_paths(vec![path.clone()], cx);
                        }
                        cx.stop_propagation();
                    })
                }),
            )
            .on_click({
                let key = key.clone();
                cx.listener(move |this, event: &ClickEvent, _window, cx| {
                    let modifiers = event.modifiers();
                    this.select(
                        key.clone(),
                        modifiers.control || modifiers.platform,
                        modifiers.shift,
                        cx,
                    );
                })
            })
            .on_mouse_down(MouseButton::Right, {
                let key = key.clone();
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    if !this.session.is_selected(&key) {
                        this.select(key.clone(), false, false, cx);
                    }
                    this.open_context_menu(event.position, cx);
                    cx.stop_propagation();
                })
            })
            .into_any_element()
    }

    fn render_selection_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let selected_count = self.session.selected_count();
        if selected_count == 0 {
            return div().h(px(0.)).flex_shrink_0().into_any_element();
        }
        let stage_count = self.session.selected_paths(false).len();
        let unstage_count = self.session.selected_paths(true).len();
        let mut toolbar = div()
            .id("git-selection-toolbar")
            .h(px(32.))
            .w_full()
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_xs()
                    .text_color(theme::muted_text())
                    .child(SharedString::from(
                        rust_i18n::t!("git.selection_count", count = selected_count).to_string(),
                    )),
            );
        if stage_count > 0 {
            toolbar = toolbar.child(
                Button::new("git-stage-selected-toolbar")
                    .size(ButtonSize::Icon(px(24.)))
                    .variant(ButtonVariant::Ghost)
                    .tooltip(rust_i18n::t!("git.stage_selected", count = stage_count).to_string())
                    .icon(icons::icon(icons::IconName::Plus, 13.).text_color(theme::muted_text()))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.stage_selected(cx);
                    })),
            );
        }
        if unstage_count > 0 {
            toolbar = toolbar.child(
                Button::new("git-unstage-selected-toolbar")
                    .size(ButtonSize::Icon(px(24.)))
                    .variant(ButtonVariant::Ghost)
                    .tooltip(
                        rust_i18n::t!("git.unstage_selected", count = unstage_count).to_string(),
                    )
                    .icon(icons::icon(icons::IconName::Minus, 13.).text_color(theme::muted_text()))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.unstage_selected(cx);
                    })),
            );
        }
        if self.session.can_discard_selection() {
            toolbar = toolbar.child(
                Button::new("git-discard-selected-toolbar")
                    .size(ButtonSize::Icon(px(24.)))
                    .variant(ButtonVariant::Ghost)
                    .tooltip(i18n::text("git.discard_selected"))
                    .icon(icons::icon(icons::IconName::Trash, 13.).text_color(theme::danger()))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.request_discard_selected(cx);
                    })),
            );
        }
        toolbar.into_any_element()
    }

    fn render_discard_confirmation(&self, count: usize, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("git-discard-confirmation")
            .flex_shrink_0()
            .w_full()
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::danger())
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text())
                    .child(SharedString::from(
                        rust_i18n::t!("git.discard_confirm", count = count).to_string(),
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::muted_text())
                    .child(SharedString::from(i18n::text("git.discard_confirm_detail"))),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .justify_end()
                    .gap_1()
                    .child(
                        Button::new("git-discard-cancel")
                            .size(ButtonSize::Small)
                            .variant(ButtonVariant::Secondary)
                            .label(i18n::text("git.discard_cancel"))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.cancel_discard(cx);
                            })),
                    )
                    .child(
                        Button::new("git-discard-confirm")
                            .size(ButtonSize::Small)
                            .variant(ButtonVariant::Danger)
                            .label(i18n::text("git.discard_confirm_action"))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.confirm_discard(cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_diff_pane(
        &mut self,
        compact: bool,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(entry) = self.selected_entry() else {
            return div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Hint::new(i18n::text("git.no_selection"))
                        .padding_x(px(16.))
                        .padding_y(px(16.)),
                )
                .into_any_element();
        };
        let key = ChangeKey::from(entry);
        let header = div()
            .h(px(38.))
            .flex_shrink_0()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .when(compact, |header| {
                header.child(
                    Button::new("git-back-to-changes")
                        .size(ButtonSize::Icon(px(28.)))
                        .variant(ButtonVariant::Ghost)
                        .tooltip(i18n::text("git.back_to_changes"))
                        .icon(
                            icons::icon(icons::IconName::ArrowLeft, 14.)
                                .text_color(theme::muted_text()),
                        )
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.back_to_changes(cx);
                        })),
                )
            })
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::text())
                    .child(SharedString::from(entry.path.clone())),
            )
            .child(status_glyph(entry.status))
            .child(
                Badge::new(if entry.staged {
                    i18n::text("git.staged")
                } else {
                    i18n::text("git.working")
                })
                .tone(if entry.staged {
                    BadgeTone::Success
                } else {
                    BadgeTone::Neutral
                }),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(theme::muted_text())
                    .child(SharedString::from(format!(
                        "+{} −{}",
                        entry.insertions, entry.deletions
                    ))),
            );

        let body = match &self.session.diff {
            DiffState::Idle => Hint::new(i18n::text("git.no_selection"))
                .centered()
                .into_any_element(),
            DiffState::Loading(loading_key) if loading_key == &key => {
                Hint::new(i18n::text("git.loading"))
                    .centered()
                    .into_any_element()
            }
            DiffState::Ready(ready_key, Some(file_diff))
                if ready_key == &key && file_diff.binary =>
            {
                Hint::new(i18n::text("git.binary"))
                    .centered()
                    .into_any_element()
            }
            DiffState::Ready(ready_key, Some(file_diff))
                if ready_key == &key && file_diff.lines.is_empty() =>
            {
                Hint::new(i18n::text("git.no_diff"))
                    .centered()
                    .into_any_element()
            }
            DiffState::Ready(ready_key, Some(file_diff)) if ready_key == &key => {
                let content_width = diff_content_width(file_diff);
                let key = key.clone();
                let staged = entry.staged;
                uniform_list(
                    if compact {
                        "git-diff-scroll-compact"
                    } else {
                        "git-diff-scroll"
                    },
                    file_diff.lines.len(),
                    cx.processor(
                        move |this, range: std::ops::Range<usize>, _window, cx| match &this
                            .session
                            .diff
                        {
                            DiffState::Ready(ready_key, Some(file_diff)) if ready_key == &key => {
                                file_diff.lines[range]
                                    .iter()
                                    .map(|line| {
                                        let hunk_action = (line.kind == DiffLineKind::Hunk)
                                            .then_some(line.hunk_index)
                                            .flatten()
                                            .map(|hunk_index| {
                                                this.render_hunk_action(staged, hunk_index, cx)
                                            });
                                        render_diff_line(line, content_width, hunk_action)
                                    })
                                    .collect::<Vec<_>>()
                            }
                            _ => Vec::new(),
                        },
                    ),
                )
                .track_scroll(&self.diff_scroll)
                .flex_1()
                .size_full()
                .min_w_0()
                .font_family("Zed Mono")
                .with_sizing_behavior(ListSizingBehavior::Auto)
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .into_any_element()
            }
            DiffState::Ready(ready_key, None) if ready_key == &key => {
                Hint::new(i18n::text("git.no_diff"))
                    .centered()
                    .into_any_element()
            }
            DiffState::Error(error_key, message) if error_key == &key => {
                Hint::new(message.clone()).centered().into_any_element()
            }
            _ => Hint::new(i18n::text("git.loading"))
                .centered()
                .into_any_element(),
        };

        let conflict_actions = (entry.status == ChangeStatus::Conflict)
            .then(|| self.render_conflict_actions(compact, entry.path.clone(), cx));

        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .child(header)
            .when_some(conflict_actions, |panel, actions| panel.child(actions))
            .child(body)
            .into_any_element()
    }

    fn render_conflict_actions(
        &self,
        compact: bool,
        path: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut panel = div()
            .w_full()
            .flex_shrink_0()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_2()
            .flex_wrap()
            .bg(theme::diff_del_bg())
            .border_b_1()
            .border_color(theme::danger())
            .child(icons::icon(icons::IconName::ShieldAlert, 14.).text_color(theme::danger()))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_xs()
                    .text_color(theme::danger())
                    .child(SharedString::from(i18n::text("git.conflict_actions"))),
            );
        if compact {
            panel = panel.flex_col().items_start();
        }

        let ours_path = path.clone();
        let theirs_path = path.clone();
        let resolved_path = path;
        panel
            .child(
                Button::new("git-conflict-ours")
                    .size(ButtonSize::Small)
                    .variant(ButtonVariant::Secondary)
                    .disabled(matches!(self.session.operation, OperationState::Running))
                    .label(i18n::text("git.conflict_use_ours"))
                    .icon(icons::icon(icons::IconName::GitBranch, 12.).text_color(theme::text()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.resolve_conflict(ours_path.clone(), ConflictResolution::Ours, cx);
                        cx.stop_propagation();
                    })),
            )
            .child(
                Button::new("git-conflict-theirs")
                    .size(ButtonSize::Small)
                    .variant(ButtonVariant::Secondary)
                    .disabled(matches!(self.session.operation, OperationState::Running))
                    .label(i18n::text("git.conflict_use_theirs"))
                    .icon(icons::icon(icons::IconName::GitBranch, 12.).text_color(theme::text()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.resolve_conflict(theirs_path.clone(), ConflictResolution::Theirs, cx);
                        cx.stop_propagation();
                    })),
            )
            .child(
                Button::new("git-conflict-resolved")
                    .size(ButtonSize::Small)
                    .variant(ButtonVariant::Ghost)
                    .disabled(matches!(self.session.operation, OperationState::Running))
                    .label(i18n::text("git.conflict_mark_resolved"))
                    .icon(icons::icon(icons::IconName::Check, 12.).text_color(theme::muted_text()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.resolve_conflict(
                            resolved_path.clone(),
                            ConflictResolution::MarkResolved,
                            cx,
                        );
                        cx.stop_propagation();
                    })),
            )
            .into_any_element()
    }

    fn render_hunk_action(
        &self,
        staged: bool,
        hunk_index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        Button::new(SharedString::from(format!(
            "git-{}-hunk-{hunk_index}",
            if staged { "unstage" } else { "stage" }
        )))
        .size(ButtonSize::Icon(px(22.)))
        .variant(ButtonVariant::Ghost)
        .disabled(matches!(self.session.operation, OperationState::Running))
        .tooltip(if staged {
            i18n::text("git.unstage_hunk")
        } else {
            i18n::text("git.stage_hunk")
        })
        .icon(
            icons::icon(
                if staged {
                    icons::IconName::Minus
                } else {
                    icons::IconName::Plus
                },
                12.,
            )
            .text_color(theme::muted_text()),
        )
        .on_click(cx.listener(move |this, _event, _window, cx| {
            this.toggle_hunk(hunk_index, cx);
            cx.stop_propagation();
        }))
        .into_any_element()
    }

    fn render_commit_panel(
        &mut self,
        compact: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focused = self.commit_editor.focus.is_focused(window);
        let focus = self.commit_editor.focus.clone();
        let staged_count = self.staged_count();
        let running = matches!(self.session.operation, OperationState::Running);
        let commit_label = rust_i18n::t!("git.commit_files", count = staged_count).to_string();
        let mut editor = div()
            .id("git-commit-message")
            .min_w_0()
            .min_h(px(52.))
            .max_h(px(84.))
            .px_3()
            .py_2()
            .relative()
            .overflow_y_scroll()
            .bg(theme::canvas())
            .border_1()
            .border_color(theme::border_strong())
            .rounded(px(theme::RADIUS_SM))
            .text_xs()
            .text_color(theme::text())
            .track_focus(&focus)
            .tab_stop(true)
            .focus(|style| style.border_color(theme::focus_ring()))
            .on_click({
                let focus = focus.clone();
                move |_event, window, cx| window.focus(&focus, cx)
            })
            .on_key_down(cx.listener(Self::handle_commit_editor_key));
        editor = if compact {
            editor.flex_1()
        } else {
            editor.w_full()
        };
        editor = editor.child(render_commit_editor_text(&self.commit_editor, focused));
        editor = editor.child(ime_input_canvas(focus, cx.entity()));

        let mut commit_button = Button::new("git-commit")
            .size(ButtonSize::Medium)
            .variant(ButtonVariant::Primary)
            .disabled(!self.can_commit())
            .loading(running)
            .icon(icons::icon(icons::IconName::Check, 14.).text_color(theme::canvas()))
            .label(commit_label)
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.commit_changes(cx);
            }));
        if !compact {
            commit_button = commit_button.full_width();
        }
        let controls = div()
            .w_full()
            .flex()
            .gap_2()
            .when(compact, |controls| controls.items_end())
            .when(!compact, |controls| controls.flex_col())
            .child(editor)
            .child(commit_button);
        let mut panel = div()
            .id("git-commit-panel")
            .flex_shrink_0()
            .p_2()
            .flex()
            .flex_col()
            .gap_2()
            .bg(theme::sidebar())
            .border_b_1()
            .border_color(theme::border())
            .child(controls);
        if let OperationState::Error(message) = &self.session.operation {
            panel = panel.child(
                div()
                    .w_full()
                    .text_xs()
                    .text_color(theme::danger())
                    .child(SharedString::from(message.clone())),
            );
        }
        panel
            .on_click(|_event, _window, cx| cx.stop_propagation())
            .into_any_element()
    }
}

fn render_commit_editor_text(editor: &CommitEditor, focused: bool) -> AnyElement {
    if editor.state.value.is_empty() {
        let mut row = div().h(px(18.)).flex().items_center();
        if focused {
            row = row.child(text_caret(px(15.)));
        }
        if editor.state.ime_marked_text.is_empty() {
            row = row.child(
                div()
                    .text_color(theme::faint_text())
                    .child(SharedString::from(i18n::text(
                        "git.commit_message_placeholder",
                    ))),
            );
        } else {
            row = row.child(marked_text_span(editor.state.ime_marked_text.as_str()));
        }
        return row.into_any_element();
    }

    let selection = editor.selection();
    let mut content = div().flex().flex_col();
    let mut line_start = 0;
    for line in editor.state.value.split('\n') {
        let line_end = line_start + line.len();
        let mut row = div().h(px(18.)).flex().items_center();
        if let Some((selection_start, selection_end)) = selection {
            let selected_start = selection_start.clamp(line_start, line_end);
            let selected_end = selection_end.clamp(line_start, line_end);
            row = row
                .child(text_span(&editor.state.value[line_start..selected_start]))
                .when(selected_start < selected_end, |row| {
                    row.child(div().flex_shrink_0().bg(theme::accent_soft()).child(
                        SharedString::from(
                            editor.state.value[selected_start..selected_end].to_string(),
                        ),
                    ))
                })
                .child(text_span(&editor.state.value[selected_end..line_end]));
        } else if (line_start..=line_end).contains(&editor.state.cursor) {
            row = row.child(text_span(
                &editor.state.value[line_start..editor.state.cursor],
            ));
            if focused {
                row = row.child(text_caret(px(15.)));
            }
            if !editor.state.ime_marked_text.is_empty() {
                row = row.child(marked_text_span(editor.state.ime_marked_text.as_str()));
            }
            row = row.child(text_span(
                &editor.state.value[editor.state.cursor..line_end],
            ));
        } else {
            row = row.child(text_span(line));
        }
        content = content.child(row);
        line_start = line_end + 1;
    }
    content.into_any_element()
}

const DIFF_GUTTER_WIDTH: f32 = 112.;
const DIFF_CHARACTER_WIDTH: f32 = 12.;
const DIFF_ROW_HEIGHT: f32 = 20.;

/// 单等宽字符按 12px 预留宽度，避免为每一行调用文本布局系统。
/// 这个上界覆盖中日韩全角字符；横向滚动范围略宽于实际文本是可接受的。
fn diff_content_width(file_diff: &crossh_core::git::FileDiff) -> Pixels {
    let widest_chars = file_diff
        .lines
        .iter()
        .map(|line| line.text.chars().count())
        .max()
        .unwrap_or_default();
    px(DIFF_GUTTER_WIDTH + widest_chars as f32 * DIFF_CHARACTER_WIDTH)
}

fn render_diff_line(
    line: &DiffLine,
    content_width: Pixels,
    hunk_action: Option<AnyElement>,
) -> AnyElement {
    let (background, foreground, rail) = match line.kind {
        DiffLineKind::Hunk => (theme::surface(), theme::muted_text(), theme::info()),
        DiffLineKind::Added => (theme::diff_add_bg(), theme::diff_add_fg(), theme::accent()),
        DiffLineKind::Removed => (theme::diff_del_bg(), theme::diff_del_fg(), theme::danger()),
        DiffLineKind::Context => (theme::canvas(), theme::text(), theme::canvas()),
    };
    if line.kind == DiffLineKind::Hunk {
        return div()
            .min_w(content_width)
            .h(px(DIFF_ROW_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .bg(background)
            .child(div().w(px(2.)).flex_shrink_0().bg(rail))
            .child(
                div()
                    .px_2()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(foreground)
                    .child(SharedString::from(line.text.clone())),
            )
            .child(div().flex_1())
            .when_some(hunk_action, |row, action| row.child(action))
            .into_any_element();
    }
    let number = |value: Option<u32>| value.map(|n| n.to_string()).unwrap_or_default();
    div()
        .min_w(content_width)
        .h(px(DIFF_ROW_HEIGHT))
        .flex_none()
        .flex()
        .items_center()
        .bg(background)
        .child(div().w(px(2.)).flex_shrink_0().bg(rail))
        .child(line_number(number(line.old_ln), foreground))
        .child(line_number(number(line.new_ln), foreground))
        .child(
            div()
                .flex_shrink_0()
                .px_2()
                .whitespace_nowrap()
                .text_xs()
                .text_color(foreground)
                .child(SharedString::from(line.text.clone())),
        )
        .into_any_element()
}

fn line_number(text: String, color: gpui::Rgba) -> AnyElement {
    div()
        .w(px(42.))
        .flex_shrink_0()
        .px(px(6.))
        .py(px(1.))
        .flex()
        .justify_end()
        .bg(theme::sidebar())
        .border_r_1()
        .border_color(theme::border())
        .text_xs()
        .text_color(color)
        .child(SharedString::from(text))
        .into_any_element()
}

fn status_color(status: ChangeStatus) -> gpui::Rgba {
    match status {
        ChangeStatus::Modified => theme::warning(),
        ChangeStatus::Added | ChangeStatus::Renamed => theme::accent(),
        ChangeStatus::Deleted | ChangeStatus::Conflict => theme::danger(),
        ChangeStatus::Untracked => theme::faint_text(),
    }
}

fn render_commit_file_row(file: &CommitFileChange) -> AnyElement {
    let path = file
        .old_path
        .as_ref()
        .map(|old| format!("{old} -> {}", file.path))
        .unwrap_or_else(|| file.path.clone());
    let stats = if file.binary {
        i18n::text("git.binary")
    } else {
        format!("+{} −{}", file.insertions, file.deletions)
    };
    div()
        .h(px(34.))
        .w_full()
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .border_b_1()
        .border_color(theme::border())
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_xs()
                .text_color(theme::text())
                .child(SharedString::from(path)),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_xs()
                .text_color(theme::muted_text())
                .child(SharedString::from(stats)),
        )
        .into_any_element()
}

fn status_glyph(status: ChangeStatus) -> AnyElement {
    div()
        .w(px(18.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(status_color(status))
        .child(SharedString::from(status.glyph()))
        .into_any_element()
}

fn status_badge(text: String) -> AnyElement {
    Badge::new(text).tone(BadgeTone::Info).into_any_element()
}

#[cfg(test)]
mod tests {
    use crossh_core::git::{DiffLine, FileDiff};
    use gpui::{
        ListHorizontalSizingBehavior, ListSizingBehavior, Render, TestAppContext,
        UniformListScrollHandle, px, size, uniform_list,
    };

    use super::*;

    struct DiffScrollTestView {
        diff: FileDiff,
        scroll: UniformListScrollHandle,
    }

    impl Render for DiffScrollTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let content_width = diff_content_width(&self.diff);
            let lines = self.diff.lines.clone();
            uniform_list(
                "git-diff-scroll-test",
                lines.len(),
                move |range: std::ops::Range<usize>, _window, _cx| {
                    lines[range]
                        .iter()
                        .map(|line| render_diff_line(line, content_width, None))
                        .collect::<Vec<_>>()
                },
            )
            .track_scroll(&self.scroll)
            .size_full()
            .with_sizing_behavior(ListSizingBehavior::Auto)
            .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        }
    }

    #[gpui::test]
    fn long_diff_line_has_horizontal_scroll_range(cx: &mut TestAppContext) {
        let diff = FileDiff {
            lines: vec![DiffLine {
                kind: DiffLineKind::Context,
                hunk_index: None,
                old_ln: Some(1),
                new_ln: Some(1),
                text: "x".repeat(240),
            }],
            ..Default::default()
        };
        let scroll = UniformListScrollHandle::new();
        cx.open_window(size(px(480.), px(320.)), {
            let scroll = scroll.clone();
            move |_, _| DiffScrollTestView { diff, scroll }
        });
        cx.run_until_parked();

        assert!(
            scroll.0.borrow().base_handle.max_offset().x > px(0.),
            "a long diff line must create horizontal scroll range"
        );
    }
}

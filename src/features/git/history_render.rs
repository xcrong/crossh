//! Git Log 的提交图、筛选列表和提交详情渲染。

use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ListSizingBehavior,
    ParentElement, PathBuilder, SharedString, StatefulInteractiveElement, Styled, canvas, div,
    point, prelude::FluentBuilder, px, uniform_list,
};

use crate::shared::i18n;
use crossh_core::git_history::{CommitDetail, CommitFileChange, HistoryRef, HistoryRefKind};
use crossh_core::git_history_graph::HistoryGraphRow;
use crossh_ui::{icons, theme};
use crossh_ui_component::{
    Badge, BadgeTone, Button, ButtonSize, ButtonVariant, Hint, ListState, TextInput, list_pane,
    list_state_body, pane_toolbar, scroll_y, selectable_row,
};

use super::history::{HistoryDetailState, HistoryListState, HistoryRow};
use super::window::GitWindow;
use super::{GIT_HISTORY_CONTEXT, MoveHistoryDown, MoveHistoryUp};

const HISTORY_ROW_HEIGHT: f32 = 68.;
const GRAPH_WIDTH: f32 = 88.;
const GRAPH_LANE_WIDTH: f32 = 16.;
const GRAPH_LEFT: f32 = 16.;
const GRAPH_NODE_SIZE: f32 = 10.;
const GRAPH_TRANSITION_HEIGHT: f32 = 16.;
const GRAPH_STROKE_WIDTH: f32 = 2.;

impl GitWindow {
    pub(super) fn render_history_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let width = super::model::clamp_changes_pane_width(self.changes_pane_width.get());
        let resizer = crossh_ui_component::SplitResizer::new(
            "git-history-resize",
            self.changes_pane_dragging.clone(),
            self.changes_pane_width.clone(),
        )
        .min_width(super::model::CHANGES_PANE_MIN_WIDTH)
        .max_width(super::model::CHANGES_PANE_MAX_WIDTH);
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

    pub(super) fn render_history_list(
        &mut self,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focus = self.history_focus.clone();
        let rows = self.session.history.visible_rows();
        let list_state = match &self.session.history.list_state {
            HistoryListState::Idle | HistoryListState::Loading => {
                ListState::Loading(i18n::text("git.loading").into())
            }
            HistoryListState::Error(error) => ListState::Error(error.clone().into()),
            HistoryListState::Ready if rows.is_empty() => {
                let message = if self.session.history.query.is_empty() {
                    i18n::text("git.no_history")
                } else {
                    i18n::text("git.history_no_matches")
                };
                ListState::Empty(message.into())
            }
            HistoryListState::Ready => ListState::Ready,
        };
        let body = list_state_body(list_state, || {
            let count = rows.len();
            uniform_list(
                if compact {
                    "git-history-list-compact"
                } else {
                    "git-history-list"
                },
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                    rows[range]
                        .iter()
                        .map(|row| this.render_history_row(row, cx))
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.history_scroll)
            .flex_1()
            .min_h_0()
            .with_sizing_behavior(ListSizingBehavior::Auto)
            .into_any_element()
        });

        list_pane(
            if compact {
                "git-history-compact"
            } else {
                "git-history-pane"
            },
            focus,
            GIT_HISTORY_CONTEXT,
        )
        .on_action(cx.listener(|this, _: &MoveHistoryUp, _window, cx| {
            this.move_history_selection(-1, cx);
        }))
        .on_action(cx.listener(|this, _: &MoveHistoryDown, _window, cx| {
            this.move_history_selection(1, cx);
        }))
        .child(self.render_history_toolbar(cx))
        .child(body)
        .into_any_element()
    }

    fn render_history_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let search_focus = self.history_search_focus.clone();
        let search_focus_for_click = search_focus.clone();
        pane_toolbar()
            .id("git-history-toolbar")
            .on_click(move |_event, window, cx| {
                window.focus(&search_focus_for_click, cx);
                cx.stop_propagation();
            })
            .child(icons::icon(icons::IconName::Search, 13.).text_color(theme::muted_text()))
            .child(
                TextInput::new("git-history-filter", search_focus)
                    .value(self.history_query.value.clone())
                    .placeholder(i18n::text("git.history_filter"))
                    .ime_marked_text(self.history_query.ime_marked_text.clone())
                    .text_color(if self.history_query.value.is_empty() {
                        theme::faint_text()
                    } else {
                        theme::text()
                    })
                    .bg(theme::surface())
                    .flex_1()
                    .entity(cx.entity())
                    .on_key_down(cx.listener(Self::handle_history_search_key)),
            )
            .child(
                Button::new("git-history-refresh")
                    .size(ButtonSize::Icon(px(26.)))
                    .variant(ButtonVariant::Ghost)
                    .loading(self.session.history.list_state.is_loading())
                    .tooltip(i18n::text("git.refresh"))
                    .icon(
                        icons::icon(icons::IconName::RefreshCw, 13.)
                            .text_color(theme::muted_text()),
                    )
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.refresh_history(true, cx);
                        cx.stop_propagation();
                    })),
            )
            .into_any_element()
    }

    fn render_history_row(&self, row: &HistoryRow, cx: &mut Context<Self>) -> AnyElement {
        let entry = &row.entry;
        let selected = self.session.history.selected.as_deref() == Some(entry.id.as_str());
        let id = entry.id.clone();
        let refs = self.session.history.refs_for(&entry.id);
        let graph = render_history_graph(&row.graph, selected);

        let mut decorations = div()
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1();
        for reference in refs.iter().take(3) {
            decorations = decorations.child(render_history_ref(reference));
        }
        if refs.len() > 3 {
            decorations = decorations.child(
                div()
                    .text_xs()
                    .text_color(theme::faint_text())
                    .child(SharedString::from(format!("+{}", refs.len() - 3))),
            );
        }

        selectable_row(
            SharedString::from(format!("git-history-entry-{}", entry.id)),
            selected,
            px(HISTORY_ROW_HEIGHT),
        )
        .pr_2()
        .flex()
        .items_center()
        .child(graph)
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .justify_center()
                .gap_1()
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(decorations)
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
                        .min_w_0()
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
                ),
        )
        .on_click(cx.listener(move |this, _event, _window, cx| {
            this.select_history_commit(id.clone(), cx);
        }))
        .into_any_element()
    }

    pub(super) fn render_history_detail(
        &self,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        let selected_refs = self
            .session
            .history
            .selected
            .as_deref()
            .map(|id| self.session.history.refs_for(id))
            .unwrap_or_default();
        let mut header = div()
            .h(px(46.))
            .flex_shrink_0()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border());
        if compact {
            header = header.child(
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
            );
        }
        header = header.child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::text())
                .child(SharedString::from(selected_subject)),
        );
        for reference in selected_refs.iter().take(3) {
            header = header.child(render_history_ref(reference));
        }

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
                .padded()
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
                div()
                    .h(px(38.))
                    .flex_shrink_0()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .text_xs()
                    .child(
                        div()
                            .text_color(theme::accent())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(SharedString::from(detail.summary.short_id.clone())),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_color(theme::muted_text())
                            .child(SharedString::from(detail.summary.author.clone())),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(theme::faint_text())
                            .child(SharedString::from(detail.summary.date.clone())),
                    ),
            )
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
}

fn render_history_ref(reference: &HistoryRef) -> AnyElement {
    let tone = match reference.kind {
        HistoryRefKind::LocalBranch | HistoryRefKind::Head if reference.current => {
            BadgeTone::Accent
        }
        HistoryRefKind::LocalBranch => BadgeTone::Info,
        HistoryRefKind::RemoteBranch => BadgeTone::Neutral,
        HistoryRefKind::Tag => BadgeTone::Warning,
        HistoryRefKind::Head => BadgeTone::Accent,
    };
    Badge::new(reference.name.clone())
        .tone(tone)
        .into_any_element()
}

fn render_history_graph(graph: &HistoryGraphRow, selected: bool) -> AnyElement {
    let graph_for_canvas = graph.clone();
    let node_x = GRAPH_LEFT + graph.node_lane as f32 * GRAPH_LANE_WIDTH;
    let node_color = graph_color(graph.node_color);
    let node = div()
        .absolute()
        .left(px(node_x - GRAPH_NODE_SIZE / 2.))
        .top(px(HISTORY_ROW_HEIGHT / 2. - GRAPH_NODE_SIZE / 2.))
        .size(px(GRAPH_NODE_SIZE))
        .rounded(px(GRAPH_NODE_SIZE / 2.))
        .bg(node_color)
        .border_1()
        .border_color(theme::sidebar())
        .when(selected, |node| node.border_1().border_color(theme::text()));

    div()
        .relative()
        .w(px(GRAPH_WIDTH))
        .h_full()
        .flex_shrink_0()
        .child(
            canvas(
                |_bounds, _window, _cx| (),
                move |bounds, _state, window, _cx| {
                    let center_y = bounds.size.height.as_f32() / 2.;
                    if graph_for_canvas.node_has_incoming {
                        let mut path = PathBuilder::stroke(px(GRAPH_STROKE_WIDTH));
                        path.move_to(point(bounds.origin.x + px(node_x), bounds.origin.y));
                        path.line_to(point(
                            bounds.origin.x + px(node_x),
                            bounds.origin.y + px(center_y),
                        ));
                        if let Ok(path) = path.build() {
                            window.paint_path(path, node_color);
                        }
                    }
                    for edge in &graph_for_canvas.incoming_edges {
                        let from_x = GRAPH_LEFT + edge.from_lane as f32 * GRAPH_LANE_WIDTH;
                        let to_x = GRAPH_LEFT + edge.to_lane as f32 * GRAPH_LANE_WIDTH;
                        let from_y = 0.;
                        let to_y = GRAPH_TRANSITION_HEIGHT;
                        let mut path = PathBuilder::stroke(px(GRAPH_STROKE_WIDTH));
                        path.move_to(point(
                            bounds.origin.x + px(from_x),
                            bounds.origin.y + px(from_y),
                        ));
                        if edge.from_lane == edge.to_lane {
                            path.line_to(point(
                                bounds.origin.x + px(to_x),
                                bounds.origin.y + px(to_y),
                            ));
                        } else {
                            let control_offset = (to_y - from_y) * 0.55;
                            path.cubic_bezier_to(
                                point(bounds.origin.x + px(to_x), bounds.origin.y + px(to_y)),
                                point(
                                    bounds.origin.x + px(from_x),
                                    bounds.origin.y + px(from_y + control_offset),
                                ),
                                point(
                                    bounds.origin.x + px(to_x),
                                    bounds.origin.y + px(to_y - control_offset),
                                ),
                            );
                        }
                        if let Ok(path) = path.build() {
                            window.paint_path(path, graph_color(edge.color));
                        }
                    }
                    for edge in &graph_for_canvas.edges {
                        let from_x = GRAPH_LEFT + edge.from_lane as f32 * GRAPH_LANE_WIDTH;
                        let to_x = GRAPH_LEFT + edge.to_lane as f32 * GRAPH_LANE_WIDTH;
                        let from_y = if edge.from_lane == graph_for_canvas.node_lane {
                            center_y
                        } else if graph_for_canvas
                            .incoming_edges
                            .iter()
                            .any(|incoming| incoming.to_lane == edge.from_lane)
                        {
                            GRAPH_TRANSITION_HEIGHT
                        } else {
                            0.
                        };
                        let to_y = bounds.size.height.as_f32();
                        let mut path = PathBuilder::stroke(px(GRAPH_STROKE_WIDTH));
                        path.move_to(point(
                            bounds.origin.x + px(from_x),
                            bounds.origin.y + px(from_y),
                        ));
                        if edge.from_lane == edge.to_lane {
                            path.line_to(point(
                                bounds.origin.x + px(to_x),
                                bounds.origin.y + px(to_y),
                            ));
                        } else {
                            let control_offset = (to_y - from_y) * 0.55;
                            path.cubic_bezier_to(
                                point(bounds.origin.x + px(to_x), bounds.origin.y + px(to_y)),
                                point(
                                    bounds.origin.x + px(from_x),
                                    bounds.origin.y + px(from_y + control_offset),
                                ),
                                point(
                                    bounds.origin.x + px(to_x),
                                    bounds.origin.y + px(to_y - control_offset),
                                ),
                            );
                        }
                        if let Ok(path) = path.build() {
                            window.paint_path(path, graph_color(edge.color));
                        }
                    }
                },
            )
            .absolute()
            .inset_0(),
        )
        .child(node)
        .into_any_element()
}

fn graph_color(index: usize) -> gpui::Rgba {
    match index % 5 {
        0 => theme::accent(),
        1 => theme::info(),
        2 => theme::warning(),
        3 => theme::danger(),
        _ => theme::muted_text(),
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

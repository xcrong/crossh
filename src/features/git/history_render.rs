//! Git Log 的提交图、筛选列表和提交详情渲染。

use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ListHorizontalSizingBehavior,
    ListSizingBehavior, ParentElement, PathBuilder, SharedString, StatefulInteractiveElement,
    Styled, canvas, div, point, prelude::FluentBuilder, px, uniform_list,
};

use crate::shared::i18n;
use crossh_core::git_history::{CommitDetail, CommitFileChange, HistoryRef, HistoryRefKind};
use crossh_core::git_history_graph::HistoryGraphRow;
use crossh_editor::{Scrollbar, ScrollbarMode};
use crossh_ui::{icons, theme};
use crossh_ui_component::{
    Badge, BadgeTone, Button, ButtonSize, ButtonVariant, Hint, ListStatus, TextInput, list_pane,
    list_state_body, pane_toolbar, scroll_y, selectable_row,
};

use super::history::{HistoryDetailState, HistoryFileDiffState, HistoryListState, HistoryRow};
use super::render::{diff_content_width, render_diff_line};
use super::window::GitWindow;
use super::{GIT_HISTORY_CONTEXT, MoveHistoryDown, MoveHistoryUp};

const HISTORY_ROW_HEIGHT: f32 = 68.;
const HISTORY_FILE_ROW_HEIGHT: f32 = 34.;
const HISTORY_FILE_COL_WIDTH: f32 = 260.;
/// 窄屏纵向时文件列表按内容定高，超出后内部滚动，剩余空间留给 diff。
const HISTORY_FILE_LIST_MAX_ROWS: usize = 5;
const GRAPH_WIDTH: f32 = 88.;
const GRAPH_LANE_WIDTH: f32 = 16.;
const GRAPH_LEFT: f32 = 16.;
const GRAPH_NODE_SIZE: f32 = 10.;
const GRAPH_TRANSITION_HEIGHT: f32 = 16.;
const GRAPH_STROKE_WIDTH: f32 = 2.;

impl GitWindow {
    pub(super) fn render_history_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let width = super::window::clamp_changes_pane_width(self.changes_pane_width.get());
        let resizer = crossh_ui_component::SplitResizer::new(
            "git-history-resize",
            self.changes_pane_dragging.clone(),
            self.changes_pane_width.clone(),
        )
        .min_width(super::window::CHANGES_PANE_MIN_WIDTH)
        .max_width(super::window::CHANGES_PANE_MAX_WIDTH);
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
                ListStatus::Loading(i18n::text("git.loading").into())
            }
            HistoryListState::Error(error) => ListStatus::Error(error.clone().into()),
            HistoryListState::Ready if rows.is_empty() => {
                let message = if self.session.history.query.is_empty() {
                    i18n::text("git.no_history")
                } else {
                    i18n::text("git.history_no_matches")
                };
                ListStatus::Empty(message.into())
            }
            HistoryListState::Ready => ListStatus::Ready,
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
            HistoryDetailState::Ready(detail) => self.render_commit_detail(detail, compact, cx),
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

    fn render_commit_detail(
        &self,
        detail: &CommitDetail,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let files_bar = div()
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
                rust_i18n::t!("git.files_changed", count = detail.files.len()).to_string(),
            ))
            .into_any_element();
        let file_list = self.render_commit_file_list(detail, compact, cx);
        let meta = self.render_commit_meta(detail);
        let message = self.render_commit_message(detail);
        if compact {
            // 窄屏纵向：文件列表按内容封顶，剩余空间给 diff。
            return div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(meta)
                .child(message)
                .child(files_bar)
                .child(file_list)
                .child(self.render_history_file_diff_header(detail))
                .child(self.render_history_file_diff(detail, cx))
                .into_any_element();
        }
        // 宽屏三栏：commit 信息横跨，左文件列右 diff 列，diff 独占整列高度。
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(meta)
            .child(message)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .w(px(HISTORY_FILE_COL_WIDTH))
                            .flex_shrink_0()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .border_r_1()
                            .border_color(theme::border_strong())
                            .child(files_bar)
                            .child(file_list),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .child(self.render_history_file_diff_header(detail))
                            .child(self.render_history_file_diff(detail, cx)),
                    ),
            )
            .into_any_element()
    }

    fn render_commit_meta(&self, detail: &CommitDetail) -> AnyElement {
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
            )
            .into_any_element()
    }

    fn render_commit_message(&self, detail: &CommitDetail) -> AnyElement {
        let body = detail.body.trim();
        scroll_y(&self.history_message_scroll)
            .id("git-commit-message")
            .max_h(px(90.))
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
            }))
            .into_any_element()
    }

    fn render_commit_file_list(
        &self,
        detail: &CommitDetail,
        capped: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let files = detail.files.clone();
        let file_count = files.len();
        if files.is_empty() {
            return Hint::new(i18n::text("git.no_files_changed"))
                .padded()
                .into_any_element();
        }
        let list = uniform_list(
            "git-commit-files",
            file_count,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                files[range]
                    .iter()
                    .map(|file| this.render_history_file_row(file, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.history_detail_scroll)
        .with_sizing_behavior(ListSizingBehavior::Auto);
        if capped {
            // 窄屏纵向：按内容定高，大提交在封顶高度内滚动。
            let list_height =
                px(HISTORY_FILE_ROW_HEIGHT * file_count.min(HISTORY_FILE_LIST_MAX_ROWS) as f32);
            div()
                .h(list_height)
                .flex_shrink_0()
                .w_full()
                .child(list.size_full())
                .into_any_element()
        } else {
            list.flex_1().min_h_0().into_any_element()
        }
    }

    fn render_history_file_row(
        &self,
        file: &CommitFileChange,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.session.history.selected_file.as_deref() == Some(file.path.as_str());
        let id = file.path.clone();
        selectable_row(
            SharedString::from(format!("git-commit-file-{}", file.path)),
            selected,
            px(HISTORY_FILE_ROW_HEIGHT),
        )
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_xs()
                .text_color(theme::text())
                .child(SharedString::from(commit_file_display_path(file))),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_xs()
                .text_color(theme::muted_text())
                .child(SharedString::from(commit_file_stats(file))),
        )
        .on_click(cx.listener(move |this, _event, _window, cx| {
            this.select_history_file(id.clone(), cx);
        }))
        .into_any_element()
    }

    fn render_history_file_diff_header(&self, detail: &CommitDetail) -> AnyElement {
        let content = match self
            .session
            .history
            .selected_file
            .as_deref()
            .and_then(|path| detail.files.iter().find(|file| file.path == path))
        {
            Some(file) => div()
                .min_w_0()
                .flex_1()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .child(SharedString::from(commit_file_display_path(file))),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .child(SharedString::from(commit_file_stats(file))),
                )
                .into_any_element(),
            None => div()
                .min_w_0()
                .flex_1()
                .truncate()
                .child(SharedString::from(i18n::text("git.no_selection")))
                .into_any_element(),
        };
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
            .child(content)
            .into_any_element()
    }

    fn render_history_file_diff(
        &self,
        detail: &CommitDetail,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.session.history.selected_file.clone();
        let Some(path) = selected else {
            return Hint::new(i18n::text("git.no_selection"))
                .centered()
                .into_any_element();
        };
        match self.session.history.file_diffs.get(&path) {
            None | Some(HistoryFileDiffState::Idle) | Some(HistoryFileDiffState::Loading) => {
                Hint::new(i18n::text("git.loading"))
                    .centered()
                    .into_any_element()
            }
            Some(HistoryFileDiffState::Error(error)) => {
                Hint::new(error.clone()).centered().into_any_element()
            }
            Some(HistoryFileDiffState::Ready(None)) => Hint::new(i18n::text("git.no_diff"))
                .centered()
                .into_any_element(),
            Some(HistoryFileDiffState::Ready(Some(file_diff))) if file_diff.binary => {
                Hint::new(i18n::text("git.binary"))
                    .centered()
                    .into_any_element()
            }
            Some(HistoryFileDiffState::Ready(Some(file_diff))) if file_diff.lines.is_empty() => {
                Hint::new(i18n::text("git.no_diff"))
                    .centered()
                    .into_any_element()
            }
            Some(HistoryFileDiffState::Ready(Some(file_diff))) => {
                let content_width = diff_content_width(file_diff);
                let commit_id = detail.summary.id.clone();
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(
                        uniform_list(
                            "git-commit-file-diff",
                            file_diff.lines.len(),
                            cx.processor(
                                move |this, range: std::ops::Range<usize>, _window, _cx| match this
                                    .session
                                    .history
                                    .file_diffs
                                    .get(&path)
                                {
                                    Some(HistoryFileDiffState::Ready(Some(file_diff)))
                                        if this.session.history.selected.as_deref()
                                            == Some(commit_id.as_str())
                                            && this.session.history.selected_file.as_deref()
                                                == Some(path.as_str()) =>
                                    {
                                        file_diff.lines[range]
                                            .iter()
                                            .map(|line| render_diff_line(line, content_width, None))
                                            .collect::<Vec<_>>()
                                    }
                                    _ => Vec::new(),
                                },
                            ),
                        )
                        .track_scroll(&self.history_file_diff_scroll)
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .font_family("Lilex")
                        .with_sizing_behavior(ListSizingBehavior::Auto)
                        .with_horizontal_sizing_behavior(
                            ListHorizontalSizingBehavior::Unconstrained,
                        ),
                    )
                    // 常显、加粗的横向滚动条：长行只靠 Shift+滚轮发现不了。
                    .child(
                        div().absolute().inset_0().child(
                            Scrollbar::horizontal(&self.history_file_diff_scroll)
                                .id("git-commit-file-diff-scrollbar")
                                .mode(ScrollbarMode::Always)
                                .styles(|styles| {
                                    styles.thumb(|thumb| thumb.width(px(6.)).bg(theme::accent()))
                                })
                                .viewport_from_layout(),
                        ),
                    )
                    .into_any_element()
            }
        }
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

fn commit_file_display_path(file: &CommitFileChange) -> String {
    file.old_path
        .as_ref()
        .map(|old| format!("{old} -> {}", file.path))
        .unwrap_or_else(|| file.path.clone())
}

fn commit_file_stats(file: &CommitFileChange) -> String {
    if file.binary {
        i18n::text("git.binary")
    } else {
        format!("+{} −{}", file.insertions, file.deletions)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{
        App, Bounds, Context, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
        LayoutId, Length, ListHorizontalSizingBehavior, ListSizingBehavior, ParentElement, Pixels,
        Render, ScrollDelta, ScrollWheelEvent, Styled, TestAppContext, UniformListScrollHandle,
        Window, div, point, px, uniform_list,
    };

    /// 固定 4000px 宽的探针行，记录每次 prepaint 的行原点 x。
    /// 列表横滚时行整体左移，原点 x 减小。
    struct OriginProbe {
        recorder: Rc<Cell<f32>>,
    }

    impl IntoElement for OriginProbe {
        type Element = Self;

        fn into_element(self) -> Self::Element {
            self
        }
    }

    impl Element for OriginProbe {
        type RequestLayoutState = ();
        type PrepaintState = ();

        fn id(&self) -> Option<ElementId> {
            None
        }

        fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
            None
        }

        fn request_layout(
            &mut self,
            _id: Option<&GlobalElementId>,
            _inspector_id: Option<&InspectorElementId>,
            window: &mut Window,
            cx: &mut App,
        ) -> (LayoutId, ()) {
            let mut style = gpui::Style::default();
            style.size.width = Length::Definite(px(4000.).into());
            style.size.height = Length::Definite(px(20.).into());
            (window.request_layout(style, [], cx), ())
        }

        fn prepaint(
            &mut self,
            _id: Option<&GlobalElementId>,
            _inspector_id: Option<&InspectorElementId>,
            bounds: Bounds<Pixels>,
            _request_layout: &mut (),
            _window: &mut Window,
            _cx: &mut App,
        ) {
            self.recorder.set(bounds.origin.x.into());
        }

        fn paint(
            &mut self,
            _id: Option<&GlobalElementId>,
            _inspector_id: Option<&InspectorElementId>,
            _bounds: Bounds<Pixels>,
            _request_layout: &mut (),
            _prepaint: &mut (),
            _window: &mut Window,
            _cx: &mut App,
        ) {
        }
    }

    struct ProbeList {
        scroll: UniformListScrollHandle,
        recorder: Rc<Cell<f32>>,
        nested: bool,
    }

    impl Render for ProbeList {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let recorder = self.recorder.clone();
            // 与历史 diff 列表完全相同的构造链。
            let list = uniform_list(
                "probe-diff-list",
                30,
                cx.processor(move |_, range: std::ops::Range<usize>, _, _| {
                    range
                        .map(|_| {
                            OriginProbe {
                                recorder: recorder.clone(),
                            }
                            .into_any_element()
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .with_sizing_behavior(ListSizingBehavior::Auto)
            .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained);
            if self.nested {
                // 生产嵌套：文件列（固定 260）+ diff 列。
                div()
                    .size_full()
                    .flex()
                    .child(
                        div()
                            .w(px(260.))
                            .flex_shrink_0()
                            .min_h_0()
                            .flex()
                            .flex_col(),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .child(list.flex_1().min_h_0().min_w_0()),
                    )
                    .into_any_element()
            } else {
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(list.flex_1().min_h_0())
                    .into_any_element()
            }
        }
    }

    fn scrolled_horizontally(cx: &mut TestAppContext, nested: bool) -> (f32, f32) {
        let recorder = Rc::new(Cell::new(f32::MAX));
        let view_recorder = recorder.clone();
        let (_, cx) = cx.add_window_view(|_, _| ProbeList {
            scroll: UniformListScrollHandle::new(),
            recorder: view_recorder,
            nested,
        });
        cx.run_until_parked();
        let before = recorder.get();
        assert!(
            before < f32::MAX,
            "probe row should be laid out before scrolling"
        );
        let position = cx.update(|window, _| {
            let bounds = window.bounds();
            point(
                bounds.origin.x + bounds.size.width - px(100.),
                bounds.origin.y + bounds.size.height / 2.,
            )
        });
        cx.simulate_event(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(-600.), px(0.))),
            ..Default::default()
        });
        cx.run_until_parked();
        (before, recorder.get())
    }

    #[gpui::test]
    fn bare_diff_list_scrolls_horizontally(cx: &mut TestAppContext) {
        let (before, after) = scrolled_horizontally(cx, false);
        assert!(
            after < before - 500.,
            "bare list should shift rows left: before={before} after={after}"
        );
    }

    #[gpui::test]
    fn nested_diff_list_scrolls_horizontally(cx: &mut TestAppContext) {
        let (before, after) = scrolled_horizontally(cx, true);
        assert!(
            after < before - 500.,
            "nested list should shift rows left: before={before} after={after}"
        );
    }
}

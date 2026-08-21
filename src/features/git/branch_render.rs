//! Git Branch 页面渲染。

use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ListSizingBehavior,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder,
    px, uniform_list,
};

use crate::shared::i18n;
use crossh_core::git_branch::BranchSummary;
use crossh_ui::{icons, theme};
use crossh_ui_component::{
    Badge, BadgeTone, Button, ButtonSize, ButtonVariant, ListState, list_pane, list_state_body,
    pane_operation_error, selectable_row,
};

use super::branch::BranchListState;
use super::session::OperationState;
use super::window::GitWindow;
use super::{GIT_BRANCH_CONTEXT, MoveBranchDown, MoveBranchUp, SwitchSelectedBranch};

impl GitWindow {
    pub(super) fn render_branch_list(
        &mut self,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focus = self.branch_focus.clone();
        let list_state = match &self.session.branch.list_state {
            BranchListState::Idle | BranchListState::Loading => {
                ListState::Loading(i18n::text("git.loading").into())
            }
            BranchListState::Error(error) => ListState::Error(error.clone().into()),
            BranchListState::Ready if self.session.branch.entries.is_empty() => {
                ListState::Empty(i18n::text("git.no_branches").into())
            }
            BranchListState::Ready => ListState::Ready,
        };
        let body = list_state_body(list_state, || {
            let entries = self.session.branch.entries.clone();
            let count = entries.len();
            uniform_list(
                if compact {
                    "git-branches-compact"
                } else {
                    "git-branches"
                },
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                    entries[range]
                        .iter()
                        .map(|entry| this.render_branch_row(entry, cx))
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.branch_scroll)
            .flex_1()
            .min_h_0()
            .with_sizing_behavior(ListSizingBehavior::Auto)
            .into_any_element()
        });

        let mut pane = list_pane(
            if compact {
                "git-branches-compact"
            } else {
                "git-branches-pane"
            },
            focus,
            GIT_BRANCH_CONTEXT,
        )
        .on_action(cx.listener(|this, _: &MoveBranchUp, _window, cx| {
            this.move_branch_selection(-1, cx);
        }))
        .on_action(cx.listener(|this, _: &MoveBranchDown, _window, cx| {
            this.move_branch_selection(1, cx);
        }))
        .on_action(cx.listener(|this, _: &SwitchSelectedBranch, _window, cx| {
            this.switch_selected_branch(cx);
        }))
        .child(self.render_branch_toolbar(cx));
        if let OperationState::Error(message) = &self.session.operation {
            pane = pane.child(pane_operation_error(message.clone().into()));
        }
        pane.child(body).into_any_element()
    }

    fn render_branch_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .h(px(38.))
            .flex_shrink_0()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .child(icons::icon(icons::IconName::GitBranch, 14.).text_color(theme::accent()))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text())
                    .child(SharedString::from(i18n::text("git.branches"))),
            )
            .child(div().flex_1())
            .child(
                Button::new("git-branches-refresh")
                    .size(ButtonSize::Icon(px(26.)))
                    .variant(ButtonVariant::Ghost)
                    .loading(self.session.branch.list_state.is_loading())
                    .tooltip(i18n::text("git.refresh"))
                    .icon(
                        icons::icon(icons::IconName::RefreshCw, 13.)
                            .text_color(theme::muted_text()),
                    )
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.refresh_branches(true, cx);
                    })),
            )
            .into_any_element()
    }

    fn render_branch_row(&self, branch: &BranchSummary, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.session.branch.selected.as_deref() == Some(branch.name.as_str());
        let current = branch.current;
        let name = branch.name.clone();
        let switch_name = branch.name.clone();
        let tracking = branch
            .upstream
            .clone()
            .map(|upstream| {
                if branch.upstream_gone {
                    format!("{} · {}", upstream, i18n::text("git.upstream_gone"))
                } else {
                    upstream
                }
            })
            .unwrap_or_else(|| i18n::text("git.no_upstream"));
        let mut row = selectable_row(
            SharedString::from(format!("git-branch-{}", branch.name)),
            selected,
            px(60.),
        )
        .px_3()
        .py_2()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::text())
                        .child(SharedString::from(branch.name.clone())),
                )
                .when(current, |line| {
                    line.child(
                        Badge::new(i18n::text("git.current_branch")).tone(BadgeTone::Success),
                    )
                })
                .when(branch.ahead > 0, |line| {
                    line.child(branch_status_badge(format!("↑{}", branch.ahead)))
                })
                .when(branch.behind > 0, |line| {
                    line.child(branch_status_badge(format!("↓{}", branch.behind)))
                }),
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
                        .flex_shrink_0()
                        .truncate()
                        .child(SharedString::from(tracking)),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_color(theme::muted_text())
                        .child(SharedString::from(branch.subject.clone())),
                )
                .when(!current, |line| {
                    line.child(
                        Button::new(SharedString::from(format!(
                            "git-switch-branch-{}",
                            branch.name
                        )))
                        .size(ButtonSize::Small)
                        .variant(ButtonVariant::Secondary)
                        .disabled(matches!(self.session.operation, OperationState::Running))
                        .label(i18n::text("git.switch_branch"))
                        .icon(
                            icons::icon(icons::IconName::GitBranch, 12.).text_color(theme::text()),
                        )
                        .on_click(cx.listener(
                            move |this, _event, _window, cx| {
                                this.switch_branch(switch_name.clone(), cx);
                                cx.stop_propagation();
                            },
                        )),
                    )
                }),
        )
        .on_click(cx.listener(move |this, _event, _window, cx| {
            this.select_branch(name.clone(), cx);
        }));
        if current {
            row = row.cursor_default();
        }
        row.into_any_element()
    }
}

fn branch_status_badge(text: String) -> AnyElement {
    Badge::new(text).tone(BadgeTone::Info).into_any_element()
}

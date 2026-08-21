//! Git Stash 页面渲染。

use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ListSizingBehavior,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, div, px, uniform_list,
};

use crate::shared::i18n;
use crossh_core::git_stash::StashSummary;
use crossh_ui::{icons, theme};
use crossh_ui_component::{
    Banner, BannerTone, Button, ButtonSize, ButtonVariant, ListState, list_empty, list_pane,
    selectable_row,
};

use super::session::OperationState;
use super::stash::StashListState;
use super::window::GitWindow;
use super::{ApplySelectedStash, GIT_STASH_CONTEXT, MoveStashDown, MoveStashUp};

impl GitWindow {
    pub(super) fn render_stash_list(
        &mut self,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focus = self.stash_focus.clone();
        let body = match &self.session.stash.list_state {
            StashListState::Idle | StashListState::Loading => {
                list_empty(ListState::Loading(i18n::text("git.loading").into()))
            }
            StashListState::Error(error) => list_empty(ListState::Error(error.clone().into())),
            StashListState::Ready if self.session.stash.entries.is_empty() => {
                list_empty(ListState::Empty(i18n::text("git.no_stashes").into()))
            }
            StashListState::Ready => {
                let entries = self.session.stash.entries.clone();
                let count = entries.len();
                uniform_list(
                    if compact {
                        "git-stashes-compact"
                    } else {
                        "git-stashes"
                    },
                    count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                        entries[range]
                            .iter()
                            .map(|entry| this.render_stash_row(entry, compact, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.stash_scroll)
                .flex_1()
                .min_h_0()
                .with_sizing_behavior(ListSizingBehavior::Auto)
                .into_any_element()
            }
        };

        let mut pane = list_pane(
            if compact {
                "git-stashes-compact"
            } else {
                "git-stashes-pane"
            },
            focus,
            GIT_STASH_CONTEXT,
        )
        .on_action(cx.listener(|this, _: &MoveStashUp, _window, cx| {
            this.move_stash_selection(-1, cx);
        }))
        .on_action(cx.listener(|this, _: &MoveStashDown, _window, cx| {
            this.move_stash_selection(1, cx);
        }))
        .on_action(cx.listener(|this, _: &ApplySelectedStash, _window, cx| {
            this.apply_selected_stash(cx);
        }))
        .child(self.render_stash_toolbar(cx));

        if let Some(selector) = &self.pending_stash_drop {
            pane = pane.child(self.render_stash_drop_confirmation(selector, cx));
        }
        if let OperationState::Error(message) = &self.session.operation {
            pane = pane.child(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme::danger())
                    .text_xs()
                    .text_color(theme::danger())
                    .child(SharedString::from(message.clone())),
            );
        }
        pane.child(body).into_any_element()
    }

    fn render_stash_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
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
            .child(icons::icon(icons::IconName::Save, 14.).text_color(theme::accent()))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text())
                    .child(SharedString::from(i18n::text("git.stashes"))),
            )
            .child(div().flex_1())
            .child(
                Button::new("git-stash-save")
                    .size(ButtonSize::Small)
                    .variant(ButtonVariant::Secondary)
                    .disabled(matches!(self.session.operation, OperationState::Running))
                    .label(i18n::text("git.stash_changes"))
                    .icon(icons::icon(icons::IconName::Save, 12.).text_color(theme::text()))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.stash_changes(cx);
                    })),
            )
            .child(
                Button::new("git-stashes-refresh")
                    .size(ButtonSize::Icon(px(26.)))
                    .variant(ButtonVariant::Ghost)
                    .loading(self.session.stash.list_state.is_loading())
                    .tooltip(i18n::text("git.refresh"))
                    .icon(
                        icons::icon(icons::IconName::RefreshCw, 13.)
                            .text_color(theme::muted_text()),
                    )
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.refresh_stashes(true, cx);
                    })),
            )
            .into_any_element()
    }

    fn render_stash_drop_confirmation(&self, selector: &str, cx: &mut Context<Self>) -> AnyElement {
        Banner::new("git-stash-drop-confirmation")
            .tone(BannerTone::Danger)
            .icon(icons::icon(icons::IconName::Trash, 14.).text_color(theme::danger()))
            .title(format!(
                "{}: {selector}",
                i18n::text("git.drop_stash_confirm")
            ))
            .action(
                Button::new("git-stash-drop-cancel")
                    .size(ButtonSize::Small)
                    .variant(ButtonVariant::Ghost)
                    .label(i18n::text("git.drop_stash_cancel"))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.cancel_drop_stash(cx);
                    })),
            )
            .action(
                Button::new("git-stash-drop-confirm")
                    .size(ButtonSize::Small)
                    .variant(ButtonVariant::Danger)
                    .label(i18n::text("git.drop_stash_action"))
                    .icon(icons::icon(icons::IconName::Trash, 12.).text_color(theme::canvas()))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.confirm_drop_stash(cx);
                    })),
            )
            .into_any_element()
    }

    fn render_stash_row(
        &self,
        stash: &StashSummary,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.session.stash.selected.as_deref() == Some(stash.selector.as_str());
        let selector = stash.selector.clone();
        let apply_selector = stash.selector.clone();
        let pop_selector = stash.selector.clone();
        let drop_selector = stash.selector.clone();
        let row = selectable_row(
            SharedString::from(format!("git-stash-{}", stash.selector)),
            selected,
            if compact { px(106.) } else { px(78.) },
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
                        .flex_shrink_0()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::accent())
                        .child(SharedString::from(stash.selector.clone())),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::text())
                        .child(SharedString::from(stash.message.clone())),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(theme::faint_text())
                        .child(SharedString::from(stash.date.clone())),
                ),
        )
        .child(
            div()
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .flex_wrap()
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_xs()
                        .text_color(theme::faint_text())
                        .child(SharedString::from(stash.id.clone())),
                )
                .child(
                    Button::new(SharedString::from(format!(
                        "git-stash-apply-{}",
                        stash.selector
                    )))
                    .size(ButtonSize::Small)
                    .variant(ButtonVariant::Secondary)
                    .disabled(matches!(self.session.operation, OperationState::Running))
                    .label(i18n::text("git.apply_stash"))
                    .icon(icons::icon(icons::IconName::Download, 12.).text_color(theme::text()))
                    .on_click(cx.listener(
                        move |this, _event, _window, cx| {
                            this.apply_stash(apply_selector.clone(), cx);
                            cx.stop_propagation();
                        },
                    )),
                )
                .child(
                    Button::new(SharedString::from(format!(
                        "git-stash-pop-{}",
                        stash.selector
                    )))
                    .size(ButtonSize::Small)
                    .variant(ButtonVariant::Ghost)
                    .disabled(matches!(self.session.operation, OperationState::Running))
                    .label(i18n::text("git.pop_stash"))
                    .icon(icons::icon(icons::IconName::Upload, 12.).text_color(theme::muted_text()))
                    .on_click(cx.listener(
                        move |this, _event, _window, cx| {
                            this.pop_stash(pop_selector.clone(), cx);
                            cx.stop_propagation();
                        },
                    )),
                )
                .child(
                    Button::new(SharedString::from(format!(
                        "git-stash-drop-{}",
                        stash.selector
                    )))
                    .size(ButtonSize::Icon(px(26.)))
                    .variant(ButtonVariant::Ghost)
                    .disabled(matches!(self.session.operation, OperationState::Running))
                    .tooltip(i18n::text("git.drop_stash"))
                    .icon(icons::icon(icons::IconName::Trash, 12.).text_color(theme::danger()))
                    .on_click(cx.listener(
                        move |this, _event, _window, cx| {
                            this.request_drop_stash(drop_selector.clone(), cx);
                            cx.stop_propagation();
                        },
                    )),
                ),
        )
        .on_click(cx.listener(move |this, _event, _window, cx| {
            this.select_stash(selector.clone(), cx);
        }));
        row.into_any_element()
    }
}

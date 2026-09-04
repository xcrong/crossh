//! Git Remote 页面渲染。

use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ListSizingBehavior,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder,
    px, uniform_list,
};

use crate::shared::i18n;
use crossh_core::git_remote::RemoteSummary;
use crossh_ui::{icons, theme};
use crossh_ui_component::{
    Banner, BannerTone, Button, ButtonSize, ButtonVariant, ListStatus, PaneToolbar, TextInput,
    labeled_field, list_pane, list_state_body, pane_operation_error, selectable_row,
};

use super::remote::RemoteListState;
use super::session::OperationState;
use super::window::GitWindow;
use super::{FetchSelectedRemote, GIT_REMOTE_CONTEXT, MoveRemoteDown, MoveRemoteUp};

impl GitWindow {
    pub(super) fn render_remote_list(
        &mut self,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focus = self.remote_focus.clone();
        let list_state = match &self.session.remote.list_state {
            RemoteListState::Idle | RemoteListState::Loading => {
                ListStatus::Loading(i18n::text("git.loading").into())
            }
            RemoteListState::Error(error) => ListStatus::Error(error.clone().into()),
            RemoteListState::Ready if self.session.remote.entries.is_empty() => {
                ListStatus::Empty(i18n::text("git.no_remotes").into())
            }
            RemoteListState::Ready => ListStatus::Ready,
        };
        let body = list_state_body(list_state, || {
            let entries = self.session.remote.entries.clone();
            let count = entries.len();
            uniform_list(
                if compact {
                    "git-remotes-compact"
                } else {
                    "git-remotes"
                },
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                    entries[range]
                        .iter()
                        .map(|entry| this.render_remote_row(entry, compact, cx))
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.remote_scroll)
            .flex_1()
            .min_h_0()
            .with_sizing_behavior(ListSizingBehavior::Auto)
            .into_any_element()
        });

        let mut pane = list_pane(
            if compact {
                "git-remotes-compact"
            } else {
                "git-remotes-pane"
            },
            focus,
            GIT_REMOTE_CONTEXT,
        )
        .on_action(cx.listener(|this, _: &MoveRemoteUp, _window, cx| {
            this.move_remote_selection(-1, cx);
        }))
        .on_action(cx.listener(|this, _: &MoveRemoteDown, _window, cx| {
            this.move_remote_selection(1, cx);
        }))
        .on_action(cx.listener(|this, _: &FetchSelectedRemote, _window, cx| {
            this.fetch_selected_remote(cx);
        }))
        .child(self.render_remote_toolbar(cx));
        if self.remote_add_open {
            pane = pane.child(self.render_remote_add_form(cx));
        }
        if let Some(name) = self.pending_remote_remove.clone() {
            pane = pane.child(self.render_remote_remove_confirmation(&name, cx));
        }
        if let OperationState::Error(message) = &self.session.operation {
            pane = pane.child(pane_operation_error(message.clone().into()));
        }
        pane.child(body).into_any_element()
    }

    fn render_remote_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        PaneToolbar::new(i18n::text("git.remotes"), icons::IconName::Server)
            .child(
                Button::new("git-remotes-add")
                    .size(ButtonSize::Icon(px(26.)))
                    .variant(ButtonVariant::Ghost)
                    .disabled(matches!(self.session.operation, OperationState::Running))
                    .tooltip(i18n::text("git.add_remote"))
                    .icon(icons::icon(icons::IconName::Plus, 13.).text_color(theme::muted_text()))
                    .on_click(cx.listener(|this, _event, window, cx| {
                        this.open_remote_add(window, cx);
                    })),
            )
            .child(
                Button::new("git-remotes-fetch-all")
                    .size(ButtonSize::Small)
                    .variant(ButtonVariant::Secondary)
                    .disabled(
                        self.session.remote.entries.is_empty()
                            || matches!(self.session.operation, OperationState::Running),
                    )
                    .label(i18n::text("git.fetch_all"))
                    .icon(icons::icon(icons::IconName::Download, 12.).text_color(theme::text()))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.fetch_all_remotes(cx);
                    })),
            )
            .child(
                Button::new("git-remotes-refresh")
                    .size(ButtonSize::Icon(px(26.)))
                    .variant(ButtonVariant::Ghost)
                    .loading(self.session.remote.list_state.is_loading())
                    .tooltip(i18n::text("git.refresh"))
                    .icon(
                        icons::icon(icons::IconName::RefreshCw, 13.)
                            .text_color(theme::muted_text()),
                    )
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.refresh_remotes(true, cx);
                    })),
            )
            .into_any_element()
    }

    fn render_remote_add_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let name_focus = self.remote_add_name_focus.clone();
        let url_focus = self.remote_add_url_focus.clone();
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .child(labeled_field(
                "git-remote-add-name-row",
                i18n::text("git.remote_name"),
                name_focus,
                TextInput::new("git-remote-add-name", self.remote_add_name_focus.clone())
                    .value(self.remote_add_name.value.clone())
                    .placeholder(i18n::text("git.remote_name_placeholder"))
                    .ime_marked_text(self.remote_add_name.ime_marked_text.clone())
                    .text_color(if self.remote_add_name.value.is_empty() {
                        theme::faint_text()
                    } else {
                        theme::text()
                    })
                    .bg(theme::canvas())
                    .flex_1()
                    .entity(cx.entity())
                    .on_key_down(cx.listener(Self::handle_remote_add_name_key)),
            ))
            .child(labeled_field(
                "git-remote-add-url-row",
                i18n::text("git.remote_url"),
                url_focus,
                TextInput::new("git-remote-add-url", self.remote_add_url_focus.clone())
                    .value(self.remote_add_url.value.clone())
                    .placeholder(i18n::text("git.remote_url_placeholder"))
                    .ime_marked_text(self.remote_add_url.ime_marked_text.clone())
                    .text_color(if self.remote_add_url.value.is_empty() {
                        theme::faint_text()
                    } else {
                        theme::text()
                    })
                    .bg(theme::canvas())
                    .flex_1()
                    .entity(cx.entity())
                    .on_key_down(cx.listener(Self::handle_remote_add_url_key)),
            ))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("git-remote-add-cancel")
                            .size(ButtonSize::Small)
                            .variant(ButtonVariant::Ghost)
                            .label(i18n::text("git.remote_cancel"))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.close_remote_add(cx);
                            })),
                    )
                    .child(
                        Button::new("git-remote-add-confirm")
                            .size(ButtonSize::Small)
                            .variant(ButtonVariant::Secondary)
                            .disabled(!self.can_submit_remote_add())
                            .label(i18n::text("git.add_remote"))
                            .icon(icons::icon(icons::IconName::Plus, 12.).text_color(theme::text()))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.submit_remote_add(cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_remote_remove_confirmation(&self, name: &str, cx: &mut Context<Self>) -> AnyElement {
        Banner::new("git-remote-remove-confirmation")
            .tone(BannerTone::Danger)
            .icon(icons::icon(icons::IconName::Trash, 14.).text_color(theme::danger()))
            .title(format!(
                "{}: {name}",
                i18n::text("git.remote_remove_confirm")
            ))
            .action(
                Button::new("git-remote-remove-cancel")
                    .size(ButtonSize::Small)
                    .variant(ButtonVariant::Ghost)
                    .label(i18n::text("git.remote_cancel"))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.cancel_remove_remote(cx);
                    })),
            )
            .action(
                Button::new("git-remote-remove-confirm")
                    .size(ButtonSize::Small)
                    .variant(ButtonVariant::Danger)
                    .label(i18n::text("git.remove_remote"))
                    .icon(icons::icon(icons::IconName::Trash, 12.).text_color(theme::canvas()))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.confirm_remove_remote(cx);
                    })),
            )
            .into_any_element()
    }

    fn render_remote_row(
        &self,
        remote: &RemoteSummary,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.session.remote.selected.as_deref() == Some(remote.name.as_str());
        let name = remote.name.clone();
        let fetch_name = remote.name.clone();
        let remove_name = remote.name.clone();
        let fetch_url = remote.fetch_url.clone();
        let push_url = remote.push_url.clone();
        let push_distinct = match (&fetch_url, &push_url) {
            (Some(fetch), Some(push)) => fetch != push,
            (None, Some(_)) | (Some(_), None) => true,
            (None, None) => false,
        };
        let row = selectable_row(
            SharedString::from(format!("git-remote-{}", remote.name)),
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
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::text())
                        .child(SharedString::from(remote.name.clone())),
                )
                .child(
                    Button::new(SharedString::from(format!(
                        "git-fetch-remote-{}",
                        remote.name
                    )))
                    .size(ButtonSize::Small)
                    .variant(ButtonVariant::Secondary)
                    .disabled(matches!(self.session.operation, OperationState::Running))
                    .label(i18n::text("git.fetch"))
                    .on_click(cx.listener(
                        move |this, _event, _window, cx| {
                            this.fetch_remote(fetch_name.clone(), cx);
                            cx.stop_propagation();
                        },
                    )),
                )
                .child(
                    Button::new(SharedString::from(format!(
                        "git-remove-remote-{}",
                        remote.name
                    )))
                    .size(ButtonSize::Icon(px(26.)))
                    .variant(ButtonVariant::Ghost)
                    .disabled(matches!(self.session.operation, OperationState::Running))
                    .tooltip(i18n::text("git.remove_remote"))
                    .icon(icons::icon(icons::IconName::Trash, 12.).text_color(theme::danger()))
                    .on_click(cx.listener(
                        move |this, _event, _window, cx| {
                            this.request_remove_remote(remove_name.clone(), cx);
                            cx.stop_propagation();
                        },
                    )),
                ),
        )
        .child(
            div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .text_xs()
                .text_color(theme::faint_text())
                .when_some(fetch_url, |column, url| {
                    column.child(
                        div()
                            .w_full()
                            .truncate()
                            .child(SharedString::from(format!("↓ {url}"))),
                    )
                })
                .when(push_distinct, |column| {
                    column.when_some(push_url, |column, url| {
                        column.child(
                            div()
                                .w_full()
                                .truncate()
                                .child(SharedString::from(format!("↑ {url}"))),
                        )
                    })
                }),
        )
        .on_click(cx.listener(move |this, _event, _window, cx| {
            this.select_remote(name.clone(), cx);
        }));
        row.into_any_element()
    }
}

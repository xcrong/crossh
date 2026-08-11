//! Git 窗口布局与视觉渲染。

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

use gpui::{
    AnyElement, Bounds, Context, FontWeight, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, canvas, div, prelude::FluentBuilder, px,
};

use crate::shared::i18n;
use crossh_core::git::{ChangeStatus, DiffLine, DiffLineKind, FileChange};
use crossh_ui::widgets::{ime_input_canvas, text_caret};
use crossh_ui::{icons, theme};
use crossh_ui_component::{Badge, BadgeTone, Button, ButtonSize, ButtonVariant};

use super::editor::CommitEditor;
use super::model::{
    ChangeKey, CompactPage, DiffState, OperationState, clamp_changes_pane_width,
    uses_compact_git_layout,
};
use super::window::GitWindow;
use super::{
    BackToChanges, CommitChanges, GIT_CHANGES_CONTEXT, GIT_WINDOW_CONTEXT, MoveSelectionDown,
    MoveSelectionUp, RefreshChanges, ToggleSelectedStage,
};

impl Render for GitWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.compact_layout = uses_compact_git_layout(window.viewport_size().width);
        let body = if self.compact_layout {
            match self.compact_page {
                CompactPage::Changes => self.render_changes_pane(true, window, cx),
                CompactPage::Diff => self.render_diff_pane(true, cx),
            }
        } else {
            self.render_standard_body(window, cx)
        };

        div()
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
                this.refresh_list(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &BackToChanges, _window, cx| {
                this.back_to_changes(cx);
            }))
            .child(self.render_header(cx))
            .child(body)
    }
}

impl GitWindow {
    fn render_standard_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let width = clamp_changes_pane_width(self.changes_pane_width.get());
        let container: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
        let backing = canvas(
            {
                let container = container.clone();
                move |bounds, _window, _cx| container.set(Some(bounds))
            },
            {
                let container = container.clone();
                let width_cell = self.changes_pane_width.clone();
                let dragging = self.changes_pane_dragging.clone();
                move |_bounds, _state, window, _cx| {
                    window.on_mouse_event({
                        let container = container.clone();
                        let width_cell = width_cell.clone();
                        let dragging = dragging.clone();
                        move |event: &MouseMoveEvent, _phase, window, _cx| {
                            if !dragging.get() {
                                return;
                            }
                            let Some(bounds) = container.get() else {
                                return;
                            };
                            width_cell.set(clamp_changes_pane_width(
                                (event.position.x - bounds.origin.x).as_f32(),
                            ));
                            window.refresh();
                        }
                    });
                    window.on_mouse_event({
                        let dragging = dragging.clone();
                        move |_event: &MouseUpEvent, _phase, window, _cx| {
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

        let resize_handle = div()
            .id("git-changes-resize")
            .absolute()
            .top_0()
            .right(px(-4.))
            .w(px(8.))
            .h_full()
            .cursor_col_resize()
            .on_mouse_down(MouseButton::Left, {
                let dragging = self.changes_pane_dragging.clone();
                move |_event: &MouseDownEvent, window, _cx| {
                    dragging.set(true);
                    window.refresh();
                }
            });

        div()
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .child(backing)
            .child(
                div()
                    .relative()
                    .w(px(width))
                    .flex_shrink_0()
                    .border_r_1()
                    .border_color(theme::border_strong())
                    .child(self.render_changes_pane(false, window, cx))
                    .child(resize_handle),
            )
            .child(self.render_diff_pane(false, cx))
            .into_any_element()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let staged_count = self.staged_count();
        let working_count = self.changes.len().saturating_sub(staged_count);
        let mut branch = div()
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
                        self.status
                            .as_ref()
                            .map(|status| status.branch.clone())
                            .unwrap_or_else(|| self.label.clone()),
                    )),
            );
        if let Some(status) = &self.status {
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
                Button::new("git-refresh")
                    .size(ButtonSize::Icon(px(30.)))
                    .variant(ButtonVariant::Ghost)
                    .loading(self.refreshing)
                    .tooltip(i18n::text("git.refresh"))
                    .icon(
                        icons::icon(icons::IconName::RefreshCw, 14.)
                            .text_color(theme::muted_text()),
                    )
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.refresh_list(cx);
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
        let staged = self
            .changes
            .iter()
            .filter(|change| change.staged)
            .cloned()
            .collect::<Vec<_>>();
        let working = self
            .changes
            .iter()
            .filter(|change| !change.staged)
            .cloned()
            .collect::<Vec<_>>();
        let focus = self.changes_focus.clone();

        let mut content = div().flex().flex_col().pb_2();
        if self.initial_loading {
            content = content.child(sidebar_empty_hint(&i18n::text("git.loading")));
        } else if self.changes.is_empty() {
            content = content.child(sidebar_empty_hint(&i18n::text("git.no_changes")));
        } else {
            content = content.child(self.render_section(true, &staged, self.staged_collapsed, cx));
            content =
                content.child(self.render_section(false, &working, self.working_collapsed, cx));
        }

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
            .child(self.render_commit_panel(compact, window, cx))
            .child(
                div()
                    .id(if compact {
                        "git-changes-list-compact"
                    } else {
                        "git-changes-list"
                    })
                    .track_scroll(&self.changes_scroll)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(content),
            )
            .into_any_element()
    }

    fn render_section(
        &self,
        staged: bool,
        entries: &[FileChange],
        collapsed: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = if staged {
            i18n::text("git.staged_changes")
        } else {
            i18n::text("git.changes")
        };
        let paths = entries
            .iter()
            .map(|change| change.path.clone())
            .collect::<Vec<_>>();
        let mut section = div().flex().flex_col().child(
            div()
                .id(if staged {
                    "git-section-staged"
                } else {
                    "git-section-working"
                })
                .h(px(32.))
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
                        .child(SharedString::from(entries.len().to_string())),
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
                    .disabled(entries.is_empty())
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
                    .on_click(cx.listener(
                        move |this, _event, _window, cx| {
                            if staged {
                                this.unstage_paths(paths.clone(), cx);
                            } else {
                                this.stage_paths(paths.clone(), cx);
                            }
                            cx.stop_propagation();
                        },
                    )),
                )
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    if staged {
                        this.staged_collapsed = !this.staged_collapsed;
                    } else {
                        this.working_collapsed = !this.working_collapsed;
                    }
                    cx.notify();
                })),
        );
        if !collapsed {
            for entry in entries {
                section = section.child(self.render_change_row(entry, cx));
            }
        }
        section.into_any_element()
    }

    fn render_change_row(&self, entry: &FileChange, cx: &mut Context<Self>) -> AnyElement {
        let key = ChangeKey::from(entry);
        let selected = self.selected.as_ref() == Some(&key);
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
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.select(key.clone(), cx);
            }))
            .into_any_element()
    }

    fn render_diff_pane(&self, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let Some(entry) = self.selected_entry() else {
            return div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .justify_center()
                .child(empty_hint(&i18n::text("git.no_selection")))
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

        let body = match &self.diff {
            DiffState::Idle => centered_hint(&i18n::text("git.no_selection")),
            DiffState::Loading(loading_key) if loading_key == &key => {
                centered_hint(&i18n::text("git.loading"))
            }
            DiffState::Ready(ready_key, Some(file_diff))
                if ready_key == &key && file_diff.binary =>
            {
                centered_hint(&i18n::text("git.binary"))
            }
            DiffState::Ready(ready_key, Some(file_diff))
                if ready_key == &key && file_diff.lines.is_empty() =>
            {
                centered_hint(&i18n::text("git.no_diff"))
            }
            DiffState::Ready(ready_key, Some(file_diff)) if ready_key == &key => {
                let mut lines = div().min_w_full().flex().flex_col().font_family("Zed Mono");
                for line in &file_diff.lines {
                    lines = lines.child(render_diff_line(line));
                }
                div()
                    .id(if compact {
                        "git-diff-scroll-compact"
                    } else {
                        "git-diff-scroll"
                    })
                    .track_scroll(&self.diff_scroll)
                    .flex_1()
                    .min_w_0()
                    .overflow_scroll()
                    .child(lines)
                    .into_any_element()
            }
            DiffState::Ready(ready_key, None) if ready_key == &key => {
                centered_hint(&i18n::text("git.no_diff"))
            }
            _ => centered_hint(&i18n::text("git.loading")),
        };

        div()
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

    fn render_commit_panel(
        &mut self,
        compact: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focused = self.commit_editor.focus.is_focused(window);
        let focus = self.commit_editor.focus.clone();
        let staged_count = self.staged_count();
        let running = matches!(self.operation, OperationState::Running);
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
        if let OperationState::Error(message) = &self.operation {
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
    if editor.value.is_empty() {
        let mut row = div().h(px(18.)).flex().items_center();
        if focused {
            row = row.child(text_caret(px(15.)));
        }
        if editor.ime_marked_text.is_empty() {
            row = row.child(
                div()
                    .text_color(theme::faint_text())
                    .child(SharedString::from(i18n::text(
                        "git.commit_message_placeholder",
                    ))),
            );
        } else {
            row = row.child(marked_text(&editor.ime_marked_text));
        }
        return row.into_any_element();
    }

    let selection = editor.selection();
    let mut content = div().flex().flex_col();
    let mut line_start = 0;
    for line in editor.value.split('\n') {
        let line_end = line_start + line.len();
        let mut row = div().h(px(18.)).flex().items_center();
        if let Some((selection_start, selection_end)) = selection {
            let selected_start = selection_start.clamp(line_start, line_end);
            let selected_end = selection_end.clamp(line_start, line_end);
            row = row
                .child(text_part(&editor.value[line_start..selected_start]))
                .when(selected_start < selected_end, |row| {
                    row.child(div().flex_shrink_0().bg(theme::accent_soft()).child(
                        SharedString::from(editor.value[selected_start..selected_end].to_string()),
                    ))
                })
                .child(text_part(&editor.value[selected_end..line_end]));
        } else if (line_start..=line_end).contains(&editor.cursor) {
            row = row.child(text_part(&editor.value[line_start..editor.cursor]));
            if focused {
                row = row.child(text_caret(px(15.)));
            }
            if !editor.ime_marked_text.is_empty() {
                row = row.child(marked_text(&editor.ime_marked_text));
            }
            row = row.child(text_part(&editor.value[editor.cursor..line_end]));
        } else {
            row = row.child(text_part(line));
        }
        content = content.child(row);
        line_start = line_end + 1;
    }
    content.into_any_element()
}

fn text_part(text: &str) -> AnyElement {
    div()
        .flex_shrink_0()
        .whitespace_nowrap()
        .child(SharedString::from(text.to_string()))
        .into_any_element()
}

fn marked_text(text: &str) -> AnyElement {
    div()
        .flex_shrink_0()
        .whitespace_nowrap()
        .underline()
        .text_decoration_color(theme::accent())
        .child(SharedString::from(text.to_string()))
        .into_any_element()
}

fn render_diff_line(line: &DiffLine) -> AnyElement {
    let (background, foreground, rail) = match line.kind {
        DiffLineKind::Hunk => (theme::surface(), theme::muted_text(), theme::info()),
        DiffLineKind::Added => (theme::diff_add_bg(), theme::diff_add_fg(), theme::accent()),
        DiffLineKind::Removed => (theme::diff_del_bg(), theme::diff_del_fg(), theme::danger()),
        DiffLineKind::Context => (theme::canvas(), theme::text(), theme::canvas()),
    };
    if line.kind == DiffLineKind::Hunk {
        return div()
            .min_w_full()
            .flex()
            .bg(background)
            .child(div().w(px(2.)).flex_shrink_0().bg(rail))
            .child(
                div()
                    .px_2()
                    .py(px(3.))
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(foreground)
                    .child(SharedString::from(line.text.clone())),
            )
            .into_any_element();
    }
    let number = |value: Option<u32>| value.map(|n| n.to_string()).unwrap_or_default();
    div()
        .min_w_full()
        .flex()
        .bg(background)
        .child(div().w(px(2.)).flex_shrink_0().bg(rail))
        .child(line_number(number(line.old_ln), foreground))
        .child(line_number(number(line.new_ln), foreground))
        .child(
            div()
                .flex_shrink_0()
                .px_2()
                .py(px(1.))
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

fn empty_hint(text: &str) -> AnyElement {
    div()
        .p_4()
        .text_xs()
        .text_color(theme::faint_text())
        .child(SharedString::from(text.to_string()))
        .into_any_element()
}

fn sidebar_empty_hint(text: &str) -> AnyElement {
    div()
        .px_2()
        .py_4()
        .text_xs()
        .text_color(theme::faint_text())
        .child(SharedString::from(text.to_string()))
        .into_any_element()
}

fn centered_hint(text: &str) -> AnyElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .child(empty_hint(text))
        .into_any_element()
}

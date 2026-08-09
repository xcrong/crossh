//! Git 可视化窗口：独立的 gpui 窗口（VS Code 源码管理风格）。
//!
//! 窗口自持目录与拉取的数据（`logic.rs` 纯逻辑），不依赖主窗口；
//! 由状态栏 Git 指示条点击打开。

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, App, AppContext, Bounds, Context, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, Size, StatefulInteractiveElement, Styled, Task,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, px,
};

use crate::shared::i18n;
use crossh_core::git::{ChangeStatus, DiffLineKind, FileChange, diff, list_changes};
use crossh_core::project::GitStatus;
use crossh_ui::widgets::LocalPathTooltip;
use crossh_ui::{icons, theme};

/// 窗口自身的数据刷新间隔。
const REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Git 窗口的根视图。窗口关闭即释放。
pub struct GitWindow {
    cwd: PathBuf,
    label: String,
    /// 全部改动（已暂存 + 未暂存）。
    changes: Vec<FileChange>,
    status: Option<GitStatus>,
    /// true = 右侧展示已暂存（相对 HEAD）的 diff，否则展示工作区（相对索引）。
    show_staged: bool,
    selected: Option<usize>,
    diff: Option<Option<crossh_core::git::FileDiff>>,
    loading: bool,
    list_generation: u64,
    diff_generation: u64,
    _refresh_task: Option<Task<()>>,
    changes_scroll: gpui::ScrollHandle,
    diff_scroll: gpui::ScrollHandle,
}

impl GitWindow {
    fn new(cwd: PathBuf, cx: &mut Context<Self>) -> Self {
        let label = cwd
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| cwd.to_string_lossy().into_owned());
        let mut window = Self {
            cwd,
            label,
            changes: Vec::new(),
            status: None,
            show_staged: false,
            selected: None,
            diff: None,
            loading: true,
            list_generation: 0,
            diff_generation: 0,
            _refresh_task: None,
            changes_scroll: gpui::ScrollHandle::new(),
            diff_scroll: gpui::ScrollHandle::new(),
        };
        window.refresh_list(cx);
        window.ensure_refresh_loop(cx);
        window
    }

    /// 重新拉取文件列表与汇总状态。
    fn refresh_list(&mut self, cx: &mut Context<Self>) {
        self.list_generation = self.list_generation.wrapping_add(1);
        let generation = self.list_generation;
        let cwd = self.cwd.clone();
        self.loading = true;

        cx.spawn(async move |weak, cx| {
            let (changes, status) = cx
                .background_executor()
                .spawn(async move { (list_changes(&cwd), inspect_status(&cwd)) })
                .await;
            let _ = weak.update(cx, |this, cx| {
                if this.list_generation != generation {
                    return;
                }
                this.changes = changes;
                this.status = status;
                this.loading = false;
                if this.selected.is_none() && !this.changes.is_empty() {
                    this.selected = Some(0);
                }
                this.refresh_diff(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// 重新拉取选中文件的 diff。
    fn refresh_diff(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.selected else {
            self.diff = None;
            return;
        };
        let Some(entry) = self.changes.get(index).cloned() else {
            return;
        };
        let cwd = self.cwd.clone();
        let staged = self.show_staged;
        self.diff_generation = self.diff_generation.wrapping_add(1);
        let generation = self.diff_generation;

        cx.spawn(async move |weak, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { diff(&cwd, &entry, staged) })
                .await;
            let _ = weak.update(cx, |this, cx| {
                if this.diff_generation != generation {
                    return;
                }
                this.diff = Some(result);
                cx.notify();
            });
        })
        .detach();
    }

    /// 周期刷新循环，保证窗口数据与工作区同步。
    fn ensure_refresh_loop(&mut self, cx: &mut Context<Self>) {
        if self._refresh_task.is_some() {
            return;
        }
        self._refresh_task = Some(cx.spawn(async move |weak, cx| {
            loop {
                cx.background_executor().timer(REFRESH_INTERVAL).await;
                let _ = weak.update(cx, |this, cx| this.refresh_list(cx));
            }
        }));
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.selected == Some(index) {
            return;
        }
        self.selected = Some(index);
        self.diff_scroll
            .set_offset(gpui::Point::new(px(0.), px(0.)));
        self.refresh_diff(cx);
        cx.notify();
    }

    fn set_show_staged(&mut self, staged: bool, cx: &mut Context<Self>) {
        if self.show_staged == staged {
            return;
        }
        self.show_staged = staged;
        self.diff_scroll
            .set_offset(gpui::Point::new(px(0.), px(0.)));
        self.refresh_diff(cx);
        cx.notify();
    }
}

fn inspect_status(cwd: &Path) -> Option<GitStatus> {
    crossh_core::project::inspect(cwd)
}

impl Render for GitWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let staged_count = self.changes.iter().filter(|entry| entry.staged).count();
        let working_count = self.changes.len() - staged_count;

        let (staged_entries, working_entries) = self.changes.iter().enumerate().fold(
            (Vec::new(), Vec::new()),
            |(mut staged, mut working), (index, entry)| {
                if entry.staged {
                    staged.push((index, entry));
                } else {
                    working.push((index, entry));
                }
                (staged, working)
            },
        );

        let mut list = div().flex().flex_col();
        if self.loading {
            list = list.child(empty_hint(&i18n::text("git.loading")));
        } else if self.changes.is_empty() {
            list = list.child(empty_hint(&i18n::text("git.no_changes")));
        } else {
            list = list
                .child(section_header(
                    &format!("{} ({})", i18n::text("git.staged_changes"), staged_count),
                    theme::accent(),
                ))
                .child(render_entries(&staged_entries, self.selected, cx))
                .child(section_header(
                    &format!("{} ({})", i18n::text("git.changes"), working_count),
                    theme::text(),
                ))
                .child(render_entries(&working_entries, self.selected, cx));
        }

        let right = self.render_diff_pane();

        div()
            .id("git-window")
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .child(self.render_header(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .w(px(300.))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .border_r_1()
                            .border_color(theme::border())
                            .child(
                                div()
                                    .id("git-changes-list")
                                    .track_scroll(&self.changes_scroll)
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .child(list),
                            ),
                    )
                    .child(right),
            )
    }
}

impl GitWindow {
    /// 顶部工具条：分支信息 + 暂存/工作区切换 + 刷新。
    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut branch_info = div()
            .min_w_0()
            .flex()
            .items_center()
            .gap_2()
            .child(icons::icon(icons::IconName::GitBranch, 15.).text_color(theme::accent()))
            .child(
                div()
                    .text_sm()
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
                branch_info =
                    branch_info.child(status_badge(format!("↑{}", status.ahead), theme::info()));
            }
            if status.behind > 0 {
                branch_info =
                    branch_info.child(status_badge(format!("↓{}", status.behind), theme::info()));
            }
        }

        let staged = self.show_staged;
        let toggle = div()
            .flex()
            .items_center()
            .rounded(px(theme::RADIUS_SM))
            .bg(theme::raised())
            .p(px(2.))
            .child(
                div()
                    .id("git-toggle-working")
                    .px_2()
                    .py(px(2.))
                    .text_xs()
                    .rounded(px(theme::RADIUS_SM - 1.))
                    .cursor_pointer()
                    .text_color(if staged {
                        theme::muted_text()
                    } else {
                        theme::text()
                    })
                    .bg(if staged {
                        theme::canvas()
                    } else {
                        theme::raised()
                    })
                    .child(SharedString::from(i18n::text("git.working")))
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.set_show_staged(false, cx);
                    })),
            )
            .child(
                div()
                    .id("git-toggle-staged")
                    .px_2()
                    .py(px(2.))
                    .text_xs()
                    .rounded(px(theme::RADIUS_SM - 1.))
                    .cursor_pointer()
                    .text_color(if staged {
                        theme::text()
                    } else {
                        theme::muted_text()
                    })
                    .bg(if staged {
                        theme::raised()
                    } else {
                        theme::canvas()
                    })
                    .child(SharedString::from(i18n::text("git.staged")))
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.set_show_staged(true, cx);
                    })),
            );

        div()
            .h(px(40.))
            .flex_shrink_0()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .child(branch_info)
            .child(div().flex_1())
            .child(toggle)
            .child(
                div()
                    .id("git-refresh")
                    .w(px(28.))
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(theme::RADIUS_SM))
                    .cursor_pointer()
                    .text_color(theme::muted_text())
                    .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
                    .tooltip(|_window, cx| {
                        cx.new(|_| LocalPathTooltip {
                            path: SharedString::from(i18n::text("git.refresh")),
                        })
                        .into()
                    })
                    .child(
                        icons::icon(icons::IconName::RefreshCw, 14.)
                            .text_color(theme::muted_text())
                            .hover(|s| s.text_color(theme::text())),
                    )
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.diff_scroll
                            .set_offset(gpui::Point::new(px(0.), px(0.)));
                        this.refresh_list(cx);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    /// 右侧 diff 面板。
    fn render_diff_pane(&self) -> AnyElement {
        let Some(index) = self.selected else {
            return div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .justify_center()
                .child(empty_hint(&i18n::text("git.no_selection")))
                .into_any_element();
        };
        let Some(entry) = self.changes.get(index) else {
            return div().flex_1().into_any_element();
        };

        let header = div()
            .h(px(34.))
            .flex_shrink_0()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .text_color(theme::text())
                    .child(SharedString::from(entry.path.clone())),
            )
            .child(status_glyph(entry.status))
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(theme::muted_text())
                    .child(SharedString::from(format!(
                        "+{} −{}",
                        entry.insertions, entry.deletions
                    ))),
            );

        let body = match &self.diff {
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(empty_hint(&i18n::text("git.loading")))
                .into_any_element(),
            Some(Some(file_diff)) if file_diff.binary => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(empty_hint(&i18n::text("git.binary")))
                .into_any_element(),
            Some(Some(file_diff)) if file_diff.lines.is_empty() => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(empty_hint(&i18n::text("git.no_diff")))
                .into_any_element(),
            Some(Some(file_diff)) => {
                let mut lines = div().min_w_0().flex().flex_col();
                for line in &file_diff.lines {
                    lines = lines.child(render_diff_line(line));
                }
                div()
                    .id("git-diff-scroll")
                    .track_scroll(&self.diff_scroll)
                    .flex_1()
                    .min_w_0()
                    .overflow_y_scroll()
                    .overflow_x_scroll()
                    .child(lines)
                    .into_any_element()
            }
            Some(None) => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(empty_hint(&i18n::text("git.no_diff")))
                .into_any_element(),
        };

        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }
}

/// 文件行列表。
fn render_entries(
    entries: &[(usize, &FileChange)],
    selected: Option<usize>,
    cx: &mut Context<GitWindow>,
) -> AnyElement {
    let mut list = div().flex().flex_col().py(px(2.));
    for (index, entry) in entries {
        let is_selected = selected == Some(*index);
        let row_index = *index;
        let row = div()
            .id(gpui::SharedString::from(format!("git-entry-{row_index}")))
            .w_full()
            .h(px(28.))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .text_xs()
            .text_color(if is_selected {
                theme::text()
            } else {
                theme::muted_text()
            })
            .bg(if is_selected {
                theme::surface()
            } else {
                theme::canvas()
            })
            .hover(|s| s.bg(theme::raised()))
            .child(status_glyph(entry.status))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_color(if is_selected {
                        theme::text()
                    } else {
                        theme::muted_text()
                    })
                    .child(SharedString::from(entry.path.clone())),
            );
        let row = if entry.insertions > 0 || entry.deletions > 0 {
            row.child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(theme::muted_text())
                    .child(SharedString::from(format!(
                        "+{} −{}",
                        entry.insertions, entry.deletions
                    ))),
            )
        } else {
            row
        };
        list = list.child(row.on_click(cx.listener(move |this, _ev, _window, cx| {
            this.select(row_index, cx);
        })));
    }
    list.into_any_element()
}

/// VS Code 风格的分组标题。
fn section_header(label: &str, color: gpui::Rgba) -> AnyElement {
    div()
        .px_3()
        .py(px(6.))
        .flex()
        .items_center()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color)
        .child(SharedString::from(label))
        .into_any_element()
}

/// 状态字形（M/A/D/R/!/?）。
fn status_glyph(status: ChangeStatus) -> AnyElement {
    let color = match status {
        ChangeStatus::Modified => theme::warning(),
        ChangeStatus::Added | ChangeStatus::Renamed => theme::accent(),
        ChangeStatus::Deleted | ChangeStatus::Conflict => theme::danger(),
        ChangeStatus::Untracked => theme::faint_text(),
    };
    div()
        .w(px(14.))
        .flex_none()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color)
        .child(SharedString::from(status.glyph()))
        .into_any_element()
}

/// 顶部的小徽章（↑n / ↓n）。
fn status_badge(text: String, color: gpui::Rgba) -> AnyElement {
    div()
        .px(px(6.))
        .rounded(px(theme::RADIUS_SM))
        .bg(color)
        .text_xs()
        .text_color(theme::canvas())
        .child(SharedString::from(text))
        .into_any_element()
}

/// 单个 diff 行：旧行号 | 新行号 | 内容。
fn render_diff_line(line: &crossh_core::git::DiffLine) -> AnyElement {
    let (bg, fg) = match line.kind {
        DiffLineKind::Hunk => (theme::surface(), theme::muted_text()),
        DiffLineKind::Added => (theme::diff_add_bg(), theme::diff_add_fg()),
        DiffLineKind::Removed => (theme::diff_del_bg(), theme::diff_del_fg()),
        DiffLineKind::Context => (theme::canvas(), theme::text()),
    };

    if line.kind == DiffLineKind::Hunk {
        return div()
            .px_2()
            .py(px(2.))
            .bg(bg)
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(fg)
            .child(SharedString::from(line.text.clone()))
            .into_any_element();
    }

    let old_number = line
        .old_ln
        .map(|number| format!("{number}"))
        .unwrap_or_default();
    let new_number = line
        .new_ln
        .map(|number| format!("{number}"))
        .unwrap_or_default();

    div()
        .flex()
        .flex_row()
        .bg(bg)
        .child(
            div()
                .w(px(44.))
                .flex_none()
                .px(px(6.))
                .flex()
                .justify_end()
                .text_xs()
                .text_color(fg)
                .child(SharedString::from(old_number)),
        )
        .child(
            div()
                .w(px(44.))
                .flex_none()
                .px(px(6.))
                .flex()
                .justify_end()
                .text_xs()
                .text_color(fg)
                .child(SharedString::from(new_number)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .px(px(6.))
                .overflow_x_hidden()
                .text_xs()
                .text_color(fg)
                .child(SharedString::from(line.text.clone())),
        )
        .into_any_element()
}

fn empty_hint(text: &str) -> AnyElement {
    div()
        .p_4()
        .text_xs()
        .text_color(theme::faint_text())
        .child(SharedString::from(text))
        .into_any_element()
}

/// 打开（或复用来聚焦）Git 窗口；目录不同则重新指向。
/// 借鉴 Zed：窗口复用 + `cx.defer` 延迟到当前帧结束再开窗。
pub fn open_git_window(cwd: PathBuf, cx: &mut App) {
    if let Some(window) = cx
        .windows()
        .iter()
        .find_map(|handle| handle.downcast::<GitWindow>())
    {
        let _ = window.update(cx, |this, window, cx| {
            if this.cwd != cwd {
                this.cwd = cwd;
                this.label = this
                    .cwd
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| this.cwd.to_string_lossy().into_owned());
                this.changes = Vec::new();
                this.selected = None;
                this.diff = None;
                this.refresh_list(cx);
            }
            window.activate_window();
            cx.notify();
        });
        return;
    }

    cx.defer(move |cx| {
        let bounds = Bounds::centered(
            None,
            Size {
                width: px(1000.),
                height: px(640.),
            },
            cx,
        );
        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::from(
                        cwd.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| cwd.to_string_lossy().into_owned()),
                    )),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(Size {
                    width: px(720.),
                    height: px(480.),
                }),
                ..Default::default()
            },
            |_window, cx| cx.new(|cx| GitWindow::new(cwd.clone(), cx)),
        )
        .ok();
    });
}

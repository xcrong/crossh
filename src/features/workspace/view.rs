//! 工作区渲染：标签条 + 递归分屏树、状态栏与输入弹窗。
//!
//! 布局来自 daemon 快照；渲染层按 `PaneNodeSnapshot` 递归生成 flex 行/列，
//! 终端视图按 `PaneId` 从 `WorkspaceState` 取出。

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, App, AppContext, Context, DispatchPhase, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, SharedString,
    StatefulInteractiveElement, Styled, Window, canvas, div, px,
};

use crate::features::settings::is_settings_window_open;
use crate::features::updates::UpdateStatus;
use crate::features::workspace::empty_state;
use crate::features::workspace::projection;
use crate::features::workspace::registry::SplitDrag;
use crate::features::workspace::shell::{AppShell, GitSyncOperation, GitSyncState};
use crate::features::workspace::tab_strip;
use crate::features::workspace::toaster::{ToastNotice, ToastTone};
use crossh_core::editor_launcher;
use crossh_core::git_status::GitStatus;
use crossh_core::i18n;
use crossh_session::workspace::{
    Axis, PaneId, PaneNodeSnapshot, PaneSnapshot, PaneStatus, TabSnapshot,
};
use crossh_terminal_view::TerminalItem as _;
use crossh_ui::{icons, theme};
use crossh_ui_component::{
    BadgeTone, Button, ButtonSize, ButtonVariant, ModalDialog, ModalField, SharedTextState,
    StatusBar, StatusMetric, Tooltip,
};

const TERMINAL_SPLIT_MIN_PANE_WIDTH: f32 = 160.0;
const TERMINAL_SPLIT_HANDLE_WIDTH: f32 = 8.0;
const TERMINAL_SPLIT_MIN_PANE_HEIGHT: f32 = 80.0;

fn terminal_split_available(width: Pixels) -> bool {
    width.as_f32() >= TERMINAL_SPLIT_MIN_PANE_WIDTH * 2.0 + TERMINAL_SPLIT_HANDLE_WIDTH
}

fn terminal_split_vertical_available(height: Pixels) -> bool {
    height.as_f32() >= TERMINAL_SPLIT_MIN_PANE_HEIGHT * 2.0 + TERMINAL_SPLIT_HANDLE_WIDTH
}

/// 焦点跟随鼠标的纯判定：仅当开关开启、当前是纯悬停、且悬停格尚非聚焦侧时才切焦点。
fn hover_should_switch_focus(enabled: bool, already_focused: bool, is_plain_hover: bool) -> bool {
    enabled && is_plain_hover && !already_focused
}

/// 主区：标签条 + 内容区。
pub fn render_main(
    shell: &mut AppShell,
    window: &Window,
    available_width: Pixels,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let mut pane = div()
        .flex_1()
        .min_h_0()
        .h_full()
        .bg(theme::canvas())
        .flex()
        .flex_col();

    let has_tabs = shell
        .workspace
        .active_project()
        .is_some_and(|project| !project.tabs.is_empty());
    if has_tabs {
        pane = pane.child(tab_strip::render_tab_strip(shell, cx));
    }

    let available_height = px((window.viewport_size().height.as_f32()
        - theme::TAB_HEIGHT
        - theme::STATUS_BAR_HEIGHT)
        .max(120.));
    let body = match shell.workspace.active_tab() {
        Some(tab) => render_pane_node(
            shell,
            tab,
            &tab.layout,
            available_width,
            available_height,
            cx,
        ),
        None => empty_state::render(shell, available_width, cx),
    };

    let mut terminal_area = div().flex_1().min_w_0().min_h_0().relative().child(body);
    if let Some(status) = &shell.status {
        terminal_area = terminal_area.child(
            div()
                .absolute()
                .bottom_2()
                .left_2()
                .px_3()
                .py_1()
                .rounded(px(theme::RADIUS_SM))
                .border_1()
                .border_color(theme::border_strong())
                .bg(theme::raised())
                .text_xs()
                .text_color(theme::text())
                .child(SharedString::from(status.clone())),
        );
    }

    let content = div()
        .flex_1()
        .min_h_0()
        .flex()
        .relative()
        .child(render_split_drag_overlay(shell, cx))
        .child(terminal_area);
    pane.child(content).into_any_element()
}

/// 全局分屏拖拽监听：手柄按下时写入 `SplitDrag`，这里统一换算比例并在松手时提交。
fn render_split_drag_overlay(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let drag_cell = shell.workspace.split_drag_cell.clone();
    let weak = cx.weak_entity();
    canvas(|_, _, _| {}, {
        let drag_cell = drag_cell.clone();
        move |_, _, window: &mut Window, _| {
            window.on_mouse_event({
                let drag_cell = drag_cell.clone();
                move |event: &MouseMoveEvent, phase, window, _cx| {
                    if !matches!(phase, DispatchPhase::Bubble) {
                        return;
                    }
                    let Some(mut drag) = drag_cell.get() else {
                        return;
                    };
                    let position = match drag.axis {
                        Axis::Row => event.position.x.as_f32(),
                        Axis::Column => event.position.y.as_f32(),
                    };
                    let total = drag.total_px.max(1.);
                    drag.ratio = projection::clamp_split_ratio(
                        drag.start_ratio + (position - drag.start_position) / total,
                    );
                    drag_cell.set(Some(drag));
                    window.refresh();
                }
            });
            window.on_mouse_event({
                let drag_cell = drag_cell.clone();
                let weak = weak.clone();
                move |_event: &MouseUpEvent, phase, window, cx: &mut App| {
                    if !matches!(phase, DispatchPhase::Bubble) {
                        return;
                    }
                    let Some(drag) = drag_cell.take() else {
                        return;
                    };
                    window.refresh();
                    let _ = weak.update(cx, |this, cx| {
                        this.commit_split_ratio(drag.split_id, drag.ratio, cx);
                    });
                }
            });
        }
    })
    .absolute()
    .size_full()
    .into_any_element()
}

fn render_pane_node(
    shell: &AppShell,
    tab: &TabSnapshot,
    node: &PaneNodeSnapshot,
    width: Pixels,
    height: Pixels,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    match node {
        PaneNodeSnapshot::Leaf(pane) => {
            render_pane_leaf(shell, pane, tab.active_pane == Some(pane.id), cx)
        }
        PaneNodeSnapshot::Split {
            id,
            axis,
            ratio,
            first,
            second,
        } => {
            let preview = shell
                .workspace
                .split_drag_cell
                .get()
                .filter(|drag| drag.split_id == *id)
                .map(|drag| drag.ratio);
            let ratio = preview.unwrap_or_else(|| projection::clamp_split_ratio(*ratio));
            let is_row = *axis == Axis::Row;
            let total = if is_row {
                width.as_f32()
            } else {
                height.as_f32()
            };
            let first_px = px((total * ratio).max(0.));
            let container = if is_row {
                div().size_full().flex().flex_row()
            } else {
                div().size_full().flex().flex_col()
            };

            let first_wrapper = if is_row {
                div().w(first_px).h_full().flex_none().min_w_0().min_h_0()
            } else {
                div().h(first_px).w_full().flex_none().min_w_0().min_h_0()
            };
            let second_wrapper = if is_row {
                div().flex_1().h_full().min_w_0().min_h_0()
            } else {
                div().flex_1().w_full().min_w_0().min_h_0()
            };

            let split_id = *id;
            let drag_cell = shell.workspace.split_drag_cell.clone();
            let divider = if is_row {
                div()
                    .id(("split-divider", split_id.0))
                    .w(px(TERMINAL_SPLIT_HANDLE_WIDTH))
                    .h_full()
                    .flex_none()
                    .cursor_col_resize()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .w(px(1.))
                            .h_full()
                            .bg(theme::border())
                            .hover(|style| style.bg(theme::accent())),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        move |event: &MouseDownEvent, window, _| {
                            drag_cell.set(Some(SplitDrag {
                                split_id,
                                axis: Axis::Row,
                                total_px: width.as_f32(),
                                start_ratio: ratio,
                                start_position: event.position.x.as_f32(),
                                ratio,
                            }));
                            window.refresh();
                        },
                    )
            } else {
                div()
                    .id(("split-divider", split_id.0))
                    .w_full()
                    .h(px(TERMINAL_SPLIT_HANDLE_WIDTH))
                    .flex_none()
                    .cursor_row_resize()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .h(px(1.))
                            .w_full()
                            .bg(theme::border())
                            .hover(|style| style.bg(theme::accent())),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        move |event: &MouseDownEvent, window, _| {
                            drag_cell.set(Some(SplitDrag {
                                split_id,
                                axis: Axis::Column,
                                total_px: height.as_f32(),
                                start_ratio: ratio,
                                start_position: event.position.y.as_f32(),
                                ratio,
                            }));
                            window.refresh();
                        },
                    )
            };

            let (first_width, first_height) = if is_row {
                (first_px, height)
            } else {
                (width, first_px)
            };
            let (second_width, second_height) = if is_row {
                (px((total - total * ratio).max(0.)), height)
            } else {
                (width, px((total - total * ratio).max(0.)))
            };
            container
                .child(first_wrapper.child(render_pane_node(
                    shell,
                    tab,
                    first,
                    first_width,
                    first_height,
                    cx,
                )))
                .child(divider)
                .child(second_wrapper.child(render_pane_node(
                    shell,
                    tab,
                    second,
                    second_width,
                    second_height,
                    cx,
                )))
                .into_any_element()
        }
    }
}

fn render_pane_leaf(
    shell: &AppShell,
    pane: &PaneSnapshot,
    active: bool,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let content = match shell.workspace.pane_view(pane.id) {
        Some(view) => view.clone().into_any_element(),
        None => render_pane_placeholder(shell, pane, cx),
    };
    let pane_id = pane.id;
    div()
        .id(("pane-leaf", pane.id.0))
        .relative()
        .size_full()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .border_t_1()
        .border_color(if active {
            theme::accent()
        } else {
            theme::border()
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _event, _window, cx| {
                this.focus_pane(pane_id, cx);
            }),
        )
        .on_mouse_move(
            cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                let already_focused = this.workspace.focused_pane() == Some(pane_id);
                let is_plain_hover = event.pressed_button.is_none() && !cx.has_active_drag();
                if hover_should_switch_focus(
                    this.workspace_settings.focus_follows_mouse,
                    already_focused,
                    is_plain_hover,
                ) {
                    this.focus_pane(pane_id, cx);
                }
            }),
        )
        .child(content)
        .into_any_element()
}

fn render_pane_placeholder(
    shell: &AppShell,
    pane: &PaneSnapshot,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let reason = i18n::text(projection::pane_status_reason_key(&pane.status));
    let can_retry = !matches!(pane.status, PaneStatus::Starting)
        && pane.cwd_available
        && shell.workspace.is_connected();
    let pane_id = pane.id;
    let mut card = div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .px_4()
        .text_center()
        .child(
            div()
                .text_sm()
                .text_color(theme::muted_text())
                .child(SharedString::from(reason)),
        )
        .child(
            div()
                .max_w(px(520.))
                .truncate()
                .text_xs()
                .text_color(theme::faint_text())
                .child(SharedString::from(pane.cwd.to_string_lossy().into_owned())),
        );
    if can_retry {
        card = card.child(
            Button::new(("pane-retry", pane.id.0))
                .size(ButtonSize::Small)
                .variant(ButtonVariant::Secondary)
                .icon(icons::icon(icons::IconName::RefreshCw, 12.))
                .label(i18n::text("workspace.retry_pane"))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.ensure_pane_session(pane_id, cx);
                })),
        );
    }
    card.into_any_element()
}

fn copy_status_path_to_clipboard(path: &Path, cx: &App) {
    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
        path.to_string_lossy().into_owned(),
    ));
}

/// 「在外部编辑器中打开」状态栏按钮；目标为聚焦 pane 的 cwd。
fn render_open_in_editor_button(
    cwd: PathBuf,
    shell: &AppShell,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let directory = cwd;
    let tooltip = match tooltip_editor_name(shell) {
        Some(name) => {
            rust_i18n::t!("tooltip.open_in_editor_with", editor = name.as_str()).to_string()
        }
        None => i18n::text("tooltip.open_in_editor"),
    };
    Button::new("status-open-in-editor")
        .size(ButtonSize::Icon(px(22.)))
        .variant(ButtonVariant::Ghost)
        .icon(icons::icon(icons::IconName::SquarePen, 13.).text_color(theme::muted_text()))
        .tooltip(tooltip)
        .on_click(cx.listener(move |this, _event, _window, cx| {
            this.open_project_in_editor(&directory, cx);
        }))
        .into_any_element()
}

/// 当前解析出的编辑器显示名：配置值原样，检测结果取 basename，供 tooltip 展示。
fn tooltip_editor_name(shell: &AppShell) -> Option<String> {
    let path_env = editor_launcher::effective_path();
    let editor = editor_launcher::resolve_editor(
        shell.workspace_settings.editor_command.as_deref(),
        &path_env,
        editor_launcher::executable_exists,
    );
    editor.map(|binary| editor_launcher::command_display_name(&binary))
}

fn render_status_path(cwd: PathBuf, cx: &mut Context<AppShell>) -> AnyElement {
    let path_text = cwd.to_string_lossy().into_owned();

    div()
        .id("status-path")
        .ml_2()
        .min_w_0()
        .flex()
        .items_center()
        .gap_2()
        .px(px(6.))
        .py(px(2.))
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .text_color(theme::muted_text())
        .hover(|style| style.bg(theme::raised()).text_color(theme::text()))
        .on_click(cx.listener(move |this, _event, _window, cx| {
            copy_status_path_to_clipboard(&cwd, cx);
            this.show_toast(
                ToastNotice::new(i18n::text("toast.path_copied"), ToastTone::Success),
                cx,
            );
            cx.stop_propagation();
        }))
        .child(
            icons::icon(icons::IconName::FolderOpen, 12.)
                .flex_shrink_0()
                .text_color(theme::faint_text()),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .child(SharedString::from(path_text)),
        )
        .into_any_element()
}

pub(crate) fn render_workspace_status_bar(
    shell: &AppShell,
    available_width: Pixels,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let pane = shell.workspace.active_pane();
    let left = render_status_left_pane(
        render_status_left_basic(shell, cx),
        shell,
        available_width,
        pane,
        cx,
    );
    let right = render_status_right(shell, cx);

    StatusBar::new("workspace-status-bar")
        .child(left)
        .child(right)
        .into_any_element()
}

/// 左侧基础开关：设置与 host 侧边栏。
fn render_status_left_basic(shell: &AppShell, cx: &mut Context<AppShell>) -> gpui::Div {
    div()
        .min_w_0()
        .flex()
        .items_center()
        .gap_1()
        .child(render_status_bar_toggle(
            "status-settings",
            icons::IconName::Settings,
            "tooltip.settings",
            is_settings_window_open(cx),
            AppShell::toggle_settings,
            cx,
        ))
        .child(render_status_bar_toggle(
            "status-host-sidebar",
            icons::IconName::PanelLeft,
            "tooltip.host_sidebar",
            shell.workspace_settings.show_host_sidebar,
            AppShell::toggle_host_sidebar,
            cx,
        ))
}

/// 在左侧容器上追加 pane 区：时间戳/分屏/焦点开关、compose 按钮与编辑器/path/git 状态。
fn render_status_left_pane(
    mut left: gpui::Div,
    shell: &AppShell,
    available_width: Pixels,
    pane: Option<&PaneSnapshot>,
    cx: &mut Context<AppShell>,
) -> gpui::Div {
    if pane.is_some() {
        let show_timestamps = pane
            .and_then(|pane| shell.workspace.pane_view(pane.id))
            .map(|view| view.show_timestamps(cx))
            .unwrap_or(shell.terminal_settings.show_timestamps);
        left = left.child(render_status_bar_toggle(
            "status-timestamps",
            icons::IconName::Clock,
            "tooltip.timestamps",
            show_timestamps,
            AppShell::toggle_timestamps,
            cx,
        ));
        left = left.child(render_workspace_terminal_toggle(shell, available_width, cx));
        left = left.child(render_status_bar_toggle(
            "status-focus-follows-mouse",
            icons::IconName::Crosshair,
            "tooltip.focus_follows_mouse",
            shell.workspace_settings.focus_follows_mouse,
            AppShell::toggle_focus_follows_mouse,
            cx,
        ));
    }

    let compose_active = shell.workspace.compose_visible_for_focused();
    left = left.child(
        Button::new("status-compose")
            .size(ButtonSize::Icon(px(22.)))
            .variant(ButtonVariant::Ghost)
            .selected(compose_active)
            .disabled(pane.is_none())
            .icon(
                icons::icon(icons::IconName::Keyboard, 13.).text_color(if compose_active {
                    theme::accent()
                } else {
                    theme::muted_text()
                }),
            )
            .tooltip(i18n::text("tooltip.compose_bar"))
            .on_click(cx.listener(|this, _ev, window, cx| {
                this.toggle_compose_bar(window, cx);
            }))
            .into_any_element(),
    );

    if let Some(pane) = pane {
        left = left.child(render_open_in_editor_button(pane.cwd.clone(), shell, cx));
        left = left.child(render_status_path(pane.cwd.clone(), cx));
        if let Some(status) = shell.workspace.git_status.get(&pane.id) {
            let sync = shell.git_sync.get(&pane.id);
            left = left.child(render_git_status(
                status,
                pane.id,
                pane.cwd.clone(),
                sync,
                cx,
            ));
        }
    }
    left
}

/// 右侧开关：scratch、链接预览、系统监控、笔记与更新指示器。
fn render_status_right(shell: &AppShell, cx: &mut Context<AppShell>) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(
            Button::new("status-scratch")
                .size(ButtonSize::Icon(px(22.)))
                .variant(ButtonVariant::Ghost)
                .selected(shell.scratch_visible)
                .icon(icons::icon(icons::IconName::Terminal, 13.).text_color(
                    if shell.scratch_visible {
                        theme::accent()
                    } else {
                        theme::muted_text()
                    },
                ))
                .tooltip(i18n::text("tooltip.scratch_terminal"))
                .on_click(cx.listener(|this, _ev, window, cx| {
                    this.toggle_scratch_terminal(window, cx);
                }))
                .into_any_element(),
        )
        .children(crate::features::workspace::shell::link_preview::render_status_toggle(shell, cx))
        .child(
            Button::new("status-system-monitor")
                .size(ButtonSize::Icon(px(22.)))
                .variant(ButtonVariant::Ghost)
                .selected(shell.system_monitor.visible)
                .icon(icons::icon(icons::IconName::Activity, 13.).text_color(
                    if shell.system_monitor.visible {
                        theme::accent()
                    } else {
                        theme::muted_text()
                    },
                ))
                .tooltip(i18n::text("tooltip.system_monitor"))
                .on_click(cx.listener(|this, _ev, _window, cx| {
                    this.toggle_system_monitor(cx);
                }))
                .into_any_element(),
        )
        .child(
            Button::new("status-note")
                .size(ButtonSize::Icon(px(22.)))
                .variant(ButtonVariant::Ghost)
                .icon(icons::icon(icons::IconName::FileText, 13.).text_color(theme::muted_text()))
                .tooltip(i18n::text("tooltip.note"))
                .on_click(cx.listener(|_, _ev, _window, _cx| {
                    if let Err(error) = crossh_core::note_launcher::spawn_note_process() {
                        log::warn!("spawn note failed: {error}");
                    }
                }))
                .into_any_element(),
        )
        .child(render_update_indicator(shell, cx))
}

fn render_workspace_terminal_toggle(
    shell: &AppShell,
    available_width: Pixels,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let Some(tab) = shell.workspace.active_tab() else {
        return div().h(px(0.)).flex_none().into_any_element();
    };
    let has_split = tab.layout.leaves().count() > 1;
    let can_split_horizontal = has_split || terminal_split_available(available_width);
    let can_split_vertical =
        has_split || terminal_split_vertical_available(px((available_width.as_f32() - 0.).max(0.)));
    div()
        .flex()
        .flex_row()
        .gap_1()
        .child(
            Button::new("terminal-split-toggle")
                .size(ButtonSize::Icon(px(22.)))
                .variant(ButtonVariant::Ghost)
                .selected(has_split)
                .disabled(!can_split_horizontal)
                .icon(
                    icons::icon(icons::IconName::Columns2, 13.).text_color(if has_split {
                        theme::accent()
                    } else {
                        theme::muted_text()
                    }),
                )
                .tooltip(split_toggle_tooltip(
                    if has_split {
                        "tooltip.close_split"
                    } else {
                        "tooltip.split_terminal"
                    },
                    split_horizontal_hint(),
                ))
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.toggle_terminal_split(cx);
                })),
        )
        .child(
            Button::new("terminal-vertical-split-toggle")
                .size(ButtonSize::Icon(px(22.)))
                .variant(ButtonVariant::Ghost)
                .selected(has_split)
                .disabled(!can_split_vertical)
                .icon(
                    icons::icon(icons::IconName::Rows2, 13.).text_color(if has_split {
                        theme::accent()
                    } else {
                        theme::muted_text()
                    }),
                )
                .tooltip(split_toggle_tooltip(
                    if has_split {
                        "tooltip.close_vertical_split"
                    } else {
                        "tooltip.split_terminal_vertical"
                    },
                    split_vertical_hint(),
                ))
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.toggle_vertical_split(cx);
                })),
        )
        .into_any_element()
}

#[cfg(target_os = "macos")]
fn split_horizontal_hint() -> Option<&'static str> {
    Some("⌘D")
}

#[cfg(not(target_os = "macos"))]
fn split_horizontal_hint() -> Option<&'static str> {
    Some("Win+D")
}

#[cfg(target_os = "macos")]
fn split_vertical_hint() -> Option<&'static str> {
    Some("⇧⌘D")
}

#[cfg(not(target_os = "macos"))]
fn split_vertical_hint() -> Option<&'static str> {
    Some("Shift+Win+D")
}

/// tooltip 文案 + 可选快捷键后缀。
fn split_toggle_tooltip(base_key: &str, hint: Option<&str>) -> String {
    match hint {
        Some(hint) => format!("{}（{}）", i18n::text(base_key), hint),
        None => i18n::text(base_key),
    }
}

/// 状态栏更新指示器：检查 → 下载 → 重启，随更新状态切换图标与点击动作。
fn render_update_indicator(shell: &AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let status = shell
        .updates
        .read_with(cx, |updates, _app| updates.status().clone());
    let indicator = update_indicator(&status);
    let mut row = div().flex().items_center().gap_1().child(
        Button::new("status-update")
            .size(ButtonSize::Icon(px(22.)))
            .variant(ButtonVariant::Ghost)
            .disabled(indicator.disabled)
            .icon(icons::icon(indicator.icon, 13.).text_color(indicator.color))
            .tooltip(indicator.tooltip)
            .on_click(cx.listener(|this, _ev, _window, cx| {
                this.activate_update_indicator(cx);
            })),
    );
    if let Some(progress) = indicator.progress {
        row = row.child(
            div()
                .text_color(theme::muted_text())
                .child(SharedString::from(progress)),
        );
    }
    row.into_any_element()
}

/// 更新状态在状态栏上的呈现：图标、颜色、提示、可点击性与下载进度文本。
struct UpdateIndicator {
    icon: icons::IconName,
    color: gpui::Rgba,
    tooltip: String,
    /// 检查/下载进行中：只展示状态，不接受点击。
    disabled: bool,
    progress: Option<String>,
}

fn update_indicator(status: &UpdateStatus) -> UpdateIndicator {
    match status {
        UpdateStatus::Idle => UpdateIndicator {
            icon: icons::IconName::RefreshCw,
            color: theme::muted_text(),
            tooltip: i18n::text("tooltip.update_check"),
            disabled: false,
            progress: None,
        },
        UpdateStatus::Checking => UpdateIndicator {
            icon: icons::IconName::RefreshCw,
            color: theme::info(),
            tooltip: i18n::text("settings.updates_checking"),
            disabled: true,
            progress: None,
        },
        UpdateStatus::UpToDate => UpdateIndicator {
            icon: icons::IconName::Check,
            color: theme::muted_text(),
            tooltip: i18n::text("settings.updates_up_to_date"),
            disabled: false,
            progress: None,
        },
        UpdateStatus::Available(candidate) => UpdateIndicator {
            icon: icons::IconName::Download,
            color: theme::info(),
            tooltip: rust_i18n::t!(
                "tooltip.update_download",
                version = candidate.version.to_string()
            )
            .to_string(),
            disabled: false,
            progress: None,
        },
        UpdateStatus::Downloading {
            candidate,
            downloaded,
            total,
        } => {
            let fraction = if *total == 0 {
                0.
            } else {
                (*downloaded as f32 / *total as f32).clamp(0., 1.)
            };
            let progress = format!("{:.0}%", fraction * 100.);
            UpdateIndicator {
                icon: icons::IconName::Download,
                color: theme::warning(),
                tooltip: rust_i18n::t!(
                    "tooltip.update_downloading",
                    version = candidate.version.to_string(),
                    progress = progress.clone()
                )
                .to_string(),
                disabled: true,
                progress: Some(progress),
            }
        }
        UpdateStatus::Ready { candidate, .. } => UpdateIndicator {
            icon: icons::IconName::RefreshCw,
            color: theme::accent(),
            tooltip: rust_i18n::t!(
                "tooltip.update_restart",
                version = candidate.version.to_string()
            )
            .to_string(),
            disabled: false,
            progress: None,
        },
        UpdateStatus::Failed(error) => UpdateIndicator {
            icon: icons::IconName::CircleX,
            color: theme::danger(),
            tooltip: rust_i18n::t!("settings.updates_failed", error = error).to_string(),
            disabled: false,
            progress: None,
        },
    }
}

fn render_status_bar_toggle(
    id: &'static str,
    icon: icons::IconName,
    tooltip: &'static str,
    active: bool,
    toggle: fn(&mut AppShell, &mut Context<AppShell>),
    cx: &mut Context<AppShell>,
) -> AnyElement {
    Button::new(id)
        .size(ButtonSize::Icon(px(22.)))
        .variant(ButtonVariant::Ghost)
        .icon(icons::icon(icon, 13.).text_color(if active {
            theme::accent()
        } else {
            theme::muted_text()
        }))
        .tooltip(i18n::text(tooltip))
        .on_click(cx.listener(move |this, _ev, _window, cx| toggle(this, cx)))
        .into_any_element()
}

fn render_git_status(
    status: &GitStatus,
    pane_id: PaneId,
    cwd: PathBuf,
    sync: Option<&GitSyncState>,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let click_cwd = cwd;
    let mut git = div()
        .id("status-git")
        .flex_none()
        .flex()
        .items_center()
        .gap_2()
        .px(px(6.))
        .py(px(2.))
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .hover(|s| s.bg(theme::raised()))
        .tooltip(|_window, cx| {
            cx.new(|_| Tooltip::new(crossh_core::i18n::text("git.title")))
                .into()
        })
        .child(icons::icon(icons::IconName::GitBranch, 13.).text_color(theme::accent()))
        .child(
            div()
                .text_color(theme::text())
                .child(SharedString::from(status.branch.clone())),
        )
        .on_click(cx.listener(move |_this, _ev, _window, _cx| {
            if let Err(error) = crossh_core::git_launcher::spawn_git_process(&click_cwd) {
                log::error!(
                    "failed to start crossh-git for {}: {error}",
                    click_cwd.display()
                );
            }
        }));
    if !status.commit.is_empty() {
        git = git.child(
            div()
                .flex_shrink_0()
                .text_color(theme::muted_text())
                .child(SharedString::from(status.commit.clone())),
        );
    }

    if status.ahead > 0 {
        git = git.child(StatusMetric::new(format!("↑{}", status.ahead)).tone(BadgeTone::Info));
    }
    if status.behind > 0 {
        git = git.child(StatusMetric::new(format!("↓{}", status.behind)).tone(BadgeTone::Info));
    }
    if status.staged > 0 {
        git = git.child(StatusMetric::new(format!("+{}", status.staged)).tone(BadgeTone::Accent));
    }
    if status.modified > 0 {
        git =
            git.child(StatusMetric::new(format!("~{}", status.modified)).tone(BadgeTone::Warning));
    }
    if status.untracked > 0 {
        git =
            git.child(StatusMetric::new(format!("?{}", status.untracked)).tone(BadgeTone::Neutral));
    }
    if status.conflicts > 0 {
        git =
            git.child(StatusMetric::new(format!("!{}", status.conflicts)).tone(BadgeTone::Danger));
    }
    if status.is_clean() {
        git = git.child(StatusMetric::new(i18n::text("git.clean")).tone(BadgeTone::Success));
    }
    if status.behind > 0 || status.ahead > 0 || sync.is_some() {
        let mut actions = div().flex().items_center().gap_1();
        if status.behind > 0 || sync.is_some_and(|state| state.operation == GitSyncOperation::Pull)
        {
            actions = actions.child(git_sync_button(
                "status-git-pull",
                icons::IconName::Download,
                i18n::text("git.pull"),
                GitSyncOperation::Pull,
                sync,
                pane_id,
                cx,
            ));
        }
        if status.ahead > 0 || sync.is_some_and(|state| state.operation == GitSyncOperation::Push) {
            actions = actions.child(git_sync_button(
                "status-git-push",
                icons::IconName::Upload,
                i18n::text("git.push"),
                GitSyncOperation::Push,
                sync,
                pane_id,
                cx,
            ));
        }
        git = git.child(actions);
    }
    git.into_any_element()
}

fn git_sync_button(
    id: &'static str,
    icon: icons::IconName,
    tooltip: String,
    operation: GitSyncOperation,
    sync: Option<&GitSyncState>,
    pane_id: PaneId,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let state = sync.filter(|state| state.operation == operation);
    let running = state.is_some_and(|state| state.running);
    let error = state.and_then(|state| state.error.as_deref());
    Button::new(id)
        .size(ButtonSize::Icon(px(20.)))
        .variant(ButtonVariant::Ghost)
        .loading(running)
        .disabled(running)
        .tooltip(if let Some(error) = error {
            SharedString::from(error)
        } else {
            SharedString::from(tooltip)
        })
        .icon(icons::icon(icon, 12.).text_color(if error.is_some() {
            theme::danger()
        } else {
            theme::accent()
        }))
        .on_click(cx.listener(move |this, _event, _window, cx| {
            this.run_git_sync(pane_id, operation, cx);
            cx.stop_propagation();
        }))
        .into_any_element()
}

// 收敛点：3 个弹窗仅 6 参数不同（id / placeholder / title / icon / width / on_submit/on_cancel），
// 其余 div 样式/选中高亮/caret/marked/ime_input_canvas 均由 ModalField 统一。
#[allow(clippy::too_many_arguments)]
fn render_single_line_modal(
    focus: gpui::FocusHandle,
    state: SharedTextState,
    scroll: Option<gpui::ScrollHandle>,
    cx: &mut Context<AppShell>,
    input_id: &'static str,
    placeholder: String,
    title: String,
    icon: icons::IconName,
    width: gpui::Pixels,
    scrim_id: &'static str,
    card_id: &'static str,
    primary_label: String,
    save_id: &'static str,
    cancel_id: &'static str,
    on_save: fn(&mut AppShell, &mut Context<AppShell>),
    on_cancel: fn(&mut AppShell, &mut Context<AppShell>),
) -> AnyElement {
    let mut input = ModalField::new(input_id, focus, &state)
        .placeholder(placeholder)
        .entity(cx.entity())
        .on_key_down(cx.listener(|this, e, w, cx| this.handle_modal_editor_key(e, w, cx)));
    if let Some(handle) = scroll {
        input = input.scrollable(handle);
    }
    let buttons = div()
        .flex()
        .flex_row()
        .gap_2()
        .child(
            Button::new(save_id)
                .size(ButtonSize::Medium)
                .variant(ButtonVariant::Primary)
                .icon(icons::icon(icons::IconName::Check, 13.).text_color(theme::canvas()))
                .label(primary_label)
                .on_click(cx.listener(move |this, _ev, _window, cx| {
                    on_save(this, cx);
                })),
        )
        .child(
            Button::new(cancel_id)
                .size(ButtonSize::Medium)
                .variant(ButtonVariant::Secondary)
                .icon(icons::icon(icons::IconName::X, 13.).text_color(theme::muted_text()))
                .label(i18n::text("prompt.cancel"))
                .on_click(cx.listener(move |this, _ev, _window, cx| {
                    on_cancel(this, cx);
                })),
        );
    ModalDialog::new(title, icons::icon(icon, 16.).text_color(theme::info()))
        .width(width)
        .scrim_id(scrim_id)
        .card_id(card_id)
        .blocks_card_clicks()
        .on_backdrop_click(cx.listener(move |this, _ev, _window, cx| {
            on_cancel(this, cx);
        }))
        .child(input)
        .actions(buttons)
        .into_any_element()
}

/// 固定标签重命名弹窗：ModalDialog + 自绘文本输入（IME 支持）。
pub fn render_rename_editor(
    shell: &mut AppShell,
    _window: &Window,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let Some(editor) = &shell.rename_editor else {
        return div().into_any_element();
    };
    let state = SharedTextState::new(editor.state.value.clone())
        .with_cursor(editor.state.cursor)
        .with_anchor(editor.state.anchor)
        .with_ime_marked_text(editor.state.ime_marked_text.clone())
        .with_ime_replacement(editor.state.ime_replacement);
    render_single_line_modal(
        editor.focus.clone(),
        state,
        None,
        cx,
        "rename-editor-input",
        i18n::text("rename_tab.name_placeholder"),
        i18n::text("rename_tab.title"),
        icons::IconName::Pencil,
        px(420.),
        "rename-editor-scrim",
        "rename-editor-card",
        i18n::text("rename_tab.save"),
        "rename-editor-save",
        "rename-editor-cancel",
        AppShell::submit_rename_tab,
        AppShell::cancel_rename_tab,
    )
}

pub fn render_default_command_editor(
    shell: &mut AppShell,
    _window: &Window,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let Some(editor) = &shell.default_command_editor else {
        return div().into_any_element();
    };
    let state = SharedTextState::new(editor.state.value.clone())
        .with_cursor(editor.state.cursor)
        .with_anchor(editor.state.anchor)
        .with_ime_marked_text(editor.state.ime_marked_text.clone())
        .with_ime_replacement(editor.state.ime_replacement);
    render_single_line_modal(
        editor.focus.clone(),
        state,
        None,
        cx,
        "default-command-editor-input",
        i18n::text("default_command.placeholder"),
        i18n::text("default_command.title"),
        icons::IconName::Terminal,
        px(500.),
        "default-command-editor-scrim",
        "default-command-editor-card",
        i18n::text("default_command.save"),
        "default-command-editor-save",
        "default-command-editor-cancel",
        AppShell::submit_default_command,
        AppShell::cancel_default_command,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossh_update::{ArtifactFormat, UpdateArtifact, UpdateCandidate, UpdateTarget};
    use gpui::{ClipboardEntry, TestAppContext};

    #[test]
    fn hover_should_switch_focus_respects_toggle_drag_and_current_focus() {
        assert!(hover_should_switch_focus(true, false, true));
        assert!(!hover_should_switch_focus(false, false, true));
        assert!(!hover_should_switch_focus(true, true, true));
        assert!(!hover_should_switch_focus(true, false, false));
        assert!(!hover_should_switch_focus(false, true, false));
        assert!(!hover_should_switch_focus(true, true, false));
    }

    #[gpui::test]
    fn spec_20260817_workspace_status_path_copy_copies_full_current_path_to_clipboard(
        cx: &mut TestAppContext,
    ) {
        let path = PathBuf::from(
            "/Users/me/projects/crossh/a-very-long-directory-name-that-must-not-be-truncated",
        );

        cx.update(|cx| copy_status_path_to_clipboard(&path, cx));

        let copied = cx.read_from_clipboard().and_then(|item| {
            item.into_entries().find_map(|entry| match entry {
                ClipboardEntry::String(value) => Some(value.text),
                _ => None,
            })
        });
        assert_eq!(copied, Some(path.to_string_lossy().into_owned()));
    }

    #[test]
    fn terminal_split_requires_two_usable_columns() {
        assert!(!terminal_split_available(px(327.)));
        assert!(terminal_split_available(px(328.)));
    }

    fn update_candidate() -> UpdateCandidate {
        UpdateCandidate {
            version: semver::Version::new(0, 2, 0),
            notes: String::new(),
            release_url: None,
            artifact: UpdateArtifact {
                url: "https://example.com/crossh.zip".into(),
                filename: "crossh.zip".into(),
                format: ArtifactFormat::Zip,
                sha256: "0".repeat(64),
                size: 1024,
            },
            target: UpdateTarget::MacosAarch64,
        }
    }

    #[test]
    fn update_indicator_maps_every_status_to_its_action() {
        let idle = update_indicator(&UpdateStatus::Idle);
        assert_eq!(idle.icon, icons::IconName::RefreshCw);
        assert!(!idle.disabled);

        assert!(update_indicator(&UpdateStatus::Checking).disabled);

        assert_eq!(
            update_indicator(&UpdateStatus::UpToDate).icon,
            icons::IconName::Check
        );
        assert_eq!(
            update_indicator(&UpdateStatus::Available(update_candidate())).icon,
            icons::IconName::Download
        );

        let downloading = update_indicator(&UpdateStatus::Downloading {
            candidate: update_candidate(),
            downloaded: 512,
            total: 1024,
        });
        assert!(downloading.disabled);
        assert_eq!(downloading.progress.as_deref(), Some("50%"));

        let unknown_total = update_indicator(&UpdateStatus::Downloading {
            candidate: update_candidate(),
            downloaded: 0,
            total: 0,
        });
        assert_eq!(unknown_total.progress.as_deref(), Some("0%"));

        let ready = update_indicator(&UpdateStatus::Ready {
            candidate: update_candidate(),
            package: PathBuf::from("/tmp/crossh.zip"),
        });
        assert_eq!(ready.icon, icons::IconName::RefreshCw);
        assert_eq!(ready.color, theme::accent());
        assert!(!ready.disabled);

        let failed = update_indicator(&UpdateStatus::Failed("boom".into()));
        assert_eq!(failed.icon, icons::IconName::CircleX);
        assert_eq!(failed.color, theme::danger());
    }

    #[test]
    fn update_indicator_copy_resolves_in_both_locales() {
        for locale in ["en", "zh-CN"] {
            for key in [
                "tooltip.update_check",
                "tooltip.update_download",
                "tooltip.update_downloading",
                "tooltip.update_restart",
                "toast.update_downloaded",
            ] {
                let text = rust_i18n::t!(key, locale = locale).to_string();
                assert_ne!(text, key, "missing locale entry {key} for {locale}");
            }
        }
    }
}

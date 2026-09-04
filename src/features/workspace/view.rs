//! 工作区渲染：标签条 + 终端主区、状态栏与模态弹窗。

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext, Context, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::features::editor_launcher;
use crate::features::settings::is_settings_window_open;
use crate::features::workspace::empty_state;
use crate::features::workspace::registry::{SplitSide, TerminalSplitState};
use crate::features::workspace::shell::{AppShell, GitSyncOperation, GitSyncState};
pub use crate::features::workspace::state::{ActiveView, LocalDir, LocalSession, LocalSessionId};
use crate::features::workspace::tab_strip;
use crate::features::workspace::toaster::{ToastNotice, ToastTone};
use crate::shared::i18n;
use crossh_core::git_status::GitStatus;
use crossh_ui::{icons, theme};
use crossh_ui_component::{
    BadgeTone, Button, ButtonSize, ButtonVariant, ModalDialog, ModalField, SharedTextState,
    SplitResizer, StatusBar, StatusMetric, Tooltip,
};

const TERMINAL_SPLIT_MIN_PANE_WIDTH: f32 = 160.0;
const TERMINAL_SPLIT_HANDLE_WIDTH: f32 = 8.0;
const TERMINAL_SPLIT_MIN_PANE_HEIGHT: f32 = 80.0;

fn split_clamped_size(requested: f32, default: f32, min: f32, max: f32) -> f32 {
    let base = if requested <= 0.0 { default } else { requested };
    crossh_ui_component::clamp_panel_width(base, min, max)
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

    // 空状态（无活动视图）不渲染 Tab 条，避免 38px 空条 + 单个 "+" 的噪音；
    // 有会话时 TabStrip 负责展示对应项目的标签及 "+"。
    if shell.workspace.active_view.is_some() {
        pane = pane.child(tab_strip::render_tab_strip(shell, cx));
    }

    // 终端主区。
    let mut content = div().flex_1().min_h_0().flex().relative();
    let mut terminal_area = div().flex_1().min_w_0().min_h_0().relative();
    // 分栏跟随其属主 Tab：只有属主 Tab 正被展示时才渲染分栏；否则
    // 分栏状态保留，渲染当前活动 Tab 的普通视图，切回属主即恢复。
    if let Some(split) = shell.workspace.active_split() {
        terminal_area = terminal_area.child(render_terminal_split(
            shell,
            split,
            available_width,
            window,
            cx,
        ));
    } else if let Some(active_view) = shell.workspace.active_view {
        if let Some(active_pane) = render_workspace_view(shell, active_view) {
            terminal_area = terminal_area.child(active_pane);
        } else {
            terminal_area = terminal_area.child(empty_state::render(shell, available_width, cx));
        }
    } else {
        terminal_area = terminal_area.child(empty_state::render(shell, available_width, cx));
    }

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
    content = content.child(terminal_area);
    pane = pane.child(content);
    pane.into_any_element()
}

fn render_workspace_view(shell: &AppShell, view: ActiveView) -> Option<AnyElement> {
    match view {
        ActiveView::LocalSession(session_id) => shell
            .workspace
            .sessions
            .local_sessions
            .get(&session_id)
            .map(|session| session.terminal.clone().into_any_element()),
    }
}
fn render_terminal_split(
    shell: &mut AppShell,
    split: TerminalSplitState,
    available_width: Pixels,
    window: &Window,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let available_width_f = available_width.as_f32().max(0.0);
    let has_room_h = terminal_split_available(px(available_width_f));
    let pane_min_width = if has_room_h {
        TERMINAL_SPLIT_MIN_PANE_WIDTH
    } else {
        0.0
    };
    let max_left_width = if has_room_h {
        (available_width_f - TERMINAL_SPLIT_HANDLE_WIDTH - pane_min_width).max(pane_min_width)
    } else {
        (available_width_f - TERMINAL_SPLIT_HANDLE_WIDTH).max(0.0)
    };
    let default_left_width = ((available_width_f - TERMINAL_SPLIT_HANDLE_WIDTH) / 2.0).max(0.0);
    let split_width = shell
        .workspace
        .split_widths
        .get(&split.left)
        .cloned()
        .unwrap_or_else(|| Rc::new(Cell::new(0.)));
    let left_width = terminal_split_left_width(
        split_width.get(),
        default_left_width,
        pane_min_width,
        max_left_width,
    );

    // 高度：按窗口可用高度响应式，与宽度同构
    let viewport_h = window.viewport_size().height.as_f32();
    // 预留状态栏+标签栏约 72px，剩余为终端区高度
    let available_height_f = (viewport_h - 72.0).max(200.0);
    let has_room_v =
        available_height_f >= TERMINAL_SPLIT_MIN_PANE_HEIGHT * 2.0 + TERMINAL_SPLIT_HANDLE_WIDTH;
    let pane_min_height = if has_room_v {
        TERMINAL_SPLIT_MIN_PANE_HEIGHT
    } else {
        0.0
    };
    let max_top_height = if has_room_v {
        (available_height_f - TERMINAL_SPLIT_HANDLE_WIDTH - pane_min_height).max(pane_min_height)
    } else {
        (available_height_f - TERMINAL_SPLIT_HANDLE_WIDTH).max(0.0)
    };
    let default_top_height = ((available_height_f - TERMINAL_SPLIT_HANDLE_WIDTH) / 2.0).max(0.0);
    let split_height_left = shell
        .workspace
        .split_heights
        .get(&split.left)
        .cloned()
        .unwrap_or_else(|| Rc::new(Cell::new(0.)));
    let split_height_right = shell
        .workspace
        .split_heights_right
        .get(&split.left)
        .cloned()
        .unwrap_or_else(|| Rc::new(Cell::new(0.)));
    let left_top_height = terminal_split_top_height(
        split_height_left.get(),
        default_top_height,
        pane_min_height,
        max_top_height,
    );
    let right_top_height = terminal_split_top_height(
        split_height_right.get(),
        default_top_height,
        pane_min_height,
        max_top_height,
    );

    // 无右列：单列上下
    if split.right.is_none() {
        if let Some(bottom_view) = split.bottom_left {
            return render_single_column_vertical(
                shell,
                split,
                bottom_view,
                left_top_height,
                split_height_left,
                pane_min_height,
                max_top_height,
                cx,
            );
        }
        return render_split_pane(shell, split.left, SplitSide::Left, true, cx);
    }

    // 双列：各自按列独立上下
    let left_top_pane = render_split_pane(
        shell,
        split.left,
        SplitSide::Left,
        split.focused == SplitSide::Left,
        cx,
    );
    let right_top_pane = render_split_pane(
        shell,
        split.right.unwrap(),
        SplitSide::Right,
        split.focused == SplitSide::Right,
        cx,
    );

    let left_col = render_split_column(
        shell,
        left_width,
        split_height_left,
        left_top_height,
        pane_min_height,
        max_top_height,
        left_top_pane,
        split.bottom_left.map(|v| {
            render_split_pane(
                shell,
                v,
                SplitSide::BottomLeft,
                split.focused == SplitSide::BottomLeft,
                cx,
            )
        }),
        SplitSide::BottomLeft,
        true,
    );
    let right_col = render_split_column(
        shell,
        0.0, // flex_1 宽度由父 flex 决定，这里传 0 仅占位，内部按 flex_1 处理
        split_height_right,
        right_top_height,
        pane_min_height,
        max_top_height,
        right_top_pane,
        split.bottom_right.map(|v| {
            render_split_pane(
                shell,
                v,
                SplitSide::BottomRight,
                split.focused == SplitSide::BottomRight,
                cx,
            )
        }),
        SplitSide::BottomRight,
        false,
    );

    div()
        .id("terminal-split")
        .size_full()
        .relative()
        .flex()
        .flex_row()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .child(left_col)
        .child(right_col)
        .child(render_terminal_split_resizer(
            shell,
            split_width,
            left_width,
            pane_min_width,
            max_left_width,
        ))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_single_column_vertical(
    shell: &AppShell,
    split: TerminalSplitState,
    bottom_view: ActiveView,
    top_height: f32,
    height_cell: Rc<Cell<f32>>,
    min_height: f32,
    max_height: f32,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let top = render_split_pane(
        shell,
        split.left,
        SplitSide::Left,
        split.focused == SplitSide::Left,
        cx,
    );
    let bottom = render_split_pane(
        shell,
        bottom_view,
        SplitSide::BottomLeft,
        split.focused == SplitSide::BottomLeft,
        cx,
    );
    div()
        .id("terminal-split-vertical-single")
        .size_full()
        .relative()
        .flex()
        .flex_col()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .child(div().h(px(top_height)).w_full().flex_shrink_0().child(top))
        .child(div().flex_1().min_h_0().w_full().child(bottom))
        .child(render_vertical_split_resizer(
            shell,
            height_cell,
            top_height,
            min_height,
            max_height,
            SplitSide::BottomLeft,
        ))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_split_column(
    shell: &AppShell,
    width: f32,
    height_cell: Rc<Cell<f32>>,
    top_height: f32,
    min_height: f32,
    max_height: f32,
    top_pane: AnyElement,
    bottom_pane: Option<AnyElement>,
    bottom_side: SplitSide,
    is_left: bool,
) -> AnyElement {
    let mut col = div()
        .h_full()
        .flex()
        .flex_col()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .relative();
    col = if is_left {
        col.w(px(width)).flex_shrink_0()
    } else {
        col.flex_1().min_w_0()
    };
    if let Some(bottom) = bottom_pane {
        col = col
            .child(
                div()
                    .h(px(top_height))
                    .w_full()
                    .flex_shrink_0()
                    .child(top_pane),
            )
            .child(div().flex_1().min_h_0().w_full().child(bottom))
            .child(render_vertical_split_resizer(
                shell,
                height_cell,
                top_height,
                min_height,
                max_height,
                bottom_side,
            ));
    } else {
        col = col.child(div().flex_1().min_h_0().w_full().child(top_pane));
    }
    col.into_any_element()
}

fn render_terminal_split_resizer(
    shell: &AppShell,
    split_width: Rc<Cell<f32>>,
    left_width: f32,
    min_width: f32,
    max_width: f32,
) -> impl IntoElement {
    div()
        .relative()
        .absolute()
        .left_0()
        .top_0()
        .w(px(left_width))
        .h_full()
        .child(
            SplitResizer::new(
                "terminal-split-resizer",
                shell.terminal_split_dragging.clone(),
                split_width,
            )
            .min_size(min_width)
            .max_size(max_width)
            .line(),
        )
}

fn render_vertical_split_resizer(
    shell: &AppShell,
    split_height: Rc<Cell<f32>>,
    top_height: f32,
    min_height: f32,
    max_height: f32,
    side: SplitSide,
) -> impl IntoElement {
    let dragging = match side {
        SplitSide::BottomRight => shell.terminal_split_vertical_right_dragging.clone(),
        _ => shell.terminal_split_vertical_dragging.clone(),
    };
    let id: &'static str = match side {
        SplitSide::BottomRight => "terminal-split-vertical-resizer-right",
        SplitSide::BottomLeft => "terminal-split-vertical-resizer-left",
        _ => "terminal-split-vertical-resizer-single",
    };
    div()
        .relative()
        .absolute()
        .left_0()
        .top_0()
        .w_full()
        .h(px(top_height))
        .child(
            SplitResizer::new(id, dragging, split_height)
                .min_size(min_height)
                .max_size(max_height)
                .vertical()
                .line(),
        )
}

fn render_split_pane(
    shell: &AppShell,
    view: ActiveView,
    side: SplitSide,
    focused: bool,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let content = render_workspace_view(shell, view)
        .unwrap_or_else(|| div().size_full().bg(theme::canvas()).into_any_element());
    let id = match side {
        SplitSide::Left => "terminal-split-left",
        SplitSide::Right => "terminal-split-right",
        SplitSide::BottomLeft => "terminal-split-bottom-left",
        SplitSide::BottomRight => "terminal-split-bottom-right",
    };
    div()
        .id(id)
        .relative()
        .size_full()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .border_t_1()
        .border_color(if focused {
            theme::accent()
        } else {
            theme::border()
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _event, _window, cx| {
                this.focus_terminal_split(side, cx);
                cx.stop_propagation();
            }),
        )
        .child(content)
        .into_any_element()
}

fn terminal_split_available(width: Pixels) -> bool {
    width.as_f32() >= TERMINAL_SPLIT_MIN_PANE_WIDTH * 2.0 + TERMINAL_SPLIT_HANDLE_WIDTH
}

fn terminal_split_left_width(requested: f32, default: f32, min_width: f32, max_width: f32) -> f32 {
    split_clamped_size(requested, default, min_width, max_width)
}

fn terminal_split_top_height(
    requested: f32,
    default: f32,
    min_height: f32,
    max_height: f32,
) -> f32 {
    split_clamped_size(requested, default, min_height, max_height)
}

fn render_workspace_terminal_toggle(
    shell: &AppShell,
    available_width: Pixels,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let terminal_active = shell
        .workspace
        .focused_view()
        .is_some_and(|view| workspace_view_has_terminal(shell, view));
    if !terminal_active {
        return div().h(px(0.)).flex_none().into_any_element();
    }

    let split = shell.workspace.active_split();
    let has_horizontal = split.is_some_and(|s| s.right.is_some());
    // 上下分栏是按列独立的：选中态只反映当前聚焦列是否存在底部格
    let has_vertical = split.is_some_and(|s| {
        let is_right = s.focused.is_right_column() && s.right.is_some();
        if is_right {
            s.bottom_right.is_some()
        } else {
            s.bottom_left.is_some()
        }
    });
    let can_toggle_horizontal = has_horizontal || terminal_split_available(available_width);
    let can_vertical = shell.workspace.can_add_vertical() || has_vertical;
    let horizontal_tooltip = if has_horizontal {
        "tooltip.close_split"
    } else if can_toggle_horizontal {
        "tooltip.split_terminal"
    } else {
        "tooltip.split_terminal_narrow"
    };
    let vertical_tooltip = if has_vertical {
        "tooltip.close_vertical_split"
    } else if can_vertical {
        "tooltip.split_terminal_vertical"
    } else {
        "tooltip.split_terminal_vertical_full"
    };
    div()
        .flex()
        .flex_row()
        .gap_1()
        .child(
            Button::new("terminal-split-toggle")
                .size(ButtonSize::Icon(px(22.)))
                .variant(ButtonVariant::Ghost)
                .selected(has_horizontal)
                .disabled(!can_toggle_horizontal)
                .icon(
                    icons::icon(icons::IconName::Columns2, 13.).text_color(if has_horizontal {
                        theme::accent()
                    } else {
                        theme::muted_text()
                    }),
                )
                .tooltip(i18n::text(horizontal_tooltip))
                .on_click(cx.listener(|this, _event, window, cx| {
                    this.toggle_terminal_split(window, cx);
                })),
        )
        .child(
            Button::new("terminal-vertical-split-toggle")
                .size(ButtonSize::Icon(px(22.)))
                .variant(ButtonVariant::Ghost)
                .selected(has_vertical)
                .disabled(!can_vertical)
                .icon(
                    icons::icon(icons::IconName::Rows2, 13.).text_color(if has_vertical {
                        theme::accent()
                    } else {
                        theme::muted_text()
                    }),
                )
                .tooltip(i18n::text(vertical_tooltip))
                .on_click(cx.listener(|this, _event, window, cx| {
                    this.toggle_vertical_split(window, cx);
                })),
        )
        .into_any_element()
}

fn workspace_view_has_terminal(shell: &AppShell, view: ActiveView) -> bool {
    match view {
        ActiveView::LocalSession(session_id) => shell
            .workspace
            .sessions
            .local_sessions
            .contains_key(&session_id),
    }
}

fn copy_status_path_to_clipboard(path: &Path, cx: &App) {
    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
        path.to_string_lossy().into_owned(),
    ));
}

/// 「在外部编辑器中打开」状态栏按钮；仅本地会话渲染，目标为该会话当前 `cwd`。
fn render_open_in_editor_button(
    session: &LocalSession,
    shell: &AppShell,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let directory = session.cwd.clone();
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
    let focused_view = shell.workspace.focused_view();
    let terminal_active = focused_view.is_some_and(|view| workspace_view_has_terminal(shell, view));
    let mut left = div()
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
        ));
    if terminal_active {
        // 按终端独立：状态栏钟表反映当前焦点终端的实际状态（分屏时随聚焦侧变化）。
        let show_timestamps = focused_view
            .and_then(|view| match view {
                ActiveView::LocalSession(session_id) => shell
                    .workspace
                    .sessions
                    .local_sessions
                    .get(&session_id)
                    .map(|session| session.terminal.read(cx).show_timestamps()),
            })
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
    }
    {
        let compose_active = shell.workspace.compose_visible_for_focused();
        let compose_disabled = focused_view.is_none();
        left = left.child(
            Button::new("status-compose")
                .size(ButtonSize::Icon(px(22.)))
                .variant(ButtonVariant::Ghost)
                .selected(compose_active)
                .disabled(compose_disabled)
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
    }

    if let Some(ActiveView::LocalSession(session_id)) = focused_view
        && let Some(session) = shell.workspace.sessions.local_sessions.get(&session_id)
    {
        left = left.child(render_open_in_editor_button(session, shell, cx));
        left = left.child(render_status_path(session.cwd.clone(), cx));
        if let Some(status) = &session.git_status {
            let sync = shell.git_sync.get(&session_id);
            left = left.child(render_git_status(status, session, session_id, sync, cx));
        }
    }

    let right = div()
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
                    if let Err(error) = crate::features::note_launcher::spawn_note_process() {
                        log::warn!("spawn note failed: {error}");
                    }
                }))
                .into_any_element(),
        );

    StatusBar::new("workspace-status-bar")
        .child(left)
        .child(right)
        .into_any_element()
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
    session: &LocalSession,
    session_id: LocalSessionId,
    sync: Option<&GitSyncState>,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let click_cwd = session.cwd.clone();
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
            cx.new(|_| Tooltip::new(crate::shared::i18n::text("git.title")))
                .into()
        })
        .child(icons::icon(icons::IconName::GitBranch, 13.).text_color(theme::accent()))
        .child(
            div()
                .text_color(theme::text())
                .child(SharedString::from(status.branch.clone())),
        )
        .on_click(cx.listener(move |_this, _ev, _window, _cx| {
            if let Err(error) = crate::features::git_launcher::spawn_git_process(&click_cwd) {
                log::error!(
                    "failed to start crossh-git for {}: {error}",
                    click_cwd.display()
                );
            }
        }));

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
                session_id,
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
                session_id,
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
    session_id: LocalSessionId,
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
            this.run_git_sync(session_id, operation, cx);
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
    _shell: &mut AppShell,
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
        shell,
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
        AppShell::submit_rename_local_session,
        AppShell::cancel_rename_local_session,
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
        shell,
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
    use gpui::{ClipboardEntry, TestAppContext};

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

    #[test]
    fn terminal_split_width_preserves_dragged_values_on_both_sides_of_default() {
        assert_eq!(terminal_split_left_width(0., 500., 160., 940.), 500.);
        assert_eq!(terminal_split_left_width(220., 500., 160., 940.), 220.);
        assert_eq!(terminal_split_left_width(760., 500., 160., 940.), 760.);
    }
}

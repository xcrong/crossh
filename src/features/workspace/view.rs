//! 工作区：标签条 + 终端/SFTP/转发主区，以及会话/标签的数据类型。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, App, AppContext, ClickEvent, Context, Entity, FontWeight, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::features::connections::Connection;
use crate::features::editor_launcher;
use crate::features::settings::is_settings_window_open;
use crate::features::terminal::{ConnState, TerminalView};
use crate::features::workspace::empty_state;
use crate::features::workspace::pane::WorkspacePane;
use crate::features::workspace::registry::{SplitSide, TerminalSplitState};
use crate::features::workspace::shell::{AppShell, GitSyncOperation, GitSyncState};
use crate::features::workspace::status::{background_task_color, background_task_label};
use crate::features::workspace::tab_strip;
use crate::features::workspace::toaster::{ToastNotice, ToastTone};
use crate::shared::i18n;
use crossh_core::commands::{BackgroundTask, BackgroundTaskStatus, CommandRecord};
use crossh_core::git_status::GitStatus;
use crossh_ui::context_menu::{MenuEntry, MenuItem, ShellMenuAction};
use crossh_ui::widgets::{ime_input_canvas, marked_text_span, text_caret, text_span};
use crossh_ui::{icons, theme};
use crossh_ui_component::{
    BadgeTone, Button, ButtonSize, ButtonVariant, CountBadge, ModalDialog, SidePanel, SplitResizer,
    StatusBar, StatusDot, StatusMetric, Tooltip,
};

const TERMINAL_SPLIT_MIN_PANE_WIDTH: f32 = 160.0;
const TERMINAL_SPLIT_HANDLE_WIDTH: f32 = 8.0;

/// 一个远程终端/SFTP 标签。
pub struct Tab {
    /// 重新打开终端时使用的原始目标（别名或 user@host:port）。
    pub target: String,
    pub host_key: String,
    /// SFTP/forward tabs keep the russh connection. Zed-backed terminal tabs
    /// use Zed's PTY/SSH process directly and leave this empty.
    pub connection: Option<Entity<Connection>>,
    pub pane: Box<dyn WorkspacePane>,
}

pub type LocalSessionId = u64;

/// 当前主区正在展示的工作区。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ActiveView {
    RemoteTab(usize),
    LocalSession(LocalSessionId),
}

pub struct LocalSession {
    /// 创建会话时所属的项目目录；shell 内 `cd` 不会改变它。
    pub project_dir: PathBuf,
    /// shell 当前工作目录；可以独立于项目归属变化。
    pub cwd: PathBuf,
    pub terminal: Entity<TerminalView>,
    pub git_status: Option<GitStatus>,
    pub git_refresh: GitStatusRefresh,
    /// 固定状态：持久化 `pin_id`（`Some` 即固定，与设置中记录一一对应）。
    pub pin_id: Option<u64>,
    /// 固定标签的自定义名称；覆盖终端标题显示，重启后保留。
    pub custom_name: Option<String>,
}

/// 每个本地会话最多运行一个 Git 状态查询；期间的刷新请求只合并为一次后续检查。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GitStatusRefresh {
    in_flight: bool,
    pending: bool,
}

impl GitStatusRefresh {
    pub fn request(&mut self) -> bool {
        if self.in_flight {
            self.pending = true;
            false
        } else {
            self.in_flight = true;
            true
        }
    }

    pub fn finish(&mut self) -> bool {
        self.in_flight = false;
        std::mem::take(&mut self.pending)
    }
}

pub struct LocalDir {
    /// 侧栏分组对应的项目目录。
    pub project_dir: PathBuf,
    pub sessions: Vec<LocalSessionId>,
    pub active_session: Option<LocalSessionId>,
}

/// 主区：标签条 + 内容区。
pub fn render_main(
    shell: &mut AppShell,
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

    // 标签条。
    pane = pane.child(tab_strip::render_tab_strip(shell, cx));

    // 终端/SFTP 区。
    let mut content = div().flex_1().min_h_0().flex().relative();
    let mut terminal_area = div().flex_1().min_w_0().min_h_0().relative();
    // 分栏跟随其属主 Tab：只有属主 Tab 正被展示时才渲染分栏；否则
    // 分栏状态保留，渲染当前活动 Tab 的普通视图，切回属主即恢复。
    if let Some(split) = shell.workspace.active_split() {
        terminal_area =
            terminal_area.child(render_terminal_split(shell, split, available_width, cx));
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
        ActiveView::RemoteTab(index) => shell
            .workspace
            .sessions
            .remote_tabs
            .get(index)
            .map(|tab| tab.pane.render()),
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
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let available_width = available_width.as_f32().max(0.0);
    let has_room = terminal_split_available(px(available_width));
    let pane_min_width = if has_room {
        TERMINAL_SPLIT_MIN_PANE_WIDTH
    } else {
        0.0
    };
    let max_left_width = if has_room {
        (available_width - TERMINAL_SPLIT_HANDLE_WIDTH - pane_min_width).max(pane_min_width)
    } else {
        (available_width - TERMINAL_SPLIT_HANDLE_WIDTH).max(0.0)
    };
    let default_left_width = ((available_width - TERMINAL_SPLIT_HANDLE_WIDTH) / 2.0).max(0.0);
    let left_width = terminal_split_left_width(
        shell.terminal_split_width.get(),
        default_left_width,
        pane_min_width,
        max_left_width,
    );
    let left = div()
        .relative()
        .w(px(left_width))
        .h_full()
        .flex_shrink_0()
        .border_r_1()
        .border_color(theme::border_strong())
        .child(render_split_pane(
            shell,
            split.left,
            SplitSide::Left,
            split.focused == SplitSide::Left,
            cx,
        ));
    let right = div().h_full().flex_1().min_w_0().child(render_split_pane(
        shell,
        split.right,
        SplitSide::Right,
        split.focused == SplitSide::Right,
        cx,
    ));

    div()
        .id("terminal-split")
        .size_full()
        .relative()
        .flex()
        .flex_row()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .child(left)
        .child(right)
        .child(render_terminal_split_resizer(
            shell,
            left_width,
            pane_min_width,
            max_left_width,
        ))
        .into_any_element()
}

fn render_terminal_split_resizer(
    shell: &AppShell,
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
                shell.terminal_split_width.clone(),
            )
            .min_width(min_width)
            .max_width(max_width)
            .line(),
        )
}

fn render_split_pane(
    shell: &mut AppShell,
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
    if requested <= 0.0 {
        default.clamp(min_width, max_width)
    } else {
        requested.clamp(min_width, max_width)
    }
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

    // 分栏按钮的亮灭与当前 Tab（活动视图属主）绑定：显示「当前 Tab 是否
    // 有分栏」，其他 Tab 的分栏不影响本 Tab 的按钮状态（ADR 0011）。
    let split_active = shell.workspace.active_split().is_some();
    let can_toggle = split_active || terminal_split_available(available_width);
    let tooltip = if split_active {
        "tooltip.close_split"
    } else if can_toggle {
        "tooltip.split_terminal"
    } else {
        "tooltip.split_terminal_narrow"
    };
    Button::new("terminal-split-toggle")
        .size(ButtonSize::Icon(px(22.)))
        .variant(ButtonVariant::Ghost)
        .selected(split_active)
        .disabled(!can_toggle)
        .icon(
            icons::icon(icons::IconName::Columns2, 13.).text_color(if split_active {
                theme::accent()
            } else {
                theme::muted_text()
            }),
        )
        .tooltip(i18n::text(tooltip))
        .on_click(cx.listener(|this, _event, window, cx| {
            this.toggle_terminal_split(window, cx);
        }))
        .into_any_element()
}

fn workspace_view_has_terminal(shell: &AppShell, view: ActiveView) -> bool {
    match view {
        ActiveView::LocalSession(session_id) => shell
            .workspace
            .sessions
            .local_sessions
            .contains_key(&session_id),
        ActiveView::RemoteTab(index) => shell
            .workspace
            .sessions
            .remote_tabs
            .get(index)
            .and_then(|tab| tab.pane.terminal_entity_id())
            .is_some(),
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
    let editor = editor_launcher::resolve_editor(
        shell.workspace_settings.editor_command.as_deref(),
        std::env::var_os("PATH")
            .as_deref()
            .unwrap_or_else(|| std::ffi::OsStr::new("")),
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
        left = left.child(render_status_bar_toggle(
            "status-timestamps",
            icons::IconName::Clock,
            "tooltip.timestamps",
            shell.terminal_settings.show_timestamps,
            AppShell::toggle_timestamps,
            cx,
        ));
        left = left.child(render_workspace_terminal_toggle(shell, available_width, cx));
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

    StatusBar::new("workspace-status-bar")
        .child(left)
        .child(render_status_bar_toggle(
            "status-quick-commands",
            icons::IconName::PanelRight,
            "tooltip.quick_commands",
            shell.workspace_settings.show_quick_commands,
            AppShell::toggle_quick_commands,
            cx,
        ))
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
        git = git.child(status_metric(format!("↑{}", status.ahead), BadgeTone::Info));
    }
    if status.behind > 0 {
        git = git.child(status_metric(
            format!("↓{}", status.behind),
            BadgeTone::Info,
        ));
    }
    if status.staged > 0 {
        git = git.child(status_metric(
            format!("+{}", status.staged),
            BadgeTone::Accent,
        ));
    }
    if status.modified > 0 {
        git = git.child(status_metric(
            format!("~{}", status.modified),
            BadgeTone::Warning,
        ));
    }
    if status.untracked > 0 {
        git = git.child(status_metric(
            format!("?{}", status.untracked),
            BadgeTone::Neutral,
        ));
    }
    if status.conflicts > 0 {
        git = git.child(status_metric(
            format!("!{}", status.conflicts),
            BadgeTone::Danger,
        ));
    }
    if status.is_clean() {
        git = git.child(status_metric(i18n::text("git.clean"), BadgeTone::Success));
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

pub(crate) fn render_quick_commands(
    shell: &mut AppShell,
    scope: String,
    cwd: String,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let records = shell.command_history.top(&scope);
    let total = shell.command_history.total(&scope);
    let tasks = shell.background_tasks.tasks_for_scope(&scope);

    let header = div()
        .h(px(50.))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .justify_center()
        .gap_1()
        .px_3()
        .bg(theme::surface())
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::text())
                .child(icons::icon(icons::IconName::Terminal, 13.).text_color(theme::accent()))
                .child(SharedString::from(i18n::text("quick_commands.title")))
                .child(
                    div().ml_auto().child(
                        CountBadge::new(format!("{}/{}", records.len(), total))
                            .unbounded()
                            .padding_x(px(8.))
                            .padding_y(px(1.)),
                    ),
                ),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(theme::faint_text())
                .child(SharedString::from(cwd)),
        );

    let mut list = div()
        .id("quick-commands-list")
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap_1()
        .p_2();
    list.style().overflow.y = Some(gpui::Overflow::Scroll);
    if records.is_empty() {
        list = list.child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .px_3()
                .text_xs()
                .text_color(theme::faint_text())
                .child(SharedString::from(i18n::text("quick_commands.empty"))),
        );
    } else {
        for (index, record) in records.iter().enumerate() {
            list = list.child(render_quick_command_row(shell, &scope, record, index, cx));
        }
    }

    let task_section = (!tasks.is_empty()).then(|| {
        let mut section = div()
            .id("quick-commands-tasks")
            .flex_shrink_0()
            .max_h(px(180.))
            .flex()
            .flex_col()
            .gap_1()
            .border_t_1()
            .border_color(theme::border())
            .bg(theme::canvas())
            .p_2();
        section.style().overflow.y = Some(gpui::Overflow::Scroll);
        section = section.child(
            div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_2()
                .px_1()
                .text_xs()
                .text_color(theme::faint_text())
                .child(icons::icon(icons::IconName::Clock, 12.).text_color(theme::warning()))
                .child(SharedString::from(i18n::text(
                    "quick_commands.background_tasks",
                )))
                .child(
                    div().ml_auto().child(
                        CountBadge::new(tasks.len().to_string())
                            .unbounded()
                            .padding_x(px(4.))
                            .padding_y(px(0.)),
                    ),
                ),
        );
        for task in tasks {
            section = section.child(render_background_task_row(&task, cx));
        }
        section.into_any_element()
    });

    SidePanel::right(
        "quick-commands-resize",
        shell.quick_commands_width.clone(),
        shell.quick_commands_dragging.clone(),
    )
    .min_width(theme::QUICK_COMMANDS_MIN_WIDTH)
    .max_width(theme::QUICK_COMMANDS_MAX_WIDTH)
    .bg(theme::surface())
    .border_color(theme::border())
    .line()
    .child(header)
    .child(list)
    .children(task_section)
    .into_any_element()
}

fn render_quick_command_row(
    shell: &AppShell,
    scope: &str,
    record: &CommandRecord,
    index: usize,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let command = record.command.clone();
    let active_ids = shell.background_tasks.active_for_command(scope, &command);
    let running_id = shell
        .background_tasks
        .running_for_command(scope, &command)
        .first()
        .copied();
    let command_for_click = command.clone();
    let scope_for_click = scope.to_string();
    let run_scope = scope.to_string();
    let run_command = command.clone();
    let background_scope = scope.to_string();
    let background_command = command.clone();
    let background_restart_id = running_id;
    let background_can_start = active_ids.is_empty();
    let pin_scope = scope.to_string();
    let pin_command = command.clone();
    let row = div()
        .id(SharedString::from(format!("quick-command-row-{index}")))
        .min_h(px(32.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .hover(|style| style.bg(theme::raised()))
        .on_click(cx.listener(move |this, ev: &ClickEvent, _window, cx| {
            if ev.click_count() == 2 {
                this.run_quick_command(
                    scope_for_click.clone(),
                    command_for_click.clone(),
                    false,
                    cx,
                );
            }
        }))
        .on_mouse_down(MouseButton::Right, {
            let menu_scope = scope.to_string();
            let menu_command = command.clone();
            let menu_active_id = active_ids.first().copied();
            let menu_running_id = running_id;
            cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                let mut entries = vec![MenuEntry::Item(MenuItem {
                    id: "quick-run".into(),
                    label: i18n::text("quick_commands.run"),
                    shortcut_hint: None,
                    disabled: false,
                    danger: false,
                    action: ShellMenuAction::RunQuickCommand {
                        scope: menu_scope.clone(),
                        command: menu_command.clone(),
                        background: false,
                    },
                })];
                if let Some(id) = menu_running_id {
                    entries.push(MenuEntry::Item(MenuItem {
                        id: "quick-restart".into(),
                        label: i18n::text("quick_commands.restart"),
                        shortcut_hint: None,
                        disabled: false,
                        danger: false,
                        action: ShellMenuAction::RestartBackgroundTask(id),
                    }));
                } else if menu_active_id.is_none() {
                    entries.push(MenuEntry::Item(MenuItem {
                        id: "quick-background".into(),
                        label: i18n::text("quick_commands.run_background"),
                        shortcut_hint: None,
                        disabled: false,
                        danger: false,
                        action: ShellMenuAction::RunQuickCommand {
                            scope: menu_scope.clone(),
                            command: menu_command.clone(),
                            background: true,
                        },
                    }));
                }
                entries.push(MenuEntry::Separator);
                entries.extend([
                    MenuEntry::Item(MenuItem {
                        id: "quick-edit".into(),
                        label: i18n::text("quick_commands.edit"),
                        shortcut_hint: None,
                        disabled: false,
                        danger: false,
                        action: ShellMenuAction::EditQuickCommand {
                            scope: menu_scope.clone(),
                            command: menu_command.clone(),
                        },
                    }),
                    MenuEntry::Item(MenuItem {
                        id: "quick-copy".into(),
                        label: i18n::text("context_menu.copy"),
                        shortcut_hint: None,
                        disabled: false,
                        danger: false,
                        action: ShellMenuAction::CopyText(menu_command.clone()),
                    }),
                    MenuEntry::Item(MenuItem {
                        id: "quick-delete".into(),
                        label: i18n::text("quick_commands.delete"),
                        shortcut_hint: None,
                        disabled: false,
                        danger: true,
                        action: ShellMenuAction::DeleteQuickCommand {
                            scope: menu_scope.clone(),
                            command: menu_command.clone(),
                        },
                    }),
                    MenuEntry::Item(MenuItem {
                        id: "quick-ignore".into(),
                        label: i18n::text("quick_commands.ignore"),
                        shortcut_hint: None,
                        disabled: false,
                        danger: true,
                        action: ShellMenuAction::IgnoreQuickCommand {
                            scope: menu_scope.clone(),
                            command: menu_command.clone(),
                        },
                    }),
                ]);
                if let Some(id) = menu_active_id {
                    entries.push(MenuEntry::Separator);
                    entries.push(MenuEntry::Item(MenuItem {
                        id: "quick-stop".into(),
                        label: i18n::text("quick_commands.stop"),
                        shortcut_hint: None,
                        disabled: false,
                        danger: true,
                        action: ShellMenuAction::StopBackgroundTask(id),
                    }));
                }
                this.open_context_menu(ev.position, entries, cx);
            })
        })
        .child(icons::icon(icons::IconName::Terminal, 12.).text_color(theme::faint_text()))
        .child(render_command_preview(
            &command,
            theme::text(),
            SharedString::from(format!("quick-command-preview-{index}")),
        ))
        .child(
            Button::new(SharedString::from(format!("quick-run-{index}")))
                .size(ButtonSize::Icon(px(20.)))
                .variant(ButtonVariant::Ghost)
                .icon(icons::icon(icons::IconName::Play, 12.).text_color(theme::faint_text()))
                .tooltip(i18n::text("quick_commands.run"))
                .on_click(cx.listener(move |this, _ev, _window, cx| {
                    this.run_quick_command(run_scope.clone(), run_command.clone(), false, cx);
                    cx.stop_propagation();
                })),
        )
        .child(
            Button::new(SharedString::from(format!("quick-run-background-{index}")))
                .size(ButtonSize::Icon(px(20.)))
                .variant(ButtonVariant::Ghost)
                .icon(
                    icons::icon(
                        if background_restart_id.is_some() {
                            icons::IconName::RefreshCw
                        } else {
                            icons::IconName::Clock
                        },
                        12.,
                    )
                    .text_color(if background_restart_id.is_some() {
                        theme::warning()
                    } else {
                        theme::faint_text()
                    }),
                )
                .tooltip(i18n::text(if background_restart_id.is_some() {
                    "quick_commands.restart"
                } else {
                    "quick_commands.run_background"
                }))
                .on_click(cx.listener(move |this, _ev, _window, cx| {
                    if let Some(id) = background_restart_id {
                        this.restart_background_task(id, cx);
                    } else if background_can_start {
                        this.run_quick_command(
                            background_scope.clone(),
                            background_command.clone(),
                            true,
                            cx,
                        );
                    }
                    cx.stop_propagation();
                })),
        )
        .child(
            Button::new(SharedString::from(format!("quick-pin-{index}")))
                .size(ButtonSize::Icon(px(20.)))
                .variant(ButtonVariant::Ghost)
                .icon(
                    icons::icon(icons::IconName::Pin, 12.).text_color(if record.pinned {
                        theme::accent()
                    } else {
                        theme::faint_text()
                    }),
                )
                .tooltip(i18n::text("tooltip.pin_command"))
                .on_click(cx.listener(move |this, _ev, _window, cx| {
                    this.toggle_quick_command_pin(pin_scope.clone(), pin_command.clone(), cx);
                    cx.stop_propagation();
                })),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_xs()
                .text_color(theme::faint_text())
                .child(SharedString::from(format!("x{}", record.count))),
        );

    row.into_any_element()
}

fn render_command_preview(command: &str, color: gpui::Rgba, id: SharedString) -> AnyElement {
    let tooltip_command = command.to_string();
    let mut preview = div()
        .id(id)
        .flex_1()
        .min_w_0()
        .text_xs()
        .text_color(color)
        .tooltip(move |_window, cx| {
            cx.new(|_| Tooltip::new(tooltip_command.clone()).wide())
                .into()
        });

    if let Some((head, tail)) = command_preview_parts(command) {
        preview = preview
            .flex()
            .items_center()
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .child(SharedString::from(head)),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .px_1()
                    .text_color(theme::faint_text())
                    .child(SharedString::from("...")),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .child(SharedString::from(tail)),
            );
    } else {
        preview = preview
            .truncate()
            .child(SharedString::from(command.to_string()));
    }
    preview.into_any_element()
}

fn command_preview_parts(command: &str) -> Option<(String, String)> {
    const PREVIEW_CHARS: usize = 72;
    let char_count = command.chars().count();
    if char_count <= PREVIEW_CHARS {
        return None;
    }

    let head_chars = PREVIEW_CHARS / 2;
    let tail_chars = PREVIEW_CHARS - head_chars;
    let head_end = command
        .char_indices()
        .nth(head_chars)
        .map(|(index, _)| index)
        .unwrap_or(command.len());
    let tail_start = command
        .char_indices()
        .nth(char_count - tail_chars)
        .map(|(index, _)| index)
        .unwrap_or(0);
    Some((
        command[..head_end].to_string(),
        command[tail_start..].to_string(),
    ))
}

fn render_background_task_row(task: &BackgroundTask, cx: &mut Context<AppShell>) -> AnyElement {
    let active = matches!(
        task.status,
        BackgroundTaskStatus::Running | BackgroundTaskStatus::Stopping
    );
    let cwd = task.cwd.to_string_lossy().to_string();
    let mut row = div()
        .id(SharedString::from(format!("background-task-{}", task.id)))
        .min_h(px(28.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .rounded(px(theme::RADIUS_SM))
        .bg(theme::raised())
        .tooltip(move |_window, cx| cx.new(|_| Tooltip::new(cwd.clone())).into())
        .child(StatusDot::new(background_task_color(task.status)))
        .child(render_command_preview(
            &task.command,
            theme::muted_text(),
            SharedString::from(format!("background-command-preview-{}", task.id)),
        ))
        .child(
            div()
                .flex_shrink_0()
                .text_xs()
                .text_color(background_task_color(task.status))
                .child(SharedString::from(background_task_label(task.status))),
        );
    if active {
        let id = task.id;
        row = row.child(
            Button::new(SharedString::from(format!("background-stop-{id}")))
                .size(ButtonSize::Icon(px(20.)))
                .variant(ButtonVariant::Ghost)
                .hover_background(theme::accent_soft())
                .icon(icons::icon(icons::IconName::CircleX, 12.).text_color(theme::danger()))
                .tooltip(i18n::text("quick_commands.stop"))
                .on_click(cx.listener(move |this, _ev, _window, cx| {
                    this.stop_background_task(id, cx);
                    cx.stop_propagation();
                })),
        );
    }
    row.into_any_element()
}

pub fn render_quick_command_editor(
    shell: &mut AppShell,
    window: &Window,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let Some(editor) = &shell.quick_command_editor else {
        return div().into_any_element();
    };
    let focus = editor.focus.clone();
    let value = editor.state.value.clone();
    let ime_marked_text = editor.state.ime_marked_text.clone();
    let selection = editor.state.selection();
    let (selection_start, selection_end) =
        selection.unwrap_or((editor.state.cursor, editor.state.cursor));
    let scroll = editor.scroll.clone();
    let focused = focus.is_focused(window);
    scroll.scroll_to_item(1);
    let mut input = div()
        .id("quick-command-editor-input")
        .w_full()
        .min_w_0()
        .min_h(px(38.))
        .px_3()
        .py_2()
        .flex()
        .items_center()
        .overflow_x_scroll()
        .track_scroll(&scroll)
        .bg(theme::canvas())
        .border_1()
        .border_color(theme::border_strong())
        .rounded(px(theme::RADIUS_SM))
        .relative()
        .text_sm()
        .text_color(theme::text())
        .track_focus(&focus)
        .tab_stop(true)
        .focus(|style| style.border_color(theme::focus_ring()))
        .on_click({
            let focus = focus.clone();
            move |_ev, window, cx| window.focus(&focus, cx)
        })
        .on_key_down(cx.listener(AppShell::handle_modal_editor_key));

    if value.is_empty() {
        if focused {
            input = input.child(text_caret(px(20.)));
        }
        if ime_marked_text.is_empty() {
            input = input.child(
                div()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .text_color(theme::faint_text())
                    .child(SharedString::from(i18n::text(
                        "quick_commands.command_placeholder",
                    ))),
            );
        } else {
            input = input.child(marked_text_span(ime_marked_text.clone()));
        }
    } else {
        input = input.child(text_span(value[..selection_start].to_string()));
        if let Some((start, end)) = selection {
            input = input.child(
                div()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .bg(theme::accent_soft())
                    .text_color(theme::text())
                    .child(SharedString::from(value[start..end].to_string())),
            );
        } else {
            if focused {
                input = input.child(text_caret(px(20.)));
            }
            if !ime_marked_text.is_empty() {
                input = input.child(marked_text_span(ime_marked_text.clone()));
            }
        }
        input = input.child(text_span(value[selection_end..].to_string()));
    }
    input = input.child(ime_input_canvas(focus, cx.entity()));

    let buttons = div()
        .flex()
        .flex_row()
        .gap_2()
        .child(
            Button::new("quick-command-editor-save")
                .size(ButtonSize::Medium)
                .variant(ButtonVariant::Primary)
                .icon(icons::icon(icons::IconName::Check, 13.).text_color(theme::canvas()))
                .label(i18n::text("quick_commands.save"))
                .on_click(cx.listener(|this, _ev, _window, cx| {
                    this.submit_quick_command_editor(cx);
                })),
        )
        .child(
            Button::new("quick-command-editor-cancel")
                .size(ButtonSize::Medium)
                .variant(ButtonVariant::Secondary)
                .icon(icons::icon(icons::IconName::X, 13.).text_color(theme::muted_text()))
                .label(i18n::text("prompt.cancel"))
                .on_click(cx.listener(|this, _ev, _window, cx| {
                    this.cancel_quick_command_editor(cx);
                })),
        );

    ModalDialog::new(
        i18n::text("quick_commands.edit_title"),
        icons::icon(icons::IconName::Pencil, 16.).text_color(theme::info()),
    )
    .width(px(500.))
    .scrim_id("quick-command-editor-scrim")
    .card_id("quick-command-editor-card")
    .blocks_card_clicks()
    .on_backdrop_click(cx.listener(|this, _ev, _window, cx| {
        this.cancel_quick_command_editor(cx);
    }))
    .child(input)
    .actions(buttons)
    .into_any_element()
}

/// 固定标签重命名弹窗：ModalDialog + 自绘文本输入（IME 支持），
/// 模式与 Quick Command 编辑器一致。
pub fn render_rename_editor(
    shell: &mut AppShell,
    window: &Window,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let Some(editor) = &shell.rename_editor else {
        return div().into_any_element();
    };
    let focus = editor.focus.clone();
    let value = editor.state.value.clone();
    let ime_marked_text = editor.state.ime_marked_text.clone();
    let selection = editor.state.selection();
    let (selection_start, selection_end) =
        selection.unwrap_or((editor.state.cursor, editor.state.cursor));
    let focused = focus.is_focused(window);
    let mut input = div()
        .id("rename-editor-input")
        .w_full()
        .min_w_0()
        .min_h(px(38.))
        .px_3()
        .py_2()
        .flex()
        .items_center()
        .bg(theme::canvas())
        .border_1()
        .border_color(theme::border_strong())
        .rounded(px(theme::RADIUS_SM))
        .relative()
        .text_sm()
        .text_color(theme::text())
        .track_focus(&focus)
        .tab_stop(true)
        .focus(|style| style.border_color(theme::focus_ring()))
        .on_click({
            let focus = focus.clone();
            move |_ev, window, cx| window.focus(&focus, cx)
        })
        .on_key_down(cx.listener(AppShell::handle_modal_editor_key));

    if value.is_empty() {
        if focused {
            input = input.child(text_caret(px(20.)));
        }
        if ime_marked_text.is_empty() {
            input = input.child(
                div()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .text_color(theme::faint_text())
                    .child(SharedString::from(i18n::text(
                        "rename_tab.name_placeholder",
                    ))),
            );
        } else {
            input = input.child(marked_text_span(ime_marked_text.clone()));
        }
    } else {
        input = input.child(text_span(value[..selection_start].to_string()));
        if let Some((start, end)) = selection {
            input = input.child(
                div()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .bg(theme::accent_soft())
                    .text_color(theme::text())
                    .child(SharedString::from(value[start..end].to_string())),
            );
        } else {
            if focused {
                input = input.child(text_caret(px(20.)));
            }
            if !ime_marked_text.is_empty() {
                input = input.child(marked_text_span(ime_marked_text.clone()));
            }
        }
        input = input.child(text_span(value[selection_end..].to_string()));
    }
    input = input.child(ime_input_canvas(focus, cx.entity()));

    let buttons = div()
        .flex()
        .flex_row()
        .gap_2()
        .child(
            Button::new("rename-editor-save")
                .size(ButtonSize::Medium)
                .variant(ButtonVariant::Primary)
                .icon(icons::icon(icons::IconName::Check, 13.).text_color(theme::canvas()))
                .label(i18n::text("rename_tab.save"))
                .on_click(cx.listener(|this, _ev, _window, cx| {
                    this.submit_rename_local_session(cx);
                })),
        )
        .child(
            Button::new("rename-editor-cancel")
                .size(ButtonSize::Medium)
                .variant(ButtonVariant::Secondary)
                .icon(icons::icon(icons::IconName::X, 13.).text_color(theme::muted_text()))
                .label(i18n::text("prompt.cancel"))
                .on_click(cx.listener(|this, _ev, _window, cx| {
                    this.cancel_rename_local_session(cx);
                })),
        );

    ModalDialog::new(
        i18n::text("rename_tab.title"),
        icons::icon(icons::IconName::Pencil, 16.).text_color(theme::info()),
    )
    .width(px(420.))
    .scrim_id("rename-editor-scrim")
    .card_id("rename-editor-card")
    .blocks_card_clicks()
    .on_backdrop_click(cx.listener(|this, _ev, _window, cx| {
        this.cancel_rename_local_session(cx);
    }))
    .child(input)
    .actions(buttons)
    .into_any_element()
}

fn status_metric(text: impl Into<SharedString>, tone: BadgeTone) -> AnyElement {
    StatusMetric::new(text).tone(tone).into_any_element()
}

/// 把会话按项目归属目录重建目录视图：同一项目的会话合并，保留上一次的活动会话。
/// `remembered` 是最近打开过的本地目录（无活动会话），合并进来后仍显示在侧栏。
pub fn rebuild_local_dirs(
    previous: &BTreeMap<PathBuf, LocalDir>,
    sessions: impl IntoIterator<Item = (LocalSessionId, PathBuf)>,
    remembered: impl IntoIterator<Item = PathBuf>,
    active_local_session: Option<LocalSessionId>,
) -> BTreeMap<PathBuf, LocalDir> {
    let mut next = BTreeMap::new();
    for project_dir in remembered {
        if !project_dir.is_dir() {
            continue;
        }
        next.entry(project_dir.clone()).or_insert_with(|| LocalDir {
            project_dir,
            sessions: Vec::new(),
            active_session: None,
        });
    }
    for (session_id, project_dir) in sessions {
        next.entry(project_dir.clone())
            .or_insert_with(|| LocalDir {
                project_dir,
                sessions: Vec::new(),
                active_session: None,
            })
            .sessions
            .push(session_id);
    }

    for (project_dir, dir) in &mut next {
        let previous_active = previous.get(project_dir).and_then(|old| old.active_session);
        dir.active_session = active_local_session
            .filter(|id| dir.sessions.contains(id))
            .or_else(|| previous_active.filter(|id| dir.sessions.contains(id)))
            .or_else(|| dir.sessions.first().copied());
    }
    next
}

/// 多会话目录取「最活跃」的状态用于侧栏/徽标。
pub fn preferred_state(left: ConnState, right: ConnState) -> ConnState {
    if state_priority(&right) > state_priority(&left) {
        right
    } else {
        left
    }
}

fn state_priority(state: &ConnState) -> u8 {
    match state {
        ConnState::Connected => 4,
        ConnState::Connecting => 3,
        ConnState::Error(_) => 2,
        ConnState::Closed => 1,
    }
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

    #[test]
    fn git_status_refresh_coalesces_overlapping_requests() {
        let mut refresh = GitStatusRefresh::default();

        assert!(refresh.request());
        assert!(!refresh.request());
        assert!(!refresh.request());
        assert!(refresh.finish());
        assert!(refresh.request());
        assert!(!refresh.finish());
    }

    #[test]
    fn project_directories_keep_sessions_isolated() {
        let previous = BTreeMap::from([
            (
                PathBuf::from("/Users/me/one"),
                LocalDir {
                    project_dir: PathBuf::from("/Users/me/one"),
                    sessions: vec![1, 2],
                    active_session: Some(2),
                },
            ),
            (
                PathBuf::from("/Users/me/two"),
                LocalDir {
                    project_dir: PathBuf::from("/Users/me/two"),
                    sessions: vec![3],
                    active_session: Some(3),
                },
            ),
        ]);
        let current = vec![
            (1, PathBuf::from("/Users/me/one")),
            (2, PathBuf::from("/Users/me/two")),
            (3, PathBuf::from("/Users/me/two")),
        ];

        let dirs = rebuild_local_dirs(&previous, current, Vec::new(), Some(2));
        assert_eq!(dirs[&PathBuf::from("/Users/me/one")].sessions, vec![1]);
        assert_eq!(
            dirs[&PathBuf::from("/Users/me/one")].active_session,
            Some(1)
        );
        assert_eq!(dirs[&PathBuf::from("/Users/me/two")].sessions, vec![2, 3]);
        assert_eq!(
            dirs[&PathBuf::from("/Users/me/two")].active_session,
            Some(2)
        );
    }

    #[test]
    fn project_group_does_not_follow_a_session_cwd() {
        let project_dir = PathBuf::from("/Users/me/one");
        let dirs = rebuild_local_dirs(
            &BTreeMap::new(),
            vec![(1, project_dir.clone()), (2, project_dir.clone())],
            Vec::new(),
            Some(2),
        );

        assert_eq!(dirs[&project_dir].sessions, vec![1, 2]);
        assert_eq!(dirs[&project_dir].active_session, Some(2));
        assert!(!dirs.contains_key(&PathBuf::from("/Users/me/two")));
    }

    #[test]
    fn remembered_dirs_stay_without_live_sessions() {
        let previous = BTreeMap::new();
        let root =
            std::env::temp_dir().join(format!("crossh-remembered-dir-test-{}", std::process::id()));
        let one = root.join("one");
        let two = root.join("two");
        std::fs::create_dir_all(&one).expect("first test directory should be created");
        std::fs::create_dir_all(&two).expect("second test directory should be created");
        let remembered = vec![one.clone(), two.clone()];
        let current = vec![(1, one.clone())];

        let dirs = rebuild_local_dirs(&previous, current, remembered, Some(1));
        assert_eq!(dirs[&one].sessions, vec![1]);
        assert_eq!(dirs[&two].sessions, Vec::<LocalSessionId>::new());
        assert_eq!(dirs[&two].active_session, None);
        std::fs::remove_dir_all(root).expect("test directory should be removed");
    }

    #[test]
    fn spec_20260817_recent_local_dir_recovery_missing_remembered_dir_is_not_restored() {
        let root =
            std::env::temp_dir().join(format!("crossh-recent-dir-recovery-{}", std::process::id()));
        let existing = root.join("existing");
        let missing = root.join("missing");
        std::fs::create_dir_all(&existing).expect("test directory should be created");

        let dirs = rebuild_local_dirs(
            &BTreeMap::new(),
            Vec::new(),
            vec![existing.clone(), missing.clone()],
            None,
        );

        assert!(dirs.contains_key(&existing));
        assert!(!dirs.contains_key(&missing));
        std::fs::remove_dir_all(root).expect("test directory should be removed");
    }

    #[test]
    fn project_group_prefers_a_live_session_state() {
        assert_eq!(
            preferred_state(ConnState::Closed, ConnState::Connecting),
            ConnState::Connecting
        );
        assert_eq!(
            preferred_state(ConnState::Connecting, ConnState::Connected),
            ConnState::Connected
        );
        assert_eq!(
            preferred_state(ConnState::Connected, ConnState::Error("failed".into())),
            ConnState::Connected
        );
    }

    #[test]
    fn long_command_preview_keeps_both_ends() {
        let command = format!("{} --target /srv/release.tar.gz", "deploy ".repeat(20));
        let (head, tail) = command_preview_parts(&command).expect("long command preview");

        assert_eq!(head.chars().count(), 36);
        assert_eq!(tail.chars().count(), 36);
        assert!(tail.ends_with("release.tar.gz"));
        assert!(head.starts_with("deploy"));
        assert!(command_preview_parts("git status").is_none());
    }
}

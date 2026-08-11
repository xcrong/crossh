//! 工作区：标签条 + 终端/SFTP/转发主区，以及会话/标签的数据类型。

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    AnyElement, AppContext, Bounds, ClickEvent, Context, Entity, FontWeight, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
    SharedString, StatefulInteractiveElement, Styled, Window, canvas, div, px,
};

use crate::features::connections::Connection;
use crate::features::terminal::{ConnState, TerminalView};
use crate::features::workspace::pane::WorkspacePane;
use crate::features::workspace::shell::AppShell;
use crate::shared::i18n;
use crossh_core::commands::{
    BackgroundTask, BackgroundTaskStatus, CommandRecord, local_scope, remote_scope,
};
use crossh_core::project::GitStatus;
use crossh_ui::context_menu::{MenuEntry, MenuItem, ShellMenuAction};
use crossh_ui::widgets::{LocalPathTooltip, ime_input_canvas, text_caret};
use crossh_ui::{icons, theme};
use crossh_ui_component::{Avatar, AvatarKind};

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    pub git_refresh_generation: u64,
}

pub struct LocalDir {
    /// 侧栏分组对应的项目目录。
    pub project_dir: PathBuf,
    pub sessions: Vec<LocalSessionId>,
    pub active_session: Option<LocalSessionId>,
}

/// 主区：标签条 + 内容区。
pub fn render_main(shell: &mut AppShell, cx: &mut Context<AppShell>) -> AnyElement {
    let mut pane = div()
        .flex_1()
        .min_h_0()
        .h_full()
        .bg(theme::canvas())
        .flex()
        .flex_col();

    // 标签条。
    pane = pane.child(render_tab_strip(shell, cx));

    // 终端/SFTP 区。
    let mut content = div().flex_1().min_h_0().flex().relative();
    let active_pane = match shell.workspace.active_view {
        Some(ActiveView::RemoteTab(idx)) => shell
            .workspace
            .sessions
            .remote_tabs
            .get(idx)
            .map(|tab| tab.pane.render()),
        Some(ActiveView::LocalSession(session_id)) => shell
            .workspace
            .sessions
            .local_sessions
            .get(&session_id)
            .map(|session| session.terminal.clone().into_any_element()),
        None => None,
    };
    let quick_context =
        match shell.workspace.active_view {
            Some(ActiveView::LocalSession(session_id)) => shell
                .workspace
                .sessions
                .local_sessions
                .get(&session_id)
                .map(|session| {
                    let cwd = session
                        .terminal
                        .read(cx)
                        .cwd
                        .clone()
                        .unwrap_or_else(|| session.cwd.to_string_lossy().to_string());
                    let cwd = PathBuf::from(cwd);
                    (local_scope(&cwd), cwd.to_string_lossy().to_string())
                }),
            Some(ActiveView::RemoteTab(idx)) => shell
                .workspace
                .sessions
                .remote_tabs
                .get(idx)
                .and_then(|tab| {
                    tab.pane
                        .cwd(cx)
                        .map(|cwd| (remote_scope(&tab.host_key, &cwd), cwd))
                }),
            _ => None,
        };
    let mut terminal_area = div().flex_1().min_w_0().min_h_0().relative();
    if let Some(active_pane) = active_pane {
        terminal_area = terminal_area.child(active_pane);
    } else {
        terminal_area = terminal_area.child(render_empty_state(cx));
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
    if let Some((scope, cwd)) = quick_context {
        content = if shell.workspace_settings.show_quick_commands {
            content.child(render_quick_commands(shell, scope, cwd, cx))
        } else {
            content.child(render_quick_commands_rail(shell, &scope, cx))
        };
    }
    pane = pane.child(content);
    pane.into_any_element()
}

pub(crate) fn render_workspace_status_bar(
    shell: &AppShell,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let terminal_active = match shell.workspace.active_view {
        Some(ActiveView::LocalSession(_)) => true,
        Some(ActiveView::RemoteTab(index)) => shell
            .workspace
            .sessions
            .remote_tabs
            .get(index)
            .and_then(|tab| tab.pane.terminal_entity_id())
            .is_some(),
        None => false,
    };
    let mut left = div()
        .min_w_0()
        .flex()
        .items_center()
        .gap_1()
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
    }

    if let Some(ActiveView::LocalSession(session_id)) = shell.workspace.active_view
        && let Some(session) = shell.workspace.sessions.local_sessions.get(&session_id)
    {
        let cwd = session.cwd.to_string_lossy().to_string();
        left = left.child(
            div()
                .ml_2()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .truncate()
                .child(
                    icons::icon(icons::IconName::FolderOpen, 12.).text_color(theme::faint_text()),
                )
                .child(div().min_w_0().truncate().child(SharedString::from(cwd))),
        );
        if let Some(status) = &session.git_status {
            left = left.child(render_git_status(status, session, cx));
        }
    }

    div()
        .h(px(27.))
        .w_full()
        .flex_none()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .px_3()
        .border_t_1()
        .border_color(theme::border())
        .bg(theme::surface())
        .text_xs()
        .text_color(theme::muted_text())
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
    div()
        .id(id)
        .w(px(22.))
        .h(px(22.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .hover(|style| style.bg(theme::raised()))
        .tooltip(move |_window, cx| {
            cx.new(|_| LocalPathTooltip {
                path: SharedString::from(i18n::text(tooltip)),
            })
            .into()
        })
        .child(icons::icon(icon, 13.).text_color(if active {
            theme::accent()
        } else {
            theme::muted_text()
        }))
        .on_click(cx.listener(move |this, _ev, _window, cx| toggle(this, cx)))
        .into_any_element()
}

fn render_git_status(
    status: &GitStatus,
    session: &LocalSession,
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
            cx.new(|_| LocalPathTooltip {
                path: SharedString::from(crate::shared::i18n::text("git.title")),
            })
            .into()
        })
        .child(icons::icon(icons::IconName::GitBranch, 13.).text_color(theme::accent()))
        .child(
            div()
                .text_color(theme::text())
                .child(SharedString::from(status.branch.clone())),
        )
        .on_click(cx.listener(move |_this, _ev, _window, cx| {
            crate::features::git::open_git_window(click_cwd.clone(), cx);
        }));

    if status.ahead > 0 {
        git = git.child(status_badge(format!("↑{}", status.ahead), theme::info()));
    }
    if status.behind > 0 {
        git = git.child(status_badge(format!("↓{}", status.behind), theme::info()));
    }
    if status.staged > 0 {
        git = git.child(status_badge(format!("+{}", status.staged), theme::accent()));
    }
    if status.modified > 0 {
        git = git.child(status_badge(
            format!("~{}", status.modified),
            theme::warning(),
        ));
    }
    if status.untracked > 0 {
        git = git.child(status_badge(
            format!("?{}", status.untracked),
            theme::muted_text(),
        ));
    }
    if status.conflicts > 0 {
        git = git.child(status_badge(
            format!("!{}", status.conflicts),
            theme::danger(),
        ));
    }
    if status.is_clean() {
        git = git.child(status_badge(i18n::text("git.clean"), theme::accent()));
    }
    git.into_any_element()
}

fn render_quick_commands(
    shell: &mut AppShell,
    scope: String,
    cwd: String,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let width = shell.quick_commands_width.get().clamp(
        theme::QUICK_COMMANDS_MIN_WIDTH,
        theme::QUICK_COMMANDS_MAX_WIDTH,
    );
    let records = shell.command_history.top(&scope);
    let total = shell.command_history.total(&scope);
    let tasks = shell.background_tasks.tasks_for_scope(&scope);
    let container: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
    let backing = canvas(
        {
            let container = container.clone();
            move |bounds, _window, _cx| container.set(Some(bounds))
        },
        {
            let container = container.clone();
            let width_cell = shell.quick_commands_width.clone();
            let dragging = shell.quick_commands_dragging.clone();
            move |_bounds, _state, window, _cx| {
                window.on_mouse_event({
                    let container = container.clone();
                    let width_cell = width_cell.clone();
                    let dragging = dragging.clone();
                    move |ev: &MouseMoveEvent, _phase, window, _cx| {
                        if !dragging.get() {
                            return;
                        }
                        let Some(bounds) = container.get() else {
                            return;
                        };
                        let width = (bounds.right().as_f32() - ev.position.x.as_f32()).clamp(
                            theme::QUICK_COMMANDS_MIN_WIDTH,
                            theme::QUICK_COMMANDS_MAX_WIDTH,
                        );
                        width_cell.set(width);
                        window.refresh();
                    }
                });
                window.on_mouse_event({
                    let dragging = dragging.clone();
                    move |_ev: &MouseUpEvent, _phase, window, _cx| {
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

    let resizing = shell.quick_commands_dragging.get();
    let resize_handle = div()
        .id("quick-commands-resize")
        .absolute()
        .top_0()
        .left(px(-4.))
        .w(px(8.))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_col_resize()
        .child(
            div()
                .w(px(1.))
                .h_full()
                .bg(if resizing {
                    theme::accent()
                } else {
                    theme::border()
                })
                .hover(|style| style.bg(theme::accent())),
        )
        .on_mouse_down(MouseButton::Left, {
            let dragging = shell.quick_commands_dragging.clone();
            move |_ev, window, _cx| {
                dragging.set(true);
                window.refresh();
            }
        });

    let header = div()
        .h(px(50.))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .justify_center()
        .gap_1()
        .px_3()
        .border_b_1()
        .border_color(theme::border())
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
                    div()
                        .px_2()
                        .py(px(1.))
                        .rounded_full()
                        .bg(theme::raised())
                        .ml_auto()
                        .text_xs()
                        .text_color(theme::muted_text())
                        .child(SharedString::from(format!("{}/{}", records.len(), total))),
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

    let mut task_section = div()
        .id("quick-commands-tasks")
        .flex_shrink_0()
        .max_h(px(180.))
        .border_t_1()
        .border_color(theme::border())
        .bg(theme::canvas())
        .p_2();
    task_section.style().overflow.y = Some(gpui::Overflow::Scroll);
    if !tasks.is_empty() {
        task_section = task_section.child(
            div()
                .mb_1()
                .text_xs()
                .text_color(theme::faint_text())
                .child(SharedString::from(i18n::text(
                    "quick_commands.background_tasks",
                ))),
        );
        for task in tasks {
            task_section = task_section.child(render_background_task_row(&task, cx));
        }
    }

    let panel = div()
        .relative()
        .flex_shrink_0()
        .w(px(width))
        .h_full()
        .flex()
        .flex_col()
        .bg(theme::surface())
        .border_l_1()
        .border_color(theme::border())
        .child(backing)
        .child(header)
        .child(list)
        .child(task_section)
        .child(resize_handle);
    panel.into_any_element()
}

fn render_quick_commands_rail(
    shell: &AppShell,
    scope: &str,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let mut rail = div()
        .id("quick-commands-rail")
        .w(px(theme::QUICK_COMMANDS_RAIL_WIDTH))
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .items_center()
        .pt_2()
        .bg(theme::surface())
        .border_l_1()
        .border_color(theme::border());
    for (index, record) in shell.command_history.pinned(scope).iter().enumerate() {
        let command = record.command.clone();
        let run_scope = scope.to_string();
        let menu_scope = scope.to_string();
        let menu_command = command.clone();
        let tooltip_command = command.clone();
        rail = rail.child(
            div()
                .id(SharedString::from(format!("quick-command-rail-{index}")))
                .w(px(30.))
                .h(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::RADIUS_SM))
                .cursor_pointer()
                .hover(|style| style.bg(theme::raised()))
                .tooltip(move |_window, cx| {
                    cx.new(|_| crossh_ui::widgets::CommandTooltip {
                        command: SharedString::from(tooltip_command.clone()),
                    })
                    .into()
                })
                .child(Avatar::new(&command).kind(AvatarKind::Command))
                .on_click(cx.listener(move |this, ev: &ClickEvent, _window, cx| {
                    if ev.click_count() == 2 {
                        this.run_quick_command(run_scope.clone(), command.clone(), false, cx);
                    }
                }))
                .on_mouse_down(MouseButton::Right, {
                    cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                        this.open_context_menu(
                            ev.position,
                            vec![
                                MenuEntry::Item(MenuItem {
                                    id: "quick-run-background".into(),
                                    label: i18n::text("quick_commands.run_background"),
                                    shortcut_hint: None,
                                    disabled: false,
                                    danger: false,
                                    action: ShellMenuAction::RunQuickCommand {
                                        scope: menu_scope.clone(),
                                        command: menu_command.clone(),
                                        background: true,
                                    },
                                }),
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
                                    id: "quick-unpin".into(),
                                    label: i18n::text("quick_commands.unpin"),
                                    shortcut_hint: None,
                                    disabled: false,
                                    danger: false,
                                    action: ShellMenuAction::ToggleQuickCommandPin {
                                        scope: menu_scope.clone(),
                                        command: menu_command.clone(),
                                    },
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
                            ],
                            cx,
                        );
                    })
                }),
        );
    }
    rail.into_any_element()
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
    let command_for_click = command.clone();
    let scope_for_click = scope.to_string();
    let pin_scope = scope.to_string();
    let pin_command = command.clone();
    let mut row = div()
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
            cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                let mut entries = vec![
                    MenuEntry::Item(MenuItem {
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
                    }),
                    MenuEntry::Item(MenuItem {
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
                    }),
                    MenuEntry::Separator,
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
                ];
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
            div()
                .id(SharedString::from(format!("quick-pin-{index}")))
                .w(px(20.))
                .h(px(20.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::RADIUS_SM))
                .cursor_pointer()
                .hover(|style| style.bg(theme::raised()))
                .tooltip(|_window, cx| {
                    cx.new(|_| LocalPathTooltip {
                        path: SharedString::from(i18n::text("tooltip.pin_command")),
                    })
                    .into()
                })
                .child(
                    icons::icon(icons::IconName::Pin, 12.).text_color(if record.pinned {
                        theme::accent()
                    } else {
                        theme::faint_text()
                    }),
                )
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

    if let Some(id) = active_ids.first().copied() {
        row = row.child(
            div()
                .w(px(6.))
                .h(px(6.))
                .rounded_full()
                .bg(theme::warning()),
        );
        row = row.child(
            div()
                .id(SharedString::from(format!("quick-stop-{id}")))
                .w(px(20.))
                .h(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::RADIUS_SM))
                .cursor_pointer()
                .hover(|style| style.bg(theme::accent_soft()))
                .tooltip(|_window, cx| {
                    cx.new(|_| crossh_ui::widgets::LocalPathTooltip {
                        path: SharedString::from(i18n::text("quick_commands.stop")),
                    })
                    .into()
                })
                .child(icons::icon(icons::IconName::CircleX, 12.).text_color(theme::warning()))
                .on_click(cx.listener(move |this, _ev, _window, cx| {
                    this.stop_background_task(id, cx);
                    cx.stop_propagation();
                })),
        );
    }
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
            cx.new(|_| crossh_ui::widgets::CommandTooltip {
                command: SharedString::from(tooltip_command.clone()),
            })
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
        .bg(theme::surface())
        .tooltip(move |_window, cx| {
            cx.new(|_| crossh_ui::widgets::LocalPathTooltip {
                path: SharedString::from(cwd.clone()),
            })
            .into()
        })
        .child(
            div()
                .w(px(6.))
                .h(px(6.))
                .rounded_full()
                .bg(background_task_color(task.status)),
        )
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
            div()
                .id(SharedString::from(format!("background-stop-{id}")))
                .w(px(20.))
                .h(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|style| style.bg(theme::accent_soft()))
                .child(icons::icon(icons::IconName::CircleX, 12.).text_color(theme::danger()))
                .on_click(cx.listener(move |this, _ev, _window, cx| {
                    this.stop_background_task(id, cx);
                    cx.stop_propagation();
                })),
        );
    }
    row.into_any_element()
}

fn background_task_label(status: BackgroundTaskStatus) -> String {
    i18n::text(match status {
        BackgroundTaskStatus::Running => "quick_commands.running",
        BackgroundTaskStatus::Stopping => "quick_commands.stopping",
        BackgroundTaskStatus::Succeeded => "quick_commands.succeeded",
        BackgroundTaskStatus::Failed => "quick_commands.failed",
        BackgroundTaskStatus::Terminated => "quick_commands.terminated",
    })
}

fn background_task_color(status: BackgroundTaskStatus) -> gpui::Rgba {
    match status {
        BackgroundTaskStatus::Running => theme::warning(),
        BackgroundTaskStatus::Stopping | BackgroundTaskStatus::Terminated => theme::faint_text(),
        BackgroundTaskStatus::Succeeded => theme::accent(),
        BackgroundTaskStatus::Failed => theme::danger(),
    }
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
    let value = editor.value.clone();
    let ime_marked_text = editor.ime_marked_text.clone();
    let selection = editor.selection();
    let (selection_start, selection_end) = selection.unwrap_or((editor.cursor, editor.cursor));
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
        .on_key_down(cx.listener(AppShell::handle_quick_command_editor_key));

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
            input = input.child(
                div()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .underline()
                    .text_decoration_color(theme::accent())
                    .child(SharedString::from(ime_marked_text.clone())),
            );
        }
    } else {
        input = input.child(
            div()
                .flex_shrink_0()
                .whitespace_nowrap()
                .child(SharedString::from(value[..selection_start].to_string())),
        );
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
                input = input.child(
                    div()
                        .flex_shrink_0()
                        .whitespace_nowrap()
                        .underline()
                        .text_decoration_color(theme::accent())
                        .child(SharedString::from(ime_marked_text.clone())),
                );
            }
        }
        input = input.child(
            div()
                .flex_shrink_0()
                .whitespace_nowrap()
                .child(SharedString::from(value[selection_end..].to_string())),
        );
    }
    input = input.child(ime_input_canvas(focus, cx.entity()));

    let buttons = div()
        .flex()
        .items_center()
        .gap_2()
        .mt_4()
        .child(
            div()
                .id("quick-command-editor-save")
                .h(px(30.))
                .px_3()
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(theme::RADIUS_SM))
                .cursor_pointer()
                .bg(theme::accent())
                .text_xs()
                .text_color(theme::canvas())
                .child(icons::icon(icons::IconName::Check, 13.).text_color(theme::canvas()))
                .child(SharedString::from(i18n::text("quick_commands.save")))
                .on_click(cx.listener(|this, _ev, _window, cx| {
                    this.submit_quick_command_editor(cx);
                })),
        )
        .child(
            div()
                .id("quick-command-editor-cancel")
                .h(px(30.))
                .px_3()
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(theme::RADIUS_SM))
                .cursor_pointer()
                .bg(theme::raised())
                .text_xs()
                .text_color(theme::text())
                .child(icons::icon(icons::IconName::X, 13.).text_color(theme::muted_text()))
                .child(SharedString::from(i18n::text("prompt.cancel")))
                .on_click(cx.listener(|this, _ev, _window, cx| {
                    this.cancel_quick_command_editor(cx);
                })),
        );

    let card = div()
        .id("quick-command-editor-card")
        .w(px(500.))
        .p_5()
        .bg(theme::surface())
        .border_1()
        .border_color(theme::border_strong())
        .rounded(px(theme::RADIUS_MD))
        .shadow_md()
        .flex()
        .flex_col()
        .on_click(cx.listener(|_this, _ev, _window, cx| {
            cx.stop_propagation();
        }))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::text())
                .child(icons::icon(icons::IconName::Pencil, 16.).text_color(theme::info()))
                .child(SharedString::from(i18n::text("quick_commands.edit_title"))),
        )
        .child(input)
        .child(buttons);
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme::scrim())
        .id("quick-command-editor-scrim")
        .on_click(cx.listener(|this, _ev, _window, cx| {
            this.cancel_quick_command_editor(cx);
        }))
        .child(card)
        .into_any_element()
}

fn status_badge(text: String, color: impl Into<gpui::Hsla>) -> impl IntoElement {
    div()
        .text_color(color.into())
        .child(SharedString::from(text))
}

fn render_empty_state(cx: &mut Context<AppShell>) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(340.))
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .w(px(56.))
                        .h(px(56.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(theme::RADIUS_MD))
                        .bg(theme::accent_soft())
                        .border_1()
                        .border_color(theme::border_strong())
                        .child(
                            icons::icon(icons::IconName::Terminal, 28.).text_color(theme::accent()),
                        ),
                )
                .child(
                    div()
                        .mt_2()
                        .text_lg()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::text())
                        .child(SharedString::from(i18n::text("empty_state.title"))),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme::muted_text())
                        .child(SharedString::from(i18n::text("empty_state.description"))),
                )
                .child(
                    div()
                        .id("empty-new-project")
                        .mt_3()
                        .px_3()
                        .h(px(32.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(theme::RADIUS_SM))
                        .cursor_pointer()
                        .bg(theme::accent())
                        .text_color(theme::canvas())
                        .font_weight(FontWeight::MEDIUM)
                        .hover(|style| style.bg(theme::accent_hover()))
                        .child(
                            icons::icon(icons::IconName::FolderOpen, 14.)
                                .text_color(theme::canvas()),
                        )
                        .child(SharedString::from(i18n::text("project.open")))
                        .on_click(cx.listener(|this, _ev, _window, cx| {
                            this.choose_project_directory(cx);
                        })),
                ),
        )
        .into_any_element()
}

fn render_tab_strip(shell: &AppShell, cx: &mut Context<AppShell>) -> impl IntoElement {
    let mut strip = div()
        .id("tab-strip")
        .flex()
        .flex_row()
        .h(px(theme::TAB_HEIGHT))
        .px_2()
        .gap_1()
        .items_center();
    // This GPUI revision exposes horizontal scrolling through the style state;
    // keeping the strip as a flex container lets its fixed-width children overflow.
    strip.style().overflow.x = Some(gpui::Overflow::Scroll);
    strip = strip
        .track_scroll(&shell.tab_scroll)
        .restrict_scroll_to_axis()
        .bg(theme::surface())
        .border_b_1()
        .border_color(theme::border());

    match shell.workspace.active_view {
        Some(ActiveView::RemoteTab(active_idx)) => {
            for idx in 0..shell.workspace.sessions.remote_tabs.len() {
                let tab = &shell.workspace.sessions.remote_tabs[idx];
                let is_active = active_idx == idx;
                let state = shell.connections.state_for_key(&tab.host_key, cx);
                let alias = tab_label(tab, cx);
                // 容器不绑定 click；标签名与关闭按钮分别绑定，避免事件叠加。
                let mut container = div()
                    .flex()
                    .flex_none()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h(px(28.))
                    .px_1()
                    .rounded(px(theme::RADIUS_SM));
                if is_active {
                    container = container
                        .bg(theme::accent_soft())
                        .border_b_2()
                        .border_color(theme::accent());
                } else {
                    container = container.hover(|style| style.bg(theme::raised()));
                }
                let (has_terminal, low_latency_enabled, low_latency_available) = tab
                    .pane
                    .terminal_info(cx)
                    .map(|info| (true, info.low_latency_enabled, info.low_latency_available))
                    .unwrap_or((false, false, false));
                let container = container
                    .id(("remote-tab-container", idx))
                    .on_mouse_down(MouseButton::Right, {
                        let target = tab.target.clone();
                        let single = shell.workspace.sessions.remote_tabs.len() == 1;
                        cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                            let mut entries = vec![MenuEntry::Item(MenuItem {
                                id: "switch".into(),
                                label: i18n::text("context_menu.switch"),
                                shortcut_hint: None,
                                disabled: is_active,
                                danger: false,
                                action: ShellMenuAction::SelectRemoteTab(idx),
                            })];
                            if has_terminal {
                                entries.push(MenuEntry::CheckedItem {
                                    item: MenuItem {
                                        id: "low-latency-shell-input".into(),
                                        label: i18n::text("context_menu.low_latency_shell_input"),
                                        shortcut_hint: None,
                                        disabled: !low_latency_available,
                                        danger: false,
                                        action: ShellMenuAction::ToggleLowLatencyShellInput(idx),
                                    },
                                    checked: low_latency_enabled,
                                });
                            }
                            entries.extend([
                                MenuEntry::Item(MenuItem {
                                    id: "close".into(),
                                    label: i18n::text("context_menu.close"),
                                    shortcut_hint: None,
                                    disabled: false,
                                    danger: false,
                                    action: ShellMenuAction::CloseRemoteTab(idx),
                                }),
                                MenuEntry::Item(MenuItem {
                                    id: "close-others".into(),
                                    label: i18n::text("context_menu.close_others"),
                                    shortcut_hint: None,
                                    disabled: single,
                                    danger: false,
                                    action: ShellMenuAction::CloseOtherRemoteTabs(idx),
                                }),
                                MenuEntry::Item(MenuItem {
                                    id: "close-all".into(),
                                    label: i18n::text("context_menu.close_all"),
                                    shortcut_hint: None,
                                    disabled: false,
                                    danger: false,
                                    action: ShellMenuAction::CloseAllRemoteTabs,
                                }),
                                MenuEntry::Separator,
                                MenuEntry::Item(MenuItem {
                                    id: "copy-target".into(),
                                    label: i18n::text("context_menu.copy_target"),
                                    shortcut_hint: None,
                                    disabled: false,
                                    danger: false,
                                    action: ShellMenuAction::CopyText(target.clone()),
                                }),
                            ]);
                            this.open_context_menu(ev.position, entries, cx);
                        })
                    })
                    .child(
                        div()
                            .id(("remote-tab", idx))
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .py_1()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(if is_active {
                                theme::text()
                            } else {
                                theme::muted_text()
                            })
                            .hover(|s| s.text_color(theme::accent()))
                            .child(
                                div()
                                    .w(px(6.))
                                    .h(px(6.))
                                    .rounded_full()
                                    .bg(tab_badge_color(&state)),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .max_w(px(220.))
                                    .truncate()
                                    .child(SharedString::from(alias)),
                            )
                            .on_click(cx.listener(move |this, _ev, _w, cx| {
                                this.switch_remote_tab(idx, cx);
                            })),
                    )
                    .child(
                        div()
                            .id(("remote-tab-close", idx))
                            .w(px(24.))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(theme::RADIUS_SM))
                            .cursor_pointer()
                            .text_color(theme::muted_text())
                            .hover(|s| s.bg(theme::raised()).text_color(theme::danger()))
                            .tooltip(|_window, cx| {
                                cx.new(|_| crossh_ui::widgets::LocalPathTooltip {
                                    path: SharedString::from(i18n::text("tooltip.close_tab")),
                                })
                                .into()
                            })
                            .child(
                                icons::icon(icons::IconName::X, 13.)
                                    .text_color(theme::muted_text())
                                    .hover(|s| s.text_color(theme::danger())),
                            )
                            .on_click(cx.listener(move |this, _ev, _w, cx| {
                                this.close_remote_tab(idx, cx);
                            })),
                    );
                strip = strip.child(container);
            }
        }
        Some(ActiveView::LocalSession(active_session_id)) => {
            let session_ids = shell
                .local_dir_for_session(active_session_id)
                .map(|dir| dir.sessions.clone())
                .unwrap_or_default();
            for (idx, session_id) in session_ids.iter().copied().enumerate() {
                let is_active = active_session_id == session_id;
                let state = shell
                    .workspace
                    .sessions
                    .local_sessions
                    .get(&session_id)
                    .map(|session| session.terminal.read(cx).state.clone());
                let fallback = format!("ses{}", idx + 1);
                let label = match shell.workspace.sessions.local_sessions.get(&session_id) {
                    Some(session) => session.terminal.read(cx).tab_title(&fallback),
                    None => fallback,
                };
                let mut container = div()
                    .flex()
                    .flex_none()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h(px(28.))
                    .px_1()
                    .rounded(px(theme::RADIUS_SM));
                if is_active {
                    container = container
                        .bg(theme::accent_soft())
                        .border_b_2()
                        .border_color(theme::accent());
                } else {
                    container = container.hover(|style| style.bg(theme::raised()));
                }
                let container = container
                    .id(("local-tab-container", session_id))
                    .on_mouse_down(MouseButton::Right, {
                        let cwd = shell
                            .workspace
                            .sessions
                            .local_sessions
                            .get(&session_id)
                            .map(|session| session.cwd.clone())
                            .unwrap_or_default();
                        let single = session_ids.len() == 1;
                        cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                            let entries = vec![
                                MenuEntry::Item(MenuItem {
                                    id: "switch".into(),
                                    label: i18n::text("context_menu.switch"),
                                    shortcut_hint: None,
                                    disabled: is_active,
                                    danger: false,
                                    action: ShellMenuAction::SelectLocalSession(session_id),
                                }),
                                MenuEntry::Item(MenuItem {
                                    id: "close".into(),
                                    label: i18n::text("context_menu.close"),
                                    shortcut_hint: None,
                                    disabled: false,
                                    danger: false,
                                    action: ShellMenuAction::CloseLocalSession(session_id),
                                }),
                                MenuEntry::Item(MenuItem {
                                    id: "close-others".into(),
                                    label: i18n::text("context_menu.close_others"),
                                    shortcut_hint: None,
                                    disabled: single,
                                    danger: false,
                                    action: ShellMenuAction::CloseOtherLocalSessions(session_id),
                                }),
                                MenuEntry::Separator,
                                MenuEntry::Item(MenuItem {
                                    id: "copy-path".into(),
                                    label: i18n::text("context_menu.copy_path"),
                                    shortcut_hint: None,
                                    disabled: false,
                                    danger: false,
                                    action: ShellMenuAction::CopyText(
                                        cwd.to_string_lossy().to_string(),
                                    ),
                                }),
                                MenuEntry::Item(MenuItem {
                                    id: "reveal-finder".into(),
                                    label: i18n::text("context_menu.reveal_in_finder"),
                                    shortcut_hint: None,
                                    disabled: false,
                                    danger: false,
                                    action: ShellMenuAction::RevealInFinder(cwd.clone()),
                                }),
                            ];
                            this.open_context_menu(ev.position, entries, cx);
                        })
                    })
                    .child(
                        div()
                            .id(("local-tab", session_id))
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .py_1()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(if is_active {
                                theme::text()
                            } else {
                                theme::muted_text()
                            })
                            .hover(|s| s.text_color(theme::accent()))
                            .child(
                                div()
                                    .w(px(6.))
                                    .h(px(6.))
                                    .rounded_full()
                                    .bg(tab_badge_color(&state)),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .max_w(px(220.))
                                    .truncate()
                                    .child(SharedString::from(label)),
                            )
                            .on_click(cx.listener(move |this, _ev, _w, cx| {
                                this.select_local_session(session_id, cx);
                            })),
                    )
                    .child(
                        div()
                            .id(("local-tab-close", session_id))
                            .w(px(24.))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(theme::RADIUS_SM))
                            .cursor_pointer()
                            .text_color(theme::muted_text())
                            .hover(|s| s.bg(theme::raised()).text_color(theme::danger()))
                            .tooltip(|_window, cx| {
                                cx.new(|_| crossh_ui::widgets::LocalPathTooltip {
                                    path: SharedString::from(i18n::text("tooltip.close_tab")),
                                })
                                .into()
                            })
                            .child(
                                icons::icon(icons::IconName::X, 13.)
                                    .text_color(theme::muted_text())
                                    .hover(|s| s.text_color(theme::danger())),
                            )
                            .on_click(cx.listener(move |this, _ev, _w, cx| {
                                this.close_local_session(session_id, cx);
                            })),
                    );
                strip = strip.child(container);
            }
        }
        None => {}
    }

    strip.child(
        div()
            .id("new-tab")
            .flex_shrink_0()
            .w(px(28.))
            .h(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(theme::RADIUS_SM))
            .cursor_pointer()
            .bg(theme::raised())
            .border_1()
            .border_color(theme::border())
            .text_color(theme::accent())
            .hover(|s| {
                s.bg(theme::accent())
                    .border_color(theme::accent())
                    .text_color(theme::canvas())
            })
            .tooltip(|_window, cx| {
                cx.new(|_| crossh_ui::widgets::LocalPathTooltip {
                    path: SharedString::from(i18n::text("tooltip.new_terminal")),
                })
                .into()
            })
            .child(
                icons::icon(icons::IconName::Plus, 15.)
                    .text_color(theme::accent())
                    .hover(|s| s.text_color(theme::canvas())),
            )
            .on_click(cx.listener(|this, _ev, window, cx| {
                this.new_tab(window, cx);
            })),
    )
}

fn tab_badge_color(state: &Option<ConnState>) -> gpui::Rgba {
    match state {
        Some(ConnState::Connecting) => theme::warning(),
        Some(ConnState::Connected) => theme::accent(),
        Some(ConnState::Error(_)) => theme::danger(),
        Some(ConnState::Closed) | None => theme::faint_text(),
    }
}

fn tab_label(tab: &Tab, cx: &mut Context<AppShell>) -> String {
    tab.pane.title(cx)
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
        let remembered = vec![
            PathBuf::from("/Users/me/one"),
            PathBuf::from("/Users/me/two"),
        ];
        let current = vec![(1, PathBuf::from("/Users/me/one"))];

        let dirs = rebuild_local_dirs(&previous, current, remembered, Some(1));
        assert_eq!(dirs[&PathBuf::from("/Users/me/one")].sessions, vec![1]);
        assert_eq!(
            dirs[&PathBuf::from("/Users/me/two")].sessions,
            Vec::<LocalSessionId>::new()
        );
        assert_eq!(dirs[&PathBuf::from("/Users/me/two")].active_session, None);
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

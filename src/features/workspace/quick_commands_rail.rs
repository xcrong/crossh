//! 折叠快捷命令栏及其后台任务状态。

use gpui::{
    AnyElement, AppContext, Context, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, div, px,
};

use crossh_core::commands::{
    BackgroundTask, BackgroundTaskManager, BackgroundTaskStatus, CommandRecord,
};
use crossh_ui::context_menu::{MenuEntry, MenuItem, ShellMenuAction};
use crossh_ui::theme;
use crossh_ui::widgets::{CommandTooltip, LocalPathTooltip};
use crossh_ui_component::{Avatar, AvatarKind};

use crate::features::workspace::shell::AppShell;
use crate::shared::i18n;

const QUICK_COMMANDS_RAIL_ITEM_SIZE: f32 = 30.0;
const QUICK_COMMANDS_RAIL_ITEM_GAP: f32 = 4.0;

fn rail_background_tasks(
    background_tasks: &BackgroundTaskManager,
    scope: &str,
) -> Vec<BackgroundTask> {
    background_tasks.tasks_for_scope(scope)
}

pub(crate) fn render_quick_commands_rail(
    shell: &AppShell,
    scope: &str,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let tasks = rail_background_tasks(&shell.background_tasks, scope);
    let pinned = shell.command_history.pinned(scope);
    let unpinned_tasks = unpinned_background_tasks(&tasks, &pinned);
    let mut contents = div()
        .w_full()
        .h_full()
        .min_h_0()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(QUICK_COMMANDS_RAIL_ITEM_GAP))
        .pt_2();
    contents.style().overflow.y = Some(gpui::Overflow::Scroll);

    for (index, record) in pinned.iter().enumerate() {
        contents = contents.child(render_pinned_command(
            scope,
            record.command.clone(),
            tasks.iter().find(|task| task.command == record.command),
            index,
            cx,
        ));
    }

    if !pinned.is_empty() && !unpinned_tasks.is_empty() {
        contents = contents.child(
            div()
                .w(px(20.))
                .h(px(1.))
                .my_1()
                .flex_shrink_0()
                .bg(theme::border()),
        );
    }
    for task in unpinned_tasks {
        contents = contents.child(render_background_task(task, cx));
    }

    div()
        .id("quick-commands-rail")
        .w(px(theme::QUICK_COMMANDS_RAIL_WIDTH))
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .items_center()
        .bg(theme::surface())
        .border_l_1()
        .border_color(theme::border())
        .child(contents)
        .into_any_element()
}

fn unpinned_background_tasks(
    tasks: &[BackgroundTask],
    pinned: &[CommandRecord],
) -> Vec<BackgroundTask> {
    tasks
        .iter()
        .filter(|task| !pinned.iter().any(|record| record.command == task.command))
        .cloned()
        .collect()
}

fn render_pinned_command(
    scope: &str,
    command: String,
    active_task: Option<&BackgroundTask>,
    index: usize,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let run_scope = scope.to_string();
    let menu_scope = scope.to_string();
    let menu_command = command.clone();
    let tooltip_command = command.clone();
    let active_task_id = active_task.map(|task| task.id);
    let mut item = div()
        .id(SharedString::from(format!("quick-command-rail-{index}")))
        .relative()
        .w(px(QUICK_COMMANDS_RAIL_ITEM_SIZE))
        .h(px(QUICK_COMMANDS_RAIL_ITEM_SIZE))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .hover(|style| style.bg(theme::raised()))
        .tooltip(move |_window, cx| {
            cx.new(|_| CommandTooltip {
                command: SharedString::from(tooltip_command.clone()),
            })
            .into()
        })
        .child(Avatar::new(&command).kind(AvatarKind::Command))
        .on_click(
            cx.listener(move |this, ev: &gpui::ClickEvent, _window, cx| {
                if ev.click_count() == 2 {
                    this.run_quick_command(run_scope.clone(), command.clone(), false, cx);
                }
            }),
        )
        .on_mouse_down(MouseButton::Right, {
            cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                let mut entries = vec![
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
                ];
                if let Some(id) = active_task_id {
                    entries.push(MenuEntry::Separator);
                    entries.push(MenuEntry::Item(MenuItem {
                        id: format!("quick-stop-background-{id}"),
                        label: i18n::text("quick_commands.stop"),
                        shortcut_hint: None,
                        disabled: false,
                        danger: true,
                        action: ShellMenuAction::StopBackgroundTask(id),
                    }));
                }
                this.open_context_menu(ev.position, entries, cx);
            })
        });
    if let Some(task) = active_task {
        item = item.child(background_task_badge(task.status));
    }
    item.into_any_element()
}

fn render_background_task(task: BackgroundTask, cx: &mut Context<AppShell>) -> AnyElement {
    let id = task.id;
    let status = background_task_label(task.status);
    let tooltip = format!("{status}\n{}\n{}", task.command, task.cwd.to_string_lossy());
    div()
        .id(SharedString::from(format!("quick-command-rail-task-{id}")))
        .relative()
        .w(px(QUICK_COMMANDS_RAIL_ITEM_SIZE))
        .h(px(QUICK_COMMANDS_RAIL_ITEM_SIZE))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .bg(theme::raised())
        .hover(|style| style.bg(theme::accent_soft()))
        .tooltip(move |_window, cx| {
            cx.new(|_| LocalPathTooltip {
                path: SharedString::from(tooltip.clone()),
            })
            .into()
        })
        .child(Avatar::new(&task.command).kind(AvatarKind::Command))
        .child(background_task_badge(task.status))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                this.open_context_menu(
                    ev.position,
                    vec![MenuEntry::Item(MenuItem {
                        id: format!("quick-stop-background-{id}"),
                        label: i18n::text("quick_commands.stop"),
                        shortcut_hint: None,
                        disabled: false,
                        danger: true,
                        action: ShellMenuAction::StopBackgroundTask(id),
                    })],
                    cx,
                );
            }),
        )
        .into_any_element()
}

fn background_task_badge(status: BackgroundTaskStatus) -> impl IntoElement {
    div()
        .absolute()
        .top(px(1.))
        .right(px(1.))
        .w(px(7.))
        .h(px(7.))
        .rounded_full()
        .border_1()
        .border_color(theme::surface())
        .bg(background_task_color(status))
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

#[cfg(test)]
const fn quick_commands_rail_item_pitch() -> f32 {
    QUICK_COMMANDS_RAIL_ITEM_SIZE + QUICK_COMMANDS_RAIL_ITEM_GAP
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crossh_core::commands::{
        BackgroundTaskEvent, BackgroundTaskManager, BackgroundTaskStatus, CommandRecord,
    };

    use super::{rail_background_tasks, unpinned_background_tasks};

    #[test]
    fn rail_shows_active_background_tasks_for_the_current_scope() {
        let mut tasks = BackgroundTaskManager::default();
        let first = tasks.start_remote(
            "local:/work".into(),
            PathBuf::from("/work"),
            "cargo test".into(),
            "local:1".into(),
        );
        let second = tasks.start_remote(
            "local:/work".into(),
            PathBuf::from("/work"),
            "cargo test".into(),
            "local:1".into(),
        );
        tasks.mark_stopping(first);
        tasks.start_remote(
            "local:/other".into(),
            PathBuf::from("/other"),
            "cargo check".into(),
            "local:2".into(),
        );

        let visible = rail_background_tasks(&tasks, "local:/work");

        assert_eq!(
            visible.iter().map(|task| task.id).collect::<Vec<_>>(),
            vec![second, first]
        );
        assert_eq!(visible[0].status, BackgroundTaskStatus::Running);
        assert_eq!(visible[1].status, BackgroundTaskStatus::Stopping);
    }

    #[test]
    fn rail_removes_a_background_task_after_completion() {
        let mut tasks = BackgroundTaskManager::default();
        let id = tasks.start_remote(
            "local:/work".into(),
            PathBuf::from("/work"),
            "cargo test".into(),
            "local:1".into(),
        );

        tasks.apply_event(BackgroundTaskEvent {
            id,
            status: BackgroundTaskStatus::Succeeded,
            output: String::new(),
            exit_code: Some(0),
        });

        assert!(rail_background_tasks(&tasks, "local:/work").is_empty());
    }

    #[test]
    fn rail_reuses_pinned_command_avatar_for_its_background_task() {
        let mut tasks = BackgroundTaskManager::default();
        let pinned_task = tasks.start_remote(
            "local:/work".into(),
            PathBuf::from("/work"),
            "cargo test".into(),
            "local:1".into(),
        );
        let unpinned_task = tasks.start_remote(
            "local:/work".into(),
            PathBuf::from("/work"),
            "cargo check".into(),
            "local:1".into(),
        );
        let pinned = vec![CommandRecord {
            command: "cargo test".into(),
            pinned: true,
            count: 1,
            last_used: 1,
        }];

        let visible =
            unpinned_background_tasks(&rail_background_tasks(&tasks, "local:/work"), &pinned);

        assert_eq!(
            visible.iter().map(|task| task.id).collect::<Vec<_>>(),
            vec![unpinned_task]
        );
        assert_ne!(pinned_task, unpinned_task);
    }

    #[test]
    fn collapsed_quick_commands_leave_space_between_avatars() {
        assert_eq!(super::quick_commands_rail_item_pitch(), 34.0);
    }
}

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
use crossh_ui_component::{Avatar, AvatarKind, StatusDot, Tooltip};

use crate::features::workspace::shell::AppShell;
use crate::features::workspace::status::{background_task_color, background_task_label};
use crate::shared::i18n;

const QUICK_COMMANDS_RAIL_ITEM_SIZE: f32 = 30.0;
const QUICK_COMMANDS_RAIL_ITEM_GAP: f32 = 4.0;

fn rail_background_tasks(
    background_tasks: &BackgroundTaskManager,
    scope: &str,
) -> Vec<BackgroundTask> {
    background_tasks.tasks_for_scope(scope)
}

/// 渲染层过滤：命令被固定在 rail 上时，其进行中的后台任务由该命令的头像承载
/// （头像上叠加状态徽标，右键菜单提供重启/停止），因此不为它单独再渲染一个
/// 后台任务图标，避免 rail 上出现两个同命令的图标。
/// 未被固定的命令任务仍以独立图标展示，保证每个任务都可单独查看和操作。
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

pub(crate) fn render_quick_commands_rail(
    shell: &AppShell,
    scope: &str,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let tasks = rail_background_tasks(&shell.background_tasks, scope);
    let pinned = shell.command_history.pinned(scope);
    // 被固定的命令在 rail 上复用其头像，不再重复渲染后台任务图标。
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
            shell,
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

fn render_pinned_command(
    shell: &AppShell,
    scope: &str,
    command: String,
    active_task: Option<&BackgroundTask>,
    index: usize,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let run_scope = scope.to_string();
    let menu_scope = scope.to_string();
    let menu_command = command.clone();
    let running_id = shell
        .background_tasks
        .running_for_command(scope, &command)
        .first()
        .copied();
    // 头像承载的是该命令首个进行中实例：非 Running 状态（如 Stopping）仍可终止。
    let active_task_id = active_task.map(|task| task.id);
    let tooltip_command = command.clone();
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
            cx.new(|_| Tooltip::new(tooltip_command.clone()).wide())
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
                let mut entries = Vec::new();
                if let Some(id) = running_id {
                    entries.push(MenuEntry::Item(MenuItem {
                        id: "quick-restart-background".into(),
                        label: i18n::text("quick_commands.restart"),
                        shortcut_hint: None,
                        disabled: false,
                        danger: false,
                        action: ShellMenuAction::RestartBackgroundTask(id),
                    }));
                } else if let Some(id) = active_task_id {
                    entries.push(MenuEntry::Item(MenuItem {
                        id: "quick-stop-background".into(),
                        label: i18n::text("quick_commands.stop"),
                        shortcut_hint: None,
                        disabled: false,
                        danger: true,
                        action: ShellMenuAction::StopBackgroundTask(id),
                    }));
                } else {
                    entries.push(MenuEntry::Item(MenuItem {
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
                ]);
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
    let is_running = task.status == BackgroundTaskStatus::Running;
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
        .tooltip(move |_window, cx| cx.new(|_| Tooltip::new(tooltip.clone())).into())
        .child(Avatar::new(&task.command).kind(AvatarKind::Command))
        .child(background_task_badge(task.status))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                let mut entries = Vec::new();
                if is_running {
                    entries.push(MenuEntry::Item(MenuItem {
                        id: format!("quick-restart-background-{id}"),
                        label: i18n::text("quick_commands.restart"),
                        shortcut_hint: None,
                        disabled: false,
                        danger: false,
                        action: ShellMenuAction::RestartBackgroundTask(id),
                    }));
                }
                entries.push(MenuEntry::Item(MenuItem {
                    id: format!("quick-stop-background-{id}"),
                    label: i18n::text("quick_commands.stop"),
                    shortcut_hint: None,
                    disabled: false,
                    danger: true,
                    action: ShellMenuAction::StopBackgroundTask(id),
                }));
                this.open_context_menu(ev.position, entries, cx);
            }),
        )
        .into_any_element()
}

fn background_task_badge(status: BackgroundTaskStatus) -> impl IntoElement {
    div().absolute().top(px(1.)).right(px(1.)).child(
        StatusDot::new(background_task_color(status))
            .size(px(7.))
            .border(theme::surface()),
    )
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
    fn rail_keeps_each_concurrent_task_visible_in_the_task_list() {
        // 列表层保证并发实例齐全；渲染层是否复用固定头像由
        // unpinned_background_tasks 决定（见下方两个测试）。
        let mut tasks = BackgroundTaskManager::default();
        let pinned_task = tasks.start_remote(
            "local:/work".into(),
            PathBuf::from("/work"),
            "cargo test".into(),
            "local:1".into(),
        );
        let second_pinned_task = tasks.start_remote(
            "local:/work".into(),
            PathBuf::from("/work"),
            "cargo test".into(),
            "local:1".into(),
        );
        assert_eq!(
            rail_background_tasks(&tasks, "local:/work")
                .iter()
                .map(|task| task.id)
                .collect::<Vec<_>>(),
            vec![second_pinned_task, pinned_task]
        );
    }

    #[test]
    fn rail_reuses_pinned_command_avatar_for_its_background_task() {
        // 回归防护：被固定命令进行中的任务由固定头像承载，不得再渲染独立图标。
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
    fn rail_filters_out_all_instances_of_a_pinned_command() {
        // 即使同一固定命令存在多个并发实例，也全部由该命令的头像承载，
        // 防止 rail 上出现重复图标。
        let mut tasks = BackgroundTaskManager::default();
        tasks.start_remote(
            "local:/work".into(),
            PathBuf::from("/work"),
            "cargo test".into(),
            "local:1".into(),
        );
        tasks.start_remote(
            "local:/work".into(),
            PathBuf::from("/work"),
            "cargo test".into(),
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

        assert!(visible.is_empty());
    }

    #[test]
    fn collapsed_quick_commands_leave_space_between_avatars() {
        assert_eq!(super::quick_commands_rail_item_pitch(), 34.0);
    }
}

//! AppShell 标签导航与标签级编辑：新建、切换、关闭、重命名与默认命令。
//!
//! 布局归 daemon：所有操作翻译成 `WorkspaceOp`，本模块只保存输入弹窗草稿。

use gpui::{PromptButton, PromptLevel, Window};

use super::*;
use crate::features::workspace::modal_editor::{DefaultCommandEditor, RenameEditor};
use crate::features::workspace::projection;

impl AppShell {
    pub(crate) fn handle_new_terminal(
        &mut self,
        _: &crate::NewTerminal,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_tab(cx);
    }

    pub(crate) fn handle_cycle_next_tab(
        &mut self,
        _: &crate::features::workspace::shell::CycleNextTab,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_guarded(1, cx);
    }

    pub(crate) fn handle_cycle_prev_tab(
        &mut self,
        _: &crate::features::workspace::shell::CyclePrevTab,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_guarded(-1, cx);
    }

    pub(crate) fn handle_switch_to_tab1(
        &mut self,
        _: &crate::features::workspace::shell::SwitchToTab1,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.switch_to_index(0, cx);
    }

    pub(crate) fn handle_switch_to_tab2(
        &mut self,
        _: &crate::features::workspace::shell::SwitchToTab2,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.switch_to_index(1, cx);
    }

    pub(crate) fn handle_switch_to_tab3(
        &mut self,
        _: &crate::features::workspace::shell::SwitchToTab3,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.switch_to_index(2, cx);
    }

    pub(crate) fn handle_switch_to_tab4(
        &mut self,
        _: &crate::features::workspace::shell::SwitchToTab4,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.switch_to_index(3, cx);
    }

    pub(crate) fn handle_switch_to_tab5(
        &mut self,
        _: &crate::features::workspace::shell::SwitchToTab5,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.switch_to_index(4, cx);
    }

    pub(crate) fn handle_switch_to_tab6(
        &mut self,
        _: &crate::features::workspace::shell::SwitchToTab6,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.switch_to_index(5, cx);
    }

    pub(crate) fn handle_switch_to_tab7(
        &mut self,
        _: &crate::features::workspace::shell::SwitchToTab7,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.switch_to_index(6, cx);
    }

    pub(crate) fn handle_switch_to_tab8(
        &mut self,
        _: &crate::features::workspace::shell::SwitchToTab8,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.switch_to_index(7, cx);
    }

    pub(crate) fn handle_switch_to_tab9(
        &mut self,
        _: &crate::features::workspace::shell::SwitchToTab9,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.switch_to_index(8, cx);
    }

    pub(crate) fn handle_close_active_tab(
        &mut self,
        _: &crate::CloseActiveTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_id) = self.workspace.active_tab().map(|tab| tab.id) else {
            return;
        };
        self.request_close_tab(tab_id, window, cx);
    }

    /// 新建 tab：活跃项目内交给 daemon 选 cwd；没有项目时把当前目录开成项目。
    pub(crate) fn new_tab(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.workspace.active_project() else {
            self.open_project_root(current_local_cwd(), cx);
            return;
        };
        let project_id = project.id;
        let cwd = self
            .workspace
            .focused_pane()
            .and_then(|pane_id| self.workspace.pane(pane_id))
            .map(|pane| pane.cwd.clone());
        self.send_workspace_op(
            WorkspaceOp::CreateTab {
                project_id: Some(project_id),
                title: None,
                cwd,
                default_command: None,
            },
            cx,
        );
    }

    fn should_block_tab_shortcuts(&self) -> bool {
        self.context_menu.is_some()
            || self.command_palette.is_some()
            || self.rename_editor.is_some()
            || self.default_command_editor.is_some()
    }

    fn switch_to_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.should_block_tab_shortcuts() {
            return;
        }
        let tab_id = self
            .workspace
            .active_project()
            .and_then(|project| project.tabs.get(index))
            .map(|tab| tab.id);
        if let Some(tab_id) = tab_id {
            self.activate_tab(tab_id, cx);
        }
    }

    fn cycle_guarded(&mut self, direction: isize, cx: &mut Context<Self>) {
        if self.should_block_tab_shortcuts() {
            return;
        }
        let next = self.workspace.active_project().and_then(|project| {
            let active = project.active_tab?;
            let index = projection::tab_index(project, active)?;
            let len = project.tabs.len();
            if len == 0 {
                return None;
            }
            let next = (index as isize + direction).rem_euclid(len as isize) as usize;
            project.tabs.get(next).map(|tab| tab.id)
        });
        if let Some(tab_id) = next {
            self.activate_tab(tab_id, cx);
        }
    }

    /// 关闭 tab（连同其下全部 pane）；有命令运行时先确认。
    pub(crate) fn request_close_tab(
        &mut self,
        tab_id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, tab)) = self.workspace.tab(tab_id) else {
            return;
        };
        let running = tab.layout.leaves().any(|pane| {
            self.workspace
                .pane_view(pane.id)
                .is_some_and(|view| view.is_command_running(cx))
        });
        if !running {
            self.send_workspace_op(WorkspaceOp::CloseTab { tab_id }, cx);
            return;
        }
        self.prompt_close_tab(window, cx, move |this, cx| {
            this.send_workspace_op(WorkspaceOp::CloseTab { tab_id }, cx);
        });
    }

    /// 关闭单个 pane；有命令运行时先确认。
    pub(crate) fn request_close_pane(
        &mut self,
        pane_id: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.pane(pane_id).is_none() {
            return;
        }
        let running = self
            .workspace
            .pane_view(pane_id)
            .is_some_and(|view| view.is_command_running(cx));
        if !running {
            self.send_workspace_op(WorkspaceOp::ClosePane { pane_id }, cx);
            return;
        }
        self.prompt_close_tab(window, cx, move |this, cx| {
            this.send_workspace_op(WorkspaceOp::ClosePane { pane_id }, cx);
        });
    }

    pub(crate) fn close_other_tabs(
        &mut self,
        project_id: ProjectId,
        keep: TabId,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.workspace.project(project_id) else {
            return;
        };
        let others = project
            .tabs
            .iter()
            .filter(|tab| tab.id != keep)
            .map(|tab| tab.id)
            .collect::<Vec<_>>();
        self.activate_tab(keep, cx);
        for tab_id in others {
            self.send_workspace_op(WorkspaceOp::CloseTab { tab_id }, cx);
        }
    }

    fn prompt_close_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        on_confirm: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) {
        if self.tab_close_confirmation_open || self.shutdown_in_progress {
            return;
        }
        self.tab_close_confirmation_open = true;
        let answers = [
            PromptButton::ok(i18n::text("tab_close.confirm")),
            PromptButton::cancel(i18n::text("tab_close.cancel")),
        ];
        let detail = format!(
            "{}\n\n{}",
            i18n::text("tab_close.running"),
            i18n::text("tab_close.consequence")
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            &i18n::text("tab_close.title"),
            Some(&detail),
            &answers,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let confirmed = answer.await == Ok(0);
            let _ = this.update(cx, |this, cx| {
                this.tab_close_confirmation_open = false;
                if confirmed {
                    on_confirm(this, cx);
                }
            });
        })
        .detach();
    }

    pub(crate) fn open_rename_tab(
        &mut self,
        tab_id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, tab)) = self.workspace.tab(tab_id) else {
            return;
        };
        let index = self
            .workspace
            .snapshot
            .projects
            .iter()
            .flat_map(|project| &project.tabs)
            .position(|candidate| candidate.id == tab_id)
            .unwrap_or(0);
        let focus = cx.focus_handle();
        self.rename_editor = Some(RenameEditor::new(
            tab_id,
            projection::tab_title(tab, index),
            focus.clone(),
        ));
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(crate) fn submit_rename_tab(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.rename_editor.take() else {
            return;
        };
        let title = editor.state.value.trim();
        self.send_workspace_op(
            WorkspaceOp::RenameTab {
                tab_id: editor.tab_id,
                title: (!title.is_empty()).then(|| title.to_string()),
            },
            cx,
        );
        self.refocus_active_terminal(cx);
        cx.notify();
    }

    pub(crate) fn cancel_rename_tab(&mut self, cx: &mut Context<Self>) {
        if self.rename_editor.take().is_some() {
            self.refocus_active_terminal(cx);
            cx.notify();
        }
    }

    pub(crate) fn open_default_command_editor(
        &mut self,
        tab_id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, tab)) = self.workspace.tab(tab_id) else {
            return;
        };
        let focus = cx.focus_handle();
        self.default_command_editor = Some(DefaultCommandEditor::new(
            tab_id,
            tab.default_command.clone().unwrap_or_default(),
            focus.clone(),
        ));
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(crate) fn submit_default_command(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.default_command_editor.take() else {
            return;
        };
        let command = editor.state.value.trim();
        self.set_tab_command(
            editor.tab_id,
            (!command.is_empty()).then(|| command.to_string()),
            cx,
        );
        self.refocus_active_terminal(cx);
        cx.notify();
    }

    pub(crate) fn cancel_default_command(&mut self, cx: &mut Context<Self>) {
        if self.default_command_editor.take().is_some() {
            self.refocus_active_terminal(cx);
            cx.notify();
        }
    }

    pub(crate) fn set_tab_command(
        &mut self,
        tab_id: TabId,
        command: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.send_workspace_op(WorkspaceOp::SetTabCommand { tab_id, command }, cx);
    }

    /// 把已保存的默认命令立即送进该 tab 的终端；只在该 pane 空闲时执行。
    pub(crate) fn reload_default_command(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        let Some((_, tab)) = self.workspace.tab(tab_id) else {
            return;
        };
        let Some(command) = tab
            .default_command
            .clone()
            .filter(|command| !command.trim().is_empty())
        else {
            return;
        };
        let pane_id = tab
            .active_pane
            .filter(|pane_id| {
                tab.layout
                    .find_pane(*pane_id)
                    .is_some_and(|pane| pane.session.is_some())
            })
            .or_else(|| {
                tab.layout
                    .leaves()
                    .find(|pane| pane.session.is_some())
                    .map(|pane| pane.id)
            });
        let Some(pane_id) = pane_id else {
            self.show_toast(
                ToastNotice::new(i18n::text("workspace.pane_not_ready"), ToastTone::Info),
                cx,
            );
            return;
        };
        let Some(view) = self.workspace.pane_view(pane_id).cloned() else {
            return;
        };
        if view.is_command_running(cx) {
            return;
        }
        self.workspace.default_command_runs.insert(tab_id);
        view.run_command(&command, cx);
    }

    /// tab 重排：落到目标 tab 之前；同项目内由 daemon 保证顺序。
    pub(crate) fn reorder_tab(&mut self, dragged: TabId, target: TabId, cx: &mut Context<Self>) {
        if dragged == target {
            return;
        }
        let Some((project, _)) = self.workspace.tab(target) else {
            return;
        };
        let project_id = project.id;
        self.send_workspace_op(
            WorkspaceOp::ReorderTabs {
                project_id,
                tab_id: dragged,
                before_tab_id: Some(target),
            },
            cx,
        );
    }
}

//! AppShell terminal tab and session navigation.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossh_terminal::ConnState;
use gpui::{PromptButton, PromptLevel, Window};

use task::Shell;

use crossh_core::terminal::remote_shell_bootstrap_command;

use super::*;
use crate::features::workspace::local_paths::{current_local_cwd, normalize_local_cwd};
use crate::features::workspace::modal_editor::{DefaultCommandEditor, RenameEditor};
use crate::features::workspace::pinned::{next_pin_id, pinned_tabs_for_project};
use crate::features::workspace::settings::PinnedLocalTab;

/// 单个标签页关闭时可能被打断的活动；任何一项存在都需要确认。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TabCloseRisk {
    command_running: bool,
    sftp_writes: usize,
    unsaved_editors: usize,
    active_forwards: usize,
}

/// 分栏右窗格会话退出分栏时的去向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SplitPaneRetirement {
    /// 无活动：直接销毁。分栏是当前 Tab 的临时工作区，退出即清理。
    Close,
    /// 有活动（命令运行中/传输中/未保存等）：保留为普通标签，不打断用户。
    KeepAsTab,
}

/// 决策右窗格会话退出分栏的去向。`None` 表示会话已不存在，统一按关闭
/// 处理（关闭操作对不存在的会话是安全的空操作）。
pub(super) fn split_pane_retirement(risk: Option<&TabCloseRisk>) -> SplitPaneRetirement {
    match risk {
        Some(risk) if risk.needs_confirmation() => SplitPaneRetirement::KeepAsTab,
        _ => SplitPaneRetirement::Close,
    }
}

impl TabCloseRisk {
    pub(crate) fn needs_confirmation(&self) -> bool {
        self.command_running
            || self.sftp_writes > 0
            || self.unsaved_editors > 0
            || self.active_forwards > 0
    }

    fn detail(&self) -> String {
        let mut lines = Vec::new();
        if self.command_running {
            lines.push(i18n::text("tab_close.running"));
        }
        if self.sftp_writes > 0 {
            lines.push(rust_i18n::t!("tab_close.transfers", count = self.sftp_writes).to_string());
        }
        if self.unsaved_editors > 0 {
            lines.push(
                rust_i18n::t!("tab_close.unsaved_editors", count = self.unsaved_editors)
                    .to_string(),
            );
        }
        if self.active_forwards > 0 {
            lines.push(
                rust_i18n::t!("tab_close.forwards", count = self.active_forwards).to_string(),
            );
        }
        lines.push(String::new());
        lines.push(i18n::text("tab_close.consequence"));
        lines.join("\n")
    }
}

impl AppShell {
    pub(super) fn handle_new_terminal(
        &mut self,
        _: &crate::NewTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_tab(window, cx);
    }

    pub(super) fn handle_close_active_tab(
        &mut self,
        _: &crate::CloseActiveTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_close_active_tab(window, cx);
    }

    /// 按别名或 `user@host[:port]` 打开一个终端标签。
    ///
    /// Zed owns the interactive SSH process and keeps authentication prompts
    /// inside the same terminal, just like its native terminal workflow.
    ///
    /// 打开新 Tab 不会取消源 Tab 的分栏：分栏跟随其属主 Tab，新 Tab 只是
    /// 暂时盖住它，切回属主 Tab 时原分栏原样恢复。
    pub(super) fn open_terminal_target(&mut self, target: String, cx: &mut Context<Self>) {
        let view = self.create_terminal_target(target, cx);
        self.workspace.active_view = Some(view);
        self.status = None;
        cx.notify();
    }

    pub(super) fn open_terminal_target_for_split(
        &mut self,
        target: String,
        cx: &mut Context<Self>,
    ) -> ActiveView {
        self.create_terminal_target(target, cx)
    }

    fn create_terminal_target(&mut self, target: String, cx: &mut Context<Self>) -> ActiveView {
        let resolved = self.connections.resolve(&target);
        let host_key = ConnectionManager::pool_key(&resolved);
        let terminal = TerminalView::from_zed_shell(
            None,
            Some("~".to_string()),
            zed_ssh_shell(&target, &resolved),
            true,
            self.terminal_settings.clone(),
            cx,
        );
        let event_host_key = host_key.clone();
        let subscription = cx.subscribe(
            &terminal,
            move |this, terminal, event: &TerminalEvent, cx| match event {
                TerminalEvent::Closed => {
                    this.close_remote_terminal(terminal.entity_id(), cx);
                }
                TerminalEvent::TitleChanged | TerminalEvent::Notification => cx.notify(),
                TerminalEvent::CommandStarted { command, cwd } => {
                    if !terminal.read(cx).is_local()
                        && let Some(cwd) = cwd.as_deref()
                    {
                        this.record_command(
                            remote_scope(&event_host_key, cwd),
                            command.clone(),
                            cx,
                        );
                    }
                }
                TerminalEvent::CommandFinished { status } => {
                    log::debug!("remote terminal command finished with status {status:?}");
                }
                TerminalEvent::CwdChanged => cx.notify(),
                TerminalEvent::PromptReached => {}
            },
        );
        let adjacent_subscription = cx.subscribe(
            &terminal,
            |this, terminal, event: &TerminalViewEvent, cx| match event {
                TerminalViewEvent::SendSelectionToAdjacent { text } => {
                    this.send_to_adjacent_terminal(terminal.entity_id(), text, cx);
                }
            },
        );
        self.workspace
            .sessions
            .terminal_subscriptions
            .push(subscription);
        self.workspace
            .sessions
            .terminal_subscriptions
            .push(adjacent_subscription);
        self.workspace.sessions.remote_tabs.push(Tab {
            target,
            host_key,
            connection: None,
            pane: crate::features::terminal::view::workspace_pane(terminal),
        });
        ActiveView::RemoteTab(self.workspace.sessions.remote_tabs.len() - 1)
    }

    pub(super) fn switch_tab(&mut self, idx: usize, cx: &mut Context<Self>) {
        match self.workspace.active_view {
            Some(ActiveView::RemoteTab(_)) => self.switch_remote_tab(idx, cx),
            Some(ActiveView::LocalSession(session_id)) => {
                let next_session = self
                    .local_dir_for_session(session_id)
                    .and_then(|dir| dir.sessions.get(idx).copied());
                if let Some(next_session) = next_session {
                    self.select_local_session(next_session, cx);
                }
            }
            None => {}
        }
    }

    pub(crate) fn switch_remote_tab(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.workspace.sessions.remote_tabs.len()
            || self.workspace.active_view == Some(ActiveView::RemoteTab(idx))
        {
            return;
        }
        self.workspace.active_view = Some(ActiveView::RemoteTab(idx));
        self.refocus_active_terminal(cx);
        cx.notify();
    }

    pub(crate) fn close_remote_tab(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.workspace.sessions.remote_tabs.len() {
            return;
        }
        let closing_view = ActiveView::RemoteTab(idx);
        let split_affected = self.prepare_terminal_split_view_close(closing_view, cx);
        if let Some(owner) = remote_tab_background_owner(&self.workspace.sessions.remote_tabs[idx])
        {
            self.stop_background_tasks_for_owner(&owner, cx);
        }
        self.workspace.sessions.remote_tabs[idx].pane.cleanup(cx);
        self.workspace.sessions.remote_tabs.remove(idx);
        // 移除 Tab → Entity<TerminalView> 释放 → input_tx 断 → relay 结束 →
        // Connection channel 计数减；归 0 则连接自行 disconnect。
        self.workspace.active_view = match self.workspace.active_view {
            Some(ActiveView::RemoteTab(a)) if a == idx => {
                if self.workspace.sessions.remote_tabs.is_empty() {
                    self.first_local_view()
                } else if a >= self.workspace.sessions.remote_tabs.len() {
                    Some(ActiveView::RemoteTab(
                        self.workspace.sessions.remote_tabs.len() - 1,
                    ))
                } else {
                    Some(ActiveView::RemoteTab(a))
                }
            }
            Some(ActiveView::RemoteTab(a)) if a > idx => Some(ActiveView::RemoteTab(a - 1)),
            other => other,
        };
        self.workspace.remove_compose_for_view(closing_view);
        self.workspace.remap_split_remote_tab_indices(idx);
        self.workspace.remap_compose_remote_tab_indices(idx);
        if split_affected {
            self.refocus_active_terminal(cx);
        }
        cx.notify();
    }

    fn close_remote_terminal(&mut self, terminal_id: EntityId, cx: &mut Context<Self>) {
        let Some(idx) = self
            .workspace
            .sessions
            .remote_tabs
            .iter()
            .position(|tab| tab.pane.terminal_entity_id() == Some(terminal_id))
        else {
            return;
        };
        self.close_remote_tab(idx, cx);
    }

    /// 关闭活动标签；有命令运行等风险时先弹确认框。
    pub(super) fn request_close_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.workspace.focused_view() {
            Some(ActiveView::RemoteTab(idx)) => self.request_close_remote_tab(idx, window, cx),
            Some(ActiveView::LocalSession(session_id)) => {
                self.request_close_local_session(session_id, window, cx)
            }
            None => {}
        }
    }

    /// 关闭单个远程标签；存在活动风险时先请求确认。
    pub(crate) fn request_close_remote_tab(
        &mut self,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(risk) = self.remote_tab_close_risk(idx, cx) else {
            return;
        };
        if !risk.needs_confirmation() {
            self.close_remote_tab(idx, cx);
            return;
        }
        self.prompt_close_tab(risk, window, cx, move |this, cx| {
            this.close_remote_tab(idx, cx);
        });
    }

    /// 关闭单个本地会话；存在活动风险时先请求确认。
    pub(crate) fn request_close_local_session(
        &mut self,
        session_id: LocalSessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_close_local_session_with_keep_pinned(session_id, window, cx, false);
    }

    pub(crate) fn request_close_local_session_with_keep_pinned(
        &mut self,
        session_id: LocalSessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
        keep_pinned: bool,
    ) {
        let Some(risk) = self.local_session_close_risk(session_id, cx) else {
            return;
        };
        if !risk.needs_confirmation() {
            self.close_local_session_internal(session_id, keep_pinned, cx);
            return;
        }
        self.prompt_close_tab(risk, window, cx, move |this, cx| {
            this.close_local_session_internal(session_id, keep_pinned, cx);
        });
    }

    pub(super) fn remote_tab_close_risk(
        &self,
        idx: usize,
        cx: &Context<Self>,
    ) -> Option<TabCloseRisk> {
        let tab = self.workspace.sessions.remote_tabs.get(idx)?;
        let pane_risk = tab.pane.risk(cx);
        Some(TabCloseRisk {
            command_running: tab.pane.is_command_running(cx),
            sftp_writes: pane_risk.sftp_writes,
            unsaved_editors: pane_risk.unsaved_editors,
            active_forwards: pane_risk.active_forwards,
        })
    }

    pub(crate) fn local_session_close_risk(
        &self,
        session_id: LocalSessionId,
        cx: &Context<Self>,
    ) -> Option<TabCloseRisk> {
        let session = self.workspace.sessions.local_sessions.get(&session_id)?;
        Some(TabCloseRisk {
            command_running: session.terminal.read(cx).is_command_running(cx),
            ..TabCloseRisk::default()
        })
    }

    /// 一键停止项目：关闭该项目下全部本地会话，但保留 recent/pinned（契约 2）。
    /// 批量快照 `Vec<LocalSessionId>` 后 `detach_splits_for` 再逐个经
    /// `request_close_local_session` 风险确认（契约 5），有风险的会话取消后保留。
    pub(crate) fn stop_local_project(
        &mut self,
        project_dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project_dir) = normalize_local_cwd(project_dir) else {
            return;
        };
        let ids: Vec<LocalSessionId> = match self.workspace.sessions.local_dirs.get(&project_dir) {
            Some(dir) => dir.sessions.clone(),
            None => return,
        };
        if ids.is_empty() {
            return;
        }
        let views: Vec<ActiveView> = ids.iter().copied().map(ActiveView::LocalSession).collect();
        self.detach_splits_for(&views, cx);
        for session_id in ids {
            if !self
                .workspace
                .sessions
                .local_sessions
                .contains_key(&session_id)
            {
                continue;
            }
            self.request_close_local_session_with_keep_pinned(session_id, window, cx, true);
        }
    }

    fn prompt_close_tab(
        &mut self,
        risk: TabCloseRisk,
        window: &mut Window,
        cx: &mut Context<Self>,
        on_confirm: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) {
        if self.tab_close_confirmation_open
            || self.quit_confirmation_open
            || self.shutdown_in_progress
        {
            return;
        }
        self.tab_close_confirmation_open = true;
        let answers = [
            PromptButton::ok(i18n::text("tab_close.confirm")),
            PromptButton::cancel(i18n::text("tab_close.cancel")),
        ];
        let answer = window.prompt(
            PromptLevel::Warning,
            &i18n::text("tab_close.title"),
            Some(&risk.detail()),
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

    pub(super) fn cycle_tab(&mut self, direction: isize, cx: &mut Context<Self>) {
        match self.workspace.active_view {
            Some(ActiveView::RemoteTab(current)) => {
                let visible = (0..self.workspace.sessions.remote_tabs.len())
                    .filter(|&idx| {
                        !self
                            .workspace
                            .is_split_secondary(ActiveView::RemoteTab(idx))
                    })
                    .collect::<Vec<_>>();
                let Some(next) = next_cycle_index(current, &visible, direction) else {
                    return;
                };
                self.switch_remote_tab(visible[next], cx);
            }
            Some(ActiveView::LocalSession(session_id)) => {
                let session_ids = self
                    .local_dir_for_session(session_id)
                    .map(|dir| dir.sessions.clone())
                    .unwrap_or_default();
                let visible = session_ids
                    .into_iter()
                    .filter(|&id| {
                        !self
                            .workspace
                            .is_split_secondary(ActiveView::LocalSession(id))
                    })
                    .collect::<Vec<_>>();
                if let Some(next) = next_cycle_index(session_id, &visible, direction)
                    && let Some(next_session) = visible.get(next).copied()
                {
                    self.select_local_session(next_session, cx);
                }
            }
            None => {}
        }
    }

    /// 从当前标签复制一个终端标签；没有活动标签时把焦点放到快速连接框。
    pub(crate) fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.workspace.active_view {
            Some(ActiveView::LocalSession(session_id)) => {
                let project_dir = self.local_session_project_dir(session_id);
                let cwd = self.local_session_cwd(session_id, cx);
                let _ = self.open_local_session(project_dir, cwd, cx);
                return;
            }
            Some(ActiveView::RemoteTab(idx)) => {
                if let Some(tab) = self.workspace.sessions.remote_tabs.get(idx) {
                    let target = tab.target.clone();
                    self.open_terminal_target(target, cx);
                    return;
                }
            }
            None => {}
        }
        self.search_query.clear();
        self.search_ime_marked_text.clear();
        self.search_focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn local_session_cwd(
        &self,
        session_id: LocalSessionId,
        cx: &Context<Self>,
    ) -> PathBuf {
        self.workspace
            .sessions
            .local_sessions
            .get(&session_id)
            .map(|session| {
                session
                    .terminal
                    .read(cx)
                    .cwd
                    .as_deref()
                    .and_then(|cwd| normalize_local_cwd(PathBuf::from(cwd)))
                    .unwrap_or_else(|| session.cwd.clone())
            })
            .unwrap_or_else(current_local_cwd)
    }

    pub(super) fn local_session_project_dir(&self, session_id: LocalSessionId) -> PathBuf {
        self.workspace
            .sessions
            .local_sessions
            .get(&session_id)
            .map(|session| session.project_dir.clone())
            .unwrap_or_else(current_local_cwd)
    }

    /// 关闭除 `keep` 外的全部远程标签。
    pub(super) fn close_other_remote_tabs(&mut self, keep: usize, cx: &mut Context<Self>) {
        if keep >= self.workspace.sessions.remote_tabs.len() {
            return;
        }
        let owners = self
            .workspace
            .sessions
            .remote_tabs
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != keep)
            .filter_map(|(_, tab)| remote_tab_background_owner(tab))
            .collect::<Vec<_>>();
        // 批量清扫：只拆与被清扫 Tab 相关的分栏状态，右窗格会话随清扫
        // 一视同仁地关闭，避免退休路径提前删除右窗格造成 keep 索引漂移。
        let swept = (0..self.workspace.sessions.remote_tabs.len())
            .filter(|index| *index != keep)
            .map(ActiveView::RemoteTab)
            .collect::<Vec<_>>();
        self.detach_splits_for(&swept, cx);
        for owner in owners {
            self.stop_background_tasks_for_owner(&owner, cx);
        }
        for (index, tab) in self.workspace.sessions.remote_tabs.iter().enumerate() {
            if index != keep {
                tab.pane.cleanup(cx);
            }
        }
        // 终端级 compose：keep 的草稿需从旧索引迁移到 0
        if keep != 0
            && let Some(entry) = self.workspace.compose.remove(&ActiveView::RemoteTab(keep))
        {
            self.workspace
                .compose
                .insert(ActiveView::RemoteTab(0), entry);
        }
        self.workspace.sessions.remote_tabs =
            vec![self.workspace.sessions.remote_tabs.swap_remove(keep)];
        self.workspace.active_view = Some(ActiveView::RemoteTab(0));
        cx.notify();
    }

    pub(super) fn close_all_remote_tabs(&mut self, cx: &mut Context<Self>) {
        if self.workspace.sessions.remote_tabs.is_empty() {
            return;
        }
        let owners = self
            .workspace
            .sessions
            .remote_tabs
            .iter()
            .filter_map(remote_tab_background_owner)
            .collect::<Vec<_>>();
        let swept = (0..self.workspace.sessions.remote_tabs.len())
            .map(ActiveView::RemoteTab)
            .collect::<Vec<_>>();
        self.detach_splits_for(&swept, cx);
        for owner in owners {
            self.stop_background_tasks_for_owner(&owner, cx);
        }
        for tab in &self.workspace.sessions.remote_tabs {
            tab.pane.cleanup(cx);
        }
        self.workspace.sessions.remote_tabs.clear();
        self.workspace.active_view = self.first_local_view();
        cx.notify();
    }

    /// 关闭同一目录下的其他本地会话（保留 `keep`）。
    pub(super) fn close_other_local_sessions(
        &mut self,
        keep: LocalSessionId,
        cx: &mut Context<Self>,
    ) {
        let Some(others) = self.local_dir_for_session(keep).map(|dir| {
            dir.sessions
                .iter()
                .copied()
                .filter(|id| *id != keep)
                .collect::<Vec<_>>()
        }) else {
            return;
        };
        let swept = others
            .iter()
            .copied()
            .map(ActiveView::LocalSession)
            .collect::<Vec<_>>();
        self.detach_splits_for(&swept, cx);
        for session_id in others {
            self.close_local_session(session_id, cx);
        }
        self.select_local_session(keep, cx);
    }

    pub(crate) fn pin_local_session(&mut self, session_id: LocalSessionId, cx: &mut Context<Self>) {
        let Some(session) = self.workspace.sessions.local_sessions.get_mut(&session_id) else {
            return;
        };
        if session.pin_id.is_some() {
            return;
        }
        let pin_id = next_pin_id(&self.workspace_settings.pinned_local_tabs);
        session.pin_id = Some(pin_id);
        self.workspace_settings
            .pinned_local_tabs
            .push(PinnedLocalTab {
                pin_id,
                project_dir: session.project_dir.clone(),
                cwd: self.local_session_cwd(session_id, cx),
                custom_name: None,
                default_command: None,
            });
        self.persist_settings();
        cx.notify();
    }

    /// 取消固定：会话保持打开但回到普通标签行为，持久化记录移除。
    pub(crate) fn unpin_local_session(
        &mut self,
        session_id: LocalSessionId,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.workspace.sessions.local_sessions.get_mut(&session_id) else {
            return;
        };
        let Some(pin_id) = session.pin_id.take() else {
            return;
        };
        self.workspace_settings
            .pinned_local_tabs
            .retain(|tab| tab.pin_id != pin_id);
        self.persist_settings();
        cx.notify();
    }

    /// 打开固定标签的重命名弹窗（初始值取当前自定义名称；空白表示回退默认标题）。
    /// 与 Quick Command 编辑器互斥，打开时静默关闭另一个模态。
    pub(crate) fn open_rename_local_session(
        &mut self,
        session_id: LocalSessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.workspace.sessions.local_sessions.get(&session_id) else {
            return;
        };
        let focus = cx.focus_handle();
        self.quick_command_editor = None;
        self.rename_editor = Some(RenameEditor::new(
            session_id,
            session.custom_name.clone().unwrap_or_default(),
            focus.clone(),
        ));
        window.focus(&focus, cx);
        cx.notify();
    }

    /// 提交重命名：空白清除名称（契约 4），否则覆盖并持久化到固定记录。
    pub(crate) fn submit_rename_local_session(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.rename_editor.take() else {
            return;
        };
        let name = editor.state.value.clone();
        let Some(session) = self
            .workspace
            .sessions
            .local_sessions
            .get_mut(&editor.session_id)
        else {
            // 弹窗期间会话已关闭：静默忽略，不写持久化。
            cx.notify();
            return;
        };
        let custom_name = (!name.trim().is_empty()).then(|| name.trim().to_string());
        session.custom_name = custom_name.clone();
        if let Some(pin_id) = session.pin_id
            && let Some(tab) = self
                .workspace_settings
                .pinned_local_tabs
                .iter_mut()
                .find(|tab| tab.pin_id == pin_id)
        {
            tab.custom_name = custom_name;
            self.persist_settings();
        }
        cx.notify();
    }

    pub(crate) fn cancel_rename_local_session(&mut self, cx: &mut Context<Self>) {
        if self.rename_editor.take().is_some() {
            cx.notify();
        }
    }

    /// 打开默认命令编辑弹窗（初始值取当前 default_command；空白表示清除）。
    /// 与其他模态互斥。
    pub(crate) fn open_default_command_editor(
        &mut self,
        session_id: LocalSessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.workspace.sessions.local_sessions.get(&session_id) else {
            return;
        };
        if session.pin_id.is_none() {
            return;
        }
        let focus = cx.focus_handle();
        self.quick_command_editor = None;
        self.rename_editor = None;
        self.default_command_editor = Some(DefaultCommandEditor::new(
            session_id,
            session.default_command.clone().unwrap_or_default(),
            focus.clone(),
        ));
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(crate) fn submit_default_command(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.default_command_editor.take() else {
            return;
        };
        let raw = editor.state.value.clone();
        let trimmed = raw.trim().to_string();
        let new_command = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        };
        let Some(session) = self
            .workspace
            .sessions
            .local_sessions
            .get_mut(&editor.session_id)
        else {
            cx.notify();
            return;
        };
        session.default_command = new_command.clone();
        if let Some(pin_id) = session.pin_id
            && let Some(tab) = self
                .workspace_settings
                .pinned_local_tabs
                .iter_mut()
                .find(|tab| tab.pin_id == pin_id)
        {
            tab.default_command = new_command;
            // normalized 会在 persist 前 trim+空白归一，但此处已处理
            self.persist_settings();
        }
        cx.notify();
    }

    pub(crate) fn cancel_default_command(&mut self, cx: &mut Context<Self>) {
        if self.default_command_editor.take().is_some() {
            cx.notify();
        }
    }

    /// 重载默认命令到终端；空闲时才执行（契约 5/6）。
    pub(crate) fn reload_default_command(
        &mut self,
        session_id: LocalSessionId,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.workspace.sessions.local_sessions.get(&session_id) else {
            return;
        };
        let Some(cmd) = session.default_command.clone() else {
            return;
        };
        if cmd.trim().is_empty() {
            return;
        }
        if session.pin_id.is_none() {
            return;
        }
        if session.terminal.read(cx).is_command_running(cx) {
            return;
        }
        session.terminal.update(cx, |terminal, terminal_cx| {
            terminal.run_command(&cmd, terminal_cx)
        });
        cx.notify();
    }

    pub(crate) fn clear_default_command(
        &mut self,
        session_id: LocalSessionId,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.workspace.sessions.local_sessions.get_mut(&session_id) else {
            return;
        };
        if session.pin_id.is_none() {
            return;
        }
        if session.default_command.is_none() {
            return;
        }
        session.default_command = None;
        if let Some(pin_id) = session.pin_id
            && let Some(tab) = self
                .workspace_settings
                .pinned_local_tabs
                .iter_mut()
                .find(|tab| tab.pin_id == pin_id)
        {
            tab.default_command = None;
            self.persist_settings();
        }
        cx.notify();
    }

    /// 按固定记录顺序重开会话（契约 11 恢复路径）：把 `pin_id` 与自定义
    /// 名称应用到**本次恢复创建的新会话**（显式会话 id），目录已失效的
    /// 记录跳过并即时从持久化设置清理（契约 11 Rev-4）。
    pub(super) fn restore_pinned_local_tabs(
        &mut self,
        tabs: Vec<PinnedLocalTab>,
        cx: &mut Context<Self>,
    ) {
        let mut removed_stale = false;
        for tab in tabs {
            let pin_id = tab.pin_id;
            let custom_name = tab.custom_name.clone();
            let default_command = tab.default_command.clone();
            let Some(session_id) =
                self.open_local_session(tab.project_dir.clone(), tab.cwd.clone(), cx)
            else {
                // 记录目录已失效（删除/改名/不可访问）：跳过恢复并即时
                // 清理，等价契约 8 的关闭清理，不等待下次启动。
                self.workspace_settings
                    .pinned_local_tabs
                    .retain(|entry| entry.pin_id != pin_id);
                removed_stale = true;
                continue;
            };
            self.apply_pin_to_session(session_id, pin_id, custom_name, default_command.clone(), cx);
            // 自动执行默认命令（契约 4）：恢复后若配置了 default_command，延迟到终端 Connected 再执行
            // `open_local_session` 创建的 TerminalView 初始为 Connecting（display-only），立即 send_input 会丢失；
            // 需等待 Zed TerminalBuilder 完成 attach 后再投递。
            if let Some(cmd) = default_command
                && !cmd.trim().is_empty()
            {
                let cmd = cmd.trim().to_string();
                if let Some(session) = self.workspace.sessions.local_sessions.get(&session_id) {
                    let terminal = session.terminal.clone();
                    cx.spawn(async move |weak, cx| {
                        // 最多等待 4s，轮询终端是否已 Connected 且空闲
                        for _ in 0..40 {
                            cx.background_executor()
                                .timer(Duration::from_millis(100))
                                .await;
                            let ready = weak
                                .update(cx, |this, cx| {
                                    this.workspace
                                        .sessions
                                        .local_sessions
                                        .get(&session_id)
                                        .is_some_and(|s| {
                                            s.terminal.entity_id() == terminal.entity_id()
                                                && s.terminal.read(cx).state == ConnState::Connected
                                                && !s.terminal.read(cx).is_command_running(cx)
                                        })
                                })
                                .unwrap_or(false);
                            if ready {
                                let _ = weak.update(cx, |this, cx| {
                                    if let Some(s) =
                                        this.workspace.sessions.local_sessions.get(&session_id)
                                    {
                                        s.terminal
                                            .update(cx, |t, term_cx| t.run_command(&cmd, term_cx));
                                    }
                                });
                                break;
                            }
                        }
                    })
                    .detach();
                }
            }
        }
        if removed_stale {
            self.persist_settings();
        }
    }

    /// 激活项目时恢复该项目尚无会话的固定记录（契约 11）。
    /// 幂等：已有对应 `pin_id` 会话的记录（用户手动固定或先前已恢复）
    /// 不重复打开，也不改写既有会话内容。
    pub(super) fn restore_pinned_tabs_for_project(
        &mut self,
        project_dir: &Path,
        cx: &mut Context<Self>,
    ) {
        let open_pin_ids = self
            .workspace
            .sessions
            .local_sessions
            .values()
            .filter_map(|session| session.pin_id)
            .collect::<BTreeSet<_>>();
        let pending =
            pinned_tabs_for_project(&self.workspace_settings.pinned_local_tabs, project_dir)
                .into_iter()
                .filter(|tab| !open_pin_ids.contains(&tab.pin_id))
                .cloned()
                .collect::<Vec<_>>();
        self.restore_pinned_local_tabs(pending, cx);
    }

    /// 把固定记录的状态应用到指定的本地会话（恢复路径专用；会话创建由
    /// `open_local_session` 完成）。以显式 `session_id` 为目标，不依赖
    /// 「当前活动会话」，避免失效记录污染其他会话（契约 11 Rev-4）。
    pub(super) fn apply_pin_to_session(
        &mut self,
        session_id: LocalSessionId,
        pin_id: u64,
        custom_name: Option<String>,
        default_command: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.workspace.sessions.local_sessions.get_mut(&session_id) {
            session.pin_id = Some(pin_id);
            session.custom_name = custom_name;
            // default_command 已做 trim/空白归一
            session.default_command = default_command
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty());
            cx.notify();
        }
    }

    pub(super) fn first_local_view(&self) -> Option<ActiveView> {
        self.workspace
            .sessions
            .local_dirs
            .values()
            .find_map(|dir| dir.active_session.map(ActiveView::LocalSession))
    }

    /// 把焦点交还给当前活动终端 tab（切换 tab / 关闭模态后调用）。
    pub(crate) fn refocus_active_terminal(&self, cx: &mut Context<Self>) {
        match self.workspace.focused_view() {
            Some(ActiveView::RemoteTab(idx)) => {
                if let Some(tab) = self.workspace.sessions.remote_tabs.get(idx) {
                    tab.pane.request_focus(cx);
                }
            }
            Some(ActiveView::LocalSession(session_id)) => {
                if let Some(session) = self.workspace.sessions.local_sessions.get(&session_id) {
                    session
                        .terminal
                        .update(cx, |terminal, _| terminal.request_focus());
                }
            }
            None => {}
        }
    }
}

fn next_cycle_index<T: PartialEq>(current: T, visible: &[T], direction: isize) -> Option<usize> {
    if visible.len() <= 1 {
        return None;
    }
    let position = visible.iter().position(|index| index == &current)?;
    let next = (position as isize + direction).rem_euclid(visible.len() as isize) as usize;
    (next != position).then_some(next)
}

fn zed_ssh_shell(target: &str, host: &HostConfig) -> Shell {
    let direct_target = target.contains('@')
        || target
            .rsplit_once(':')
            .is_some_and(|(_, port)| port.parse::<u16>().is_ok());
    let destination = if direct_target {
        host.effective_host().to_string()
    } else {
        target.to_string()
    };

    let mut args = vec!["-tt".to_string()];
    if direct_target {
        if let Some(user) = &host.user {
            args.extend(["-l".to_string(), user.clone()]);
        }
        if let Some(port) = host.port {
            args.extend(["-p".to_string(), port.to_string()]);
        }
    }
    args.push(destination);
    args.push(remote_shell_bootstrap_command());

    Shell::WithArguments {
        program: "ssh".to_string(),
        args,
        title_override: Some(format!("{} - Crossh", target)),
    }
}

#[cfg(test)]
mod tests {
    use crate::shared::i18n;

    use super::{SplitPaneRetirement, TabCloseRisk, next_cycle_index, split_pane_retirement};

    #[test]
    fn cycle_skips_split_secondary_and_stops_when_only_one_view_is_visible() {
        assert_eq!(next_cycle_index(0, &[0, 2, 4], 1), Some(1));
        assert_eq!(next_cycle_index(2, &[0, 2, 4], -1), Some(0));
        assert_eq!(next_cycle_index(0, &[0], 1), None);
        assert_eq!(next_cycle_index(1, &[0, 2], 1), None);
    }

    #[test]
    fn close_confirmation_is_only_required_for_material_activity() {
        assert!(!TabCloseRisk::default().needs_confirmation());

        for risk in [
            TabCloseRisk {
                command_running: true,
                ..Default::default()
            },
            TabCloseRisk {
                sftp_writes: 1,
                ..Default::default()
            },
            TabCloseRisk {
                unsaved_editors: 1,
                ..Default::default()
            },
            TabCloseRisk {
                active_forwards: 1,
                ..Default::default()
            },
        ] {
            assert!(risk.needs_confirmation());
        }
    }

    #[test]
    fn close_risk_detail_describes_what_will_be_interrupted() {
        let detail = TabCloseRisk {
            command_running: true,
            sftp_writes: 2,
            unsaved_editors: 1,
            active_forwards: 1,
        }
        .detail();
        assert!(detail.contains("2"));
        assert!(detail.contains(&i18n::text("tab_close.consequence")));
        assert!(detail.contains(&i18n::text("tab_close.running")));
    }

    #[test]
    fn split_pane_retirement_destroys_idle_panes_and_keeps_busy_ones() {
        assert_eq!(split_pane_retirement(None), SplitPaneRetirement::Close);
        assert_eq!(
            split_pane_retirement(Some(&TabCloseRisk::default())),
            SplitPaneRetirement::Close
        );
        for risky in [
            TabCloseRisk {
                command_running: true,
                ..Default::default()
            },
            TabCloseRisk {
                sftp_writes: 1,
                ..Default::default()
            },
            TabCloseRisk {
                unsaved_editors: 1,
                ..Default::default()
            },
            TabCloseRisk {
                active_forwards: 1,
                ..Default::default()
            },
        ] {
            assert_eq!(
                split_pane_retirement(Some(&risky)),
                SplitPaneRetirement::KeepAsTab
            );
        }
    }
}

//! AppShell terminal tab and session navigation.

use gpui::{PromptButton, PromptLevel};

use task::Shell;

use crossh_core::terminal::remote_shell_bootstrap_command;

use super::*;

/// 单个标签页关闭时可能被打断的活动；任何一项存在都需要确认。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TabCloseRisk {
    command_running: bool,
    sftp_writes: usize,
    unsaved_editors: usize,
    active_forwards: usize,
}

impl TabCloseRisk {
    fn needs_confirmation(&self) -> bool {
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
    pub(super) fn open_terminal_target(&mut self, target: String, cx: &mut Context<Self>) {
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
        let subscription = cx.subscribe(&terminal, move |this, terminal, event, cx| match event {
            TerminalEvent::Closed => {
                this.close_remote_terminal(terminal.entity_id(), cx);
            }
            TerminalEvent::TitleChanged | TerminalEvent::Notification => cx.notify(),
            TerminalEvent::CommandStarted { command, cwd } => {
                if !terminal.read(cx).is_local()
                    && let Some(cwd) = cwd.as_deref()
                {
                    this.record_command(remote_scope(&event_host_key, cwd), command.clone(), cx);
                }
            }
            TerminalEvent::CommandFinished { status } => {
                log::debug!("remote terminal command finished with status {status:?}");
            }
            TerminalEvent::CwdChanged => cx.notify(),
            TerminalEvent::PromptReached => {}
        });
        self.workspace
            .sessions
            .terminal_subscriptions
            .push(subscription);
        self.workspace.sessions.remote_tabs.push(Tab {
            target,
            host_key,
            connection: None,
            pane: crate::features::terminal::view::workspace_pane(terminal),
        });
        self.workspace.active_view = Some(ActiveView::RemoteTab(
            self.workspace.sessions.remote_tabs.len() - 1,
        ));
        self.status = None;
        cx.notify();
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
        if idx >= self.workspace.sessions.remote_tabs.len() {
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
        match self.workspace.active_view {
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
        let Some(risk) = self.local_session_close_risk(session_id, cx) else {
            return;
        };
        if !risk.needs_confirmation() {
            self.close_local_session(session_id, cx);
            return;
        }
        self.prompt_close_tab(risk, window, cx, move |this, cx| {
            this.close_local_session(session_id, cx);
        });
    }

    fn remote_tab_close_risk(&self, idx: usize, cx: &Context<Self>) -> Option<TabCloseRisk> {
        let tab = self.workspace.sessions.remote_tabs.get(idx)?;
        let pane_risk = tab.pane.risk(cx);
        Some(TabCloseRisk {
            command_running: tab.pane.is_command_running(cx),
            sftp_writes: pane_risk.sftp_writes,
            unsaved_editors: pane_risk.unsaved_editors,
            active_forwards: pane_risk.active_forwards,
        })
    }

    fn local_session_close_risk(
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
                let len = self.workspace.sessions.remote_tabs.len();
                if len == 0 {
                    return;
                }
                let next = (current as isize + direction).rem_euclid(len as isize) as usize;
                self.switch_remote_tab(next, cx);
            }
            Some(ActiveView::LocalSession(session_id)) => {
                let Some(dir) = self.local_dir_for_session(session_id) else {
                    return;
                };
                let Some(current) = dir.sessions.iter().position(|id| *id == session_id) else {
                    return;
                };
                let next =
                    (current as isize + direction).rem_euclid(dir.sessions.len() as isize) as usize;
                if let Some(next_session) = dir.sessions.get(next).copied() {
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
                self.open_local_session(project_dir, cwd, cx);
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
        self.host_query.clear();
        self.host_ime_marked_text.clear();
        self.host_focus.focus(window, cx);
        cx.notify();
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
        for owner in owners {
            self.stop_background_tasks_for_owner(&owner, cx);
        }
        for (index, tab) in self.workspace.sessions.remote_tabs.iter().enumerate() {
            if index != keep {
                tab.pane.cleanup(cx);
            }
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
        for session_id in others {
            self.close_local_session(session_id, cx);
        }
        self.select_local_session(keep, cx);
    }

    pub(super) fn first_local_view(&self) -> Option<ActiveView> {
        self.workspace
            .sessions
            .local_dirs
            .values()
            .find_map(|dir| dir.active_session.map(ActiveView::LocalSession))
    }

    /// 把焦点交还给当前活动终端 tab（切换 tab / 关闭模态后调用）。
    pub(super) fn refocus_active_terminal(&self, cx: &mut Context<Self>) {
        match self.workspace.active_view {
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

    use super::TabCloseRisk;

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
}

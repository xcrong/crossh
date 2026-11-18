//! daemon 工作区连接：attach、快照同步与 pane 终端视图生命周期。

use std::sync::Arc;
use std::time::Duration;

use gpui::{Context, EntityId};

use crossh_session::WorkspaceConnection;
use crossh_session::workspace::{PaneId, ProjectId, TabId, WorkspaceOp};
use crossh_terminal::ConnState;
use crossh_terminal_view::{TerminalEvent, TerminalItem as _, TerminalView};

use crate::features::workspace::projection;
use crate::features::workspace::toaster::{ToastNotice, ToastTone};

use super::AppShell;

impl AppShell {
    /// 启动工作区连接：成功后订阅快照；断开后保留最后布局并每 2 s 重连。
    pub(crate) fn start_workspace_connection(&mut self, cx: &mut Context<Self>) {
        if self._workspace_connection_task.is_some() {
            return;
        }
        self._workspace_connection_task = Some(cx.spawn(async move |weak, cx| {
            loop {
                let connecting = weak
                    .update(cx, |this, cx| {
                        this.workspace.connection = ConnState::Connecting;
                        this.status = Some(crossh_core::i18n::text("workspace.connecting"));
                        cx.notify();
                        !this.shutdown_in_progress
                    })
                    .unwrap_or(false);
                if !connecting {
                    break;
                }

                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                crate::app::bootstrap::runtime().spawn(async move {
                    let _ = tx.send(WorkspaceConnection::connect().await);
                });
                let connected = match rx.recv().await {
                    Some(Ok(connection)) => {
                        let connection = Arc::new(connection);
                        let snapshot = connection.current();
                        let applied = weak
                            .update(cx, |this, cx| {
                                this.workspace.connection = ConnState::Connected;
                                this.workspace_ready = true;
                                this.workspace_connection = Some(connection.clone());
                                this.status = None;
                                this.apply_workspace_snapshot((*snapshot).clone(), cx);
                                cx.notify();
                            })
                            .is_ok();
                        applied.then_some(connection)
                    }
                    Some(Err(error)) => {
                        let message = error.to_string();
                        let _ = weak.update(cx, |this, cx| {
                            this.workspace.connection = ConnState::Error(message.clone());
                            this.workspace_connection = None;
                            this.status = Some(
                                rust_i18n::t!(
                                    "workspace.connection_failed",
                                    error = message.as_str()
                                )
                                .to_string(),
                            );
                            cx.notify();
                        });
                        None
                    }
                    None => None,
                };

                if let Some(connection) = connected {
                    let mut snapshots = connection.snapshots();
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(100))
                            .await;
                        match snapshots.has_changed() {
                            Ok(false) => continue,
                            Ok(true) => {
                                let snapshot = snapshots.borrow_and_update().clone();
                                let _ = weak.update(cx, |this, cx| {
                                    this.apply_workspace_snapshot((*snapshot).clone(), cx);
                                });
                            }
                            Err(_) => break,
                        }
                    }
                    let _ = weak.update(cx, |this, cx| {
                        this.workspace.connection =
                            ConnState::Error("workspace connection closed".into());
                        this.workspace_connection = None;
                        this.status = Some(crossh_core::i18n::text("workspace.reconnecting"));
                        cx.notify();
                    });
                }

                let retry = weak
                    .update(cx, |this, _| !this.shutdown_in_progress)
                    .unwrap_or(false);
                if !retry {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        }));
    }

    pub(crate) fn apply_workspace_snapshot(
        &mut self,
        snapshot: crossh_session::workspace::WorkspaceSnapshot,
        cx: &mut Context<Self>,
    ) {
        let current = self.workspace.snapshot.clone();
        self.workspace.snapshot = projection::merge_snapshot(current, snapshot);
        self.sync_pane_views(cx);
        cx.notify();
    }

    /// 按快照对齐终端视图：会话 id 变化（daemon 重启）时重建同一 pane 的视图。
    fn sync_pane_views(&mut self, cx: &mut Context<Self>) {
        let live = self.workspace.live_panes();
        self.workspace.retain_panes(&live);

        let panes = self
            .workspace
            .snapshot
            .projects
            .iter()
            .flat_map(|project| &project.tabs)
            .flat_map(|tab| tab.layout.leaves())
            .cloned()
            .collect::<Vec<_>>();

        for pane in panes {
            let Some(session_id) = pane.session.clone() else {
                self.workspace.pane_views.remove(&pane.id);
                self.workspace.pane_sessions.remove(&pane.id);
                self.workspace.pane_subscriptions.remove(&pane.id);
                continue;
            };
            let needs_view = match self.workspace.pane_sessions.get(&pane.id) {
                Some(existing) => existing != &session_id,
                None => true,
            };
            if !needs_view {
                continue;
            }
            self.workspace.pane_views.remove(&pane.id);
            self.workspace.pane_subscriptions.remove(&pane.id);
            self.workspace
                .pane_sessions
                .insert(pane.id, session_id.clone());
            let terminal = TerminalView::from_daemon_attach(
                session_id,
                pane.cwd.clone(),
                self.terminal_settings.clone(),
                cx,
            );
            let pane_id = pane.id;
            let subscription = cx.subscribe(
                &terminal,
                move |this, terminal, event: &TerminalEvent, cx| {
                    this.handle_pane_terminal_event(pane_id, terminal.entity_id(), event, cx);
                },
            );
            self.workspace
                .pane_subscriptions
                .insert(pane.id, subscription);
            self.workspace.pane_views.insert(pane.id, terminal);
        }
    }

    fn handle_pane_terminal_event(
        &mut self,
        pane_id: PaneId,
        terminal_id: EntityId,
        event: &TerminalEvent,
        cx: &mut Context<Self>,
    ) {
        if self
            .workspace
            .pane_view(pane_id)
            .is_none_or(|view| view.entity_id() != terminal_id)
        {
            return;
        }
        match event {
            // 布局关闭只由快照 diff 驱动；进程退出后 daemon 会把 pane 转回可物化状态。
            TerminalEvent::Closed => {
                cx.notify();
            }
            TerminalEvent::CwdChanged => {
                self.refresh_git_status(pane_id, true, cx);
                cx.notify();
            }
            TerminalEvent::PromptReached => {
                self.refresh_git_status(pane_id, false, cx);
                self.maybe_run_default_command(pane_id, cx);
            }
            TerminalEvent::SessionAttached => {
                self.schedule_default_command(pane_id, cx);
            }
            TerminalEvent::CommandStarted { .. } => {}
            TerminalEvent::CommandFinished { status } => {
                log::debug!("pane {pane_id} command finished with status {status:?}");
            }
            TerminalEvent::ClipboardCopied => {
                self.show_toast(
                    ToastNotice::new(crossh_core::i18n::text("toast.copied"), ToastTone::Success),
                    cx,
                );
            }
            TerminalEvent::ClipboardPasted => {
                self.show_toast(
                    ToastNotice::new(crossh_core::i18n::text("toast.pasted"), ToastTone::Success),
                    cx,
                );
            }
            TerminalEvent::TitleChanged | TerminalEvent::Notification => cx.notify(),
            TerminalEvent::ProgramStatusChanged => cx.notify(),
        }
    }

    fn schedule_default_command(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        cx.spawn(async move |weak, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let _ = weak.update(cx, |this, cx| {
                this.maybe_run_default_command(pane_id, cx);
            });
        })
        .detach();
    }

    /// tab 的默认命令只在本次 attach 后执行一次；daemon 只负责持久化。
    fn maybe_run_default_command(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        let Some(location) = projection::locate_pane(&self.workspace.snapshot, pane_id) else {
            return;
        };
        let tab_id = location.tab.id;
        let Some(command) = location
            .tab
            .default_command
            .clone()
            .filter(|command| !command.trim().is_empty())
        else {
            return;
        };
        if self.workspace.default_command_runs.contains(&tab_id) {
            return;
        }
        let Some(view) = self.workspace.pane_view(pane_id).cloned() else {
            return;
        };
        if view.is_command_running(cx) {
            return;
        }
        self.workspace.default_command_runs.insert(tab_id);
        view.run_command(&command, cx);
    }

    pub(crate) fn open_project_root(&mut self, root: std::path::PathBuf, cx: &mut Context<Self>) {
        let Some(root) = super::normalize_local_cwd(root) else {
            self.show_toast(
                ToastNotice::new(
                    crossh_core::i18n::text("workspace.path_missing"),
                    ToastTone::Error,
                ),
                cx,
            );
            return;
        };
        self.send_workspace_op(WorkspaceOp::OpenProject { root }, cx);
    }

    pub(crate) fn focus_pane(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        if self.workspace.focused_pane() != Some(pane_id) {
            self.send_workspace_op(WorkspaceOp::ActivatePane { pane_id }, cx);
        }
        if let Some(view) = self.workspace.pane_view(pane_id) {
            view.request_focus(cx);
        }
    }

    pub(crate) fn ensure_pane_session(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        self.send_workspace_op(WorkspaceOp::EnsurePaneSession { pane_id }, cx);
    }

    pub(crate) fn create_tab(&mut self, project_id: Option<ProjectId>, cx: &mut Context<Self>) {
        self.send_workspace_op(
            WorkspaceOp::CreateTab {
                project_id,
                title: None,
                cwd: None,
                default_command: None,
            },
            cx,
        );
    }

    pub(crate) fn activate_tab(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        self.send_workspace_op(WorkspaceOp::ActivateTab { tab_id }, cx);
    }

    pub(crate) fn activate_project(&mut self, project_id: ProjectId, cx: &mut Context<Self>) {
        self.send_workspace_op(WorkspaceOp::ActivateProject { project_id }, cx);
    }

    /// 侧栏/空状态的项目入口：激活项目；项目还没有 tab 时补一个。
    pub(crate) fn open_project_entry(&mut self, project_id: ProjectId, cx: &mut Context<Self>) {
        let empty = self
            .workspace
            .project(project_id)
            .is_some_and(|project| project.tabs.is_empty());
        self.activate_project(project_id, cx);
        if empty {
            self.create_tab(Some(project_id), cx);
        }
    }
}

//! 工作区 actor：独占模型与运行时状态，串行处理命令并投递副作用。
//!
//! - `Workspace` 与 `Runtime` 的可变访问只在本模块；
//! - PTY spawn/close/resize 通过 [`Effect`] 交给运行时任务；
//! - 结构修改等待 writer 落盘 ack，元数据修改 300 ms 合并后写；
//! - 所有者镜像只由本模块在 bind/unbind 时写。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot, watch};

use crate::protocol::{WorkspaceMutateReply, WorkspaceMutateRequest};
use crate::workspace::{
    ApplyResult, CreatedIds, MutateOutcome, OpError, PaneId, PaneStatus, PaneStatusReason,
    ProjectId, Runtime, SessionId, StorageErrorInfo, StorageErrorKind, TabId, TermSize, Workspace,
    WorkspaceOp, WorkspaceSnapshot,
};
use crate::workspace_store::{PersistenceResult, SaveRequest, WorkspaceSaver, WorkspaceWriter};

pub type OwnershipMirror = Arc<RwLock<BTreeMap<SessionId, PaneRef>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneRef {
    pub project_id: ProjectId,
    pub tab_id: TabId,
    pub pane_id: PaneId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Spawn {
        pane_id: PaneId,
        cwd: PathBuf,
        size: TermSize,
    },
    CloseSession {
        session_id: SessionId,
    },
    ResizeSession {
        session_id: SessionId,
        size: TermSize,
    },
}

pub enum WorkspaceCommand {
    Mutate {
        request: WorkspaceMutateRequest,
        reply: oneshot::Sender<WorkspaceMutateReply>,
    },
    Get {
        reply: oneshot::Sender<Arc<WorkspaceSnapshot>>,
    },
    Attach {
        reply: oneshot::Sender<Arc<WorkspaceSnapshot>>,
    },
    AttachEnded,
    BindSession {
        pane_id: PaneId,
        session_id: SessionId,
    },
    SessionSpawnFailed {
        pane_id: PaneId,
    },
    SessionExited {
        session_id: SessionId,
    },
    PaneCwdChanged {
        session_id: SessionId,
        cwd: PathBuf,
    },
    ResizePane {
        session_id: SessionId,
        size: TermSize,
    },
    ClosePane {
        pane_id: PaneId,
    },
    CloseTab {
        tab_id: TabId,
    },
    SetPaneCwd {
        pane_id: PaneId,
        cwd: PathBuf,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

pub struct WorkspaceHandle {
    pub commands: mpsc::UnboundedSender<WorkspaceCommand>,
    pub snapshots: watch::Receiver<Arc<WorkspaceSnapshot>>,
}

pub struct WorkspaceActor {
    workspace: Workspace,
    runtime: Runtime,
    rx: mpsc::UnboundedReceiver<WorkspaceCommand>,
    result_rx: mpsc::UnboundedReceiver<PersistenceResult>,
    effects: mpsc::UnboundedSender<Effect>,
    snapshots: watch::Sender<Arc<WorkspaceSnapshot>>,
    mirror: OwnershipMirror,
    writer: WorkspaceWriter,
    pending_metadata: Option<(u64, Vec<u8>)>,
    metadata_deadline: Option<Instant>,
    last_published: WorkspaceSnapshot,
    shutting_down: bool,
}

/// 创建 actor，返回命令入口与首份快照订阅。
pub fn spawn_workspace_actor(
    workspace: Workspace,
    store: Arc<dyn WorkspaceSaver>,
    effects: mpsc::UnboundedSender<Effect>,
    mirror: OwnershipMirror,
) -> WorkspaceHandle {
    let runtime = Runtime::default();
    let snapshot = workspace.snapshot(&runtime);
    let initial = Arc::new(snapshot.clone());
    let (snapshots, snapshots_rx) = watch::channel(initial);
    let (commands, rx) = mpsc::unbounded_channel();
    let (result_tx, result_rx) = mpsc::unbounded_channel();
    let writer = WorkspaceWriter::spawn(store, result_tx);
    let actor = WorkspaceActor {
        workspace,
        runtime,
        rx,
        result_rx,
        effects,
        snapshots,
        mirror,
        writer,
        pending_metadata: None,
        metadata_deadline: None,
        last_published: snapshot,
        shutting_down: false,
    };
    tokio::spawn(actor.run());
    WorkspaceHandle {
        commands,
        snapshots: snapshots_rx,
    }
}

impl WorkspaceActor {
    async fn run(mut self) {
        loop {
            let deadline = self.metadata_deadline;
            enum Wake {
                Command(Option<WorkspaceCommand>),
                Persisted(Option<PersistenceResult>),
                Debounce,
            }
            let wake = tokio::select! {
                command = self.rx.recv() => Wake::Command(command),
                result = self.result_rx.recv() => Wake::Persisted(result),
                _ = async {
                    match deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
                        None => std::future::pending().await,
                    }
                } => Wake::Debounce,
            };
            match wake {
                Wake::Command(Some(command)) => {
                    if self.handle(command).await {
                        break;
                    }
                }
                Wake::Command(None) => break,
                Wake::Persisted(Some(result)) => self.on_persistence(result),
                Wake::Persisted(None) => {}
                Wake::Debounce => self.flush_metadata(),
            }
        }
    }

    /// 返回 `true` 表示 actor 应退出（shutdown 已处理）。
    async fn handle(&mut self, command: WorkspaceCommand) -> bool {
        match command {
            WorkspaceCommand::Mutate { request, reply } => {
                self.handle_mutate(request, Some(reply)).await;
                false
            }
            WorkspaceCommand::Get { reply } => {
                let _ = reply.send(Arc::new(self.workspace.snapshot(&self.runtime)));
                false
            }
            WorkspaceCommand::Attach { reply } => {
                self.attach();
                let _ = reply.send(Arc::new(self.workspace.snapshot(&self.runtime)));
                false
            }
            WorkspaceCommand::AttachEnded => {
                self.runtime.attached_clients = self.runtime.attached_clients.saturating_sub(1);
                false
            }
            WorkspaceCommand::BindSession {
                pane_id,
                session_id,
            } => {
                self.bind_session(pane_id, session_id);
                false
            }
            WorkspaceCommand::SessionSpawnFailed { pane_id } => {
                self.runtime.starting.remove(&pane_id);
                self.runtime.spawn_failed.insert(pane_id);
                self.publish();
                false
            }
            WorkspaceCommand::SessionExited { session_id } => {
                self.on_session_exited(&session_id).await;
                false
            }
            WorkspaceCommand::PaneCwdChanged { session_id, cwd } => {
                self.on_pane_cwd_changed(&session_id, cwd);
                false
            }
            WorkspaceCommand::ResizePane { session_id, size } => {
                self.on_resize(&session_id, size);
                false
            }
            WorkspaceCommand::ClosePane { pane_id } => {
                self.run_internal(WorkspaceOp::ClosePane { pane_id }).await;
                false
            }
            WorkspaceCommand::CloseTab { tab_id } => {
                self.run_internal(WorkspaceOp::CloseTab { tab_id }).await;
                false
            }
            WorkspaceCommand::SetPaneCwd { pane_id, cwd } => {
                self.run_internal(WorkspaceOp::SetPaneCwd { pane_id, cwd })
                    .await;
                false
            }
            WorkspaceCommand::Shutdown { reply } => {
                self.shutdown().await;
                let _ = reply.send(());
                true
            }
        }
    }

    async fn run_internal(&mut self, op: WorkspaceOp) {
        let request = WorkspaceMutateRequest {
            request_id: 0,
            expected_revision: None,
            op,
        };
        self.handle_mutate(request, None).await;
    }

    fn attach(&mut self) {
        self.runtime.attached_clients += 1;
        if self.runtime.attached_clients == 1
            && let Some(tab_id) = self.active_tab_id()
        {
            self.materialize_tab(tab_id);
        }
        self.publish();
    }

    async fn handle_mutate(
        &mut self,
        request: WorkspaceMutateRequest,
        reply: Option<oneshot::Sender<WorkspaceMutateReply>>,
    ) {
        if let Some(expected) = request.expected_revision
            && expected != self.workspace.revision
        {
            self.reply_conflict(request.request_id, expected, reply);
            return;
        }

        match &request.op {
            WorkspaceOp::RefreshAvailability => {
                self.reply_refresh_availability(request.request_id, reply);
                return;
            }
            WorkspaceOp::EnsurePaneSession { pane_id } => {
                self.handle_ensure_session(*pane_id, request.request_id, reply);
                return;
            }
            WorkspaceOp::SetPaneCwd { pane_id, cwd } => {
                let Some(status) = self.pane_status(*pane_id) else {
                    self.reply_rejected(reply, request.request_id, OpError::not_found("pane"));
                    return;
                };
                let result = self.workspace.set_pane_cwd_checked(*pane_id, cwd, &status);
                self.finish_layout_result(request.request_id, request, result, reply, false)
                    .await;
                return;
            }
            _ => {}
        }

        self.apply_mutation(request, reply).await;
    }

    fn reply_conflict(
        &self,
        request_id: u64,
        expected: u64,
        reply: Option<oneshot::Sender<WorkspaceMutateReply>>,
    ) {
        self.send_reply(
            reply,
            request_id,
            MutateOutcome::Conflict,
            CreatedIds::default(),
            Some(OpError::new(
                "conflict",
                format!(
                    "expected revision {expected}, current {}",
                    self.workspace.revision
                ),
            )),
            false,
            None,
            false,
        );
    }

    fn reply_refresh_availability(
        &mut self,
        request_id: u64,
        reply: Option<oneshot::Sender<WorkspaceMutateReply>>,
    ) {
        let after = self.workspace.snapshot(&self.runtime);
        let changed = !runtime_fields_equal(&self.last_published, &after);
        self.publish_with(after);
        self.send_reply(
            reply,
            request_id,
            if changed {
                MutateOutcome::Applied
            } else {
                MutateOutcome::Unchanged
            },
            CreatedIds::default(),
            None,
            false,
            None,
            false,
        );
    }

    /// 默认 apply 路径：先解绑受影响的会话，再应用 op，成功后就地落盘并回复。
    async fn apply_mutation(
        &mut self,
        request: WorkspaceMutateRequest,
        reply: Option<oneshot::Sender<WorkspaceMutateReply>>,
    ) {
        let panes = self.panes_to_unbind(&request.op);
        let closed_sessions = panes
            .iter()
            .filter_map(|pane_id| self.runtime.pane_session.get(pane_id).cloned())
            .collect::<Vec<_>>();
        self.unbind_panes(&panes);
        let result = self.workspace.apply(&request.op);
        if result.outcome != MutateOutcome::Applied {
            self.send_reply(
                reply,
                request.request_id,
                result.outcome,
                result.created,
                result.error,
                false,
                None,
                false,
            );
            return;
        }

        self.materialize_after_op(&request.op, &result.created);
        let structural = is_structural(&request.op);
        self.finish_applied_mutation(
            request.request_id,
            result.created,
            closed_sessions,
            structural,
            reply,
        )
        .await;
    }

    /// 成功 apply 后：结构修改等待落盘，元数据修改合并写；随后广播并回复。
    async fn finish_applied_mutation(
        &mut self,
        request_id: u64,
        created: CreatedIds,
        closed_sessions: Vec<SessionId>,
        structural: bool,
        reply: Option<oneshot::Sender<WorkspaceMutateReply>>,
    ) {
        if !structural {
            self.queue_metadata();
            self.publish();
            self.send_reply(
                reply,
                request_id,
                MutateOutcome::Applied,
                created,
                None,
                false,
                None,
                false,
            );
            return;
        }

        let persisted = self.persist_structural().await;
        for session_id in closed_sessions {
            let _ = self.effects.send(Effect::CloseSession { session_id });
        }
        let (durable, storage_error) = match persisted {
            Ok(()) => (true, None),
            Err(error) => {
                self.runtime.storage_error = Some(error.clone());
                (false, Some(error))
            }
        };
        self.publish();
        self.send_reply(
            reply,
            request_id,
            MutateOutcome::Applied,
            created,
            None,
            durable,
            storage_error,
            false,
        );
    }

    async fn finish_layout_result(
        &mut self,
        request_id: u64,
        request: WorkspaceMutateRequest,
        result: ApplyResult,
        reply: Option<oneshot::Sender<WorkspaceMutateReply>>,
        _runtime_only: bool,
    ) {
        if result.outcome != MutateOutcome::Applied {
            self.send_reply(
                reply,
                request_id,
                result.outcome,
                result.created,
                result.error,
                false,
                None,
                false,
            );
            return;
        }
        if is_structural(&request.op) {
            match self.persist_structural().await {
                Ok(()) => {
                    self.publish();
                    self.send_reply(
                        reply,
                        request_id,
                        MutateOutcome::Applied,
                        result.created,
                        None,
                        true,
                        None,
                        false,
                    );
                }
                Err(error) => {
                    self.runtime.storage_error = Some(error.clone());
                    self.publish();
                    self.send_reply(
                        reply,
                        request_id,
                        MutateOutcome::Applied,
                        result.created,
                        None,
                        false,
                        Some(error),
                        false,
                    );
                }
            }
        } else {
            self.queue_metadata();
            self.publish();
            self.send_reply(
                reply,
                request_id,
                MutateOutcome::Applied,
                result.created,
                None,
                false,
                None,
                false,
            );
        }
    }

    fn handle_ensure_session(
        &mut self,
        pane_id: PaneId,
        request_id: u64,
        reply: Option<oneshot::Sender<WorkspaceMutateReply>>,
    ) {
        let Some(status) = self.pane_status(pane_id) else {
            self.reply_rejected(reply, request_id, OpError::not_found("pane"));
            return;
        };
        match status {
            PaneStatus::Dormant { .. } if self.runtime.attached_clients > 0 => {
                self.start_pane(pane_id);
                self.publish();
                self.send_reply(
                    reply,
                    request_id,
                    MutateOutcome::Applied,
                    CreatedIds::default(),
                    None,
                    false,
                    None,
                    false,
                );
            }
            _ => {
                self.send_reply(
                    reply,
                    request_id,
                    MutateOutcome::Unchanged,
                    CreatedIds::default(),
                    None,
                    false,
                    None,
                    false,
                );
            }
        }
    }

    fn reply_rejected(
        &self,
        reply: Option<oneshot::Sender<WorkspaceMutateReply>>,
        request_id: u64,
        error: OpError,
    ) {
        self.send_reply(
            reply,
            request_id,
            MutateOutcome::Rejected,
            CreatedIds::default(),
            Some(error),
            false,
            None,
            false,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn send_reply(
        &self,
        reply: Option<oneshot::Sender<WorkspaceMutateReply>>,
        request_id: u64,
        outcome: MutateOutcome,
        created: CreatedIds,
        error: Option<OpError>,
        durable: bool,
        storage_error: Option<StorageErrorInfo>,
        _runtime_only: bool,
    ) {
        let Some(reply) = reply else {
            return;
        };
        let _ = reply.send(WorkspaceMutateReply {
            request_id,
            outcome,
            revision: self.workspace.revision,
            created,
            error,
            durable,
            storage_error,
        });
    }

    fn active_tab_id(&self) -> Option<TabId> {
        self.workspace
            .active_project_ref()
            .and_then(|project| project.active_tab)
    }

    fn materialize_after_op(&mut self, op: &WorkspaceOp, created: &CreatedIds) {
        if self.runtime.attached_clients == 0 {
            return;
        }
        match op {
            WorkspaceOp::OpenProject { .. } | WorkspaceOp::ActivateProject { .. } => {
                if let Some(tab_id) = self.active_tab_id() {
                    self.materialize_tab(tab_id);
                }
            }
            WorkspaceOp::CreateTab { .. } => {
                if let Some(tab_id) = created.tab {
                    self.materialize_tab(tab_id);
                }
            }
            WorkspaceOp::SplitPane { .. } => {
                if let Some(pane_id) = created.pane {
                    self.materialize_pane(pane_id);
                }
            }
            WorkspaceOp::ActivateTab { tab_id } => self.materialize_tab(*tab_id),
            WorkspaceOp::ActivatePane { pane_id } => {
                if let Some(tab_id) = self.workspace.locate_pane(*pane_id).map(|(_, tab, _)| tab) {
                    self.materialize_tab(tab_id);
                }
            }
            _ => {}
        }
    }

    fn materialize_tab(&mut self, tab_id: TabId) {
        let panes: Vec<PaneId> = self
            .workspace
            .tab(tab_id)
            .map(|tab| tab.layout.leaves().map(|pane| pane.id).collect())
            .unwrap_or_default();
        for pane_id in panes {
            self.materialize_pane(pane_id);
        }
    }

    fn materialize_pane(&mut self, pane_id: PaneId) {
        if self.runtime.pane_session.contains_key(&pane_id)
            || self.runtime.starting.contains(&pane_id)
        {
            return;
        }
        let Some((project_id, _, pane)) = self.workspace.locate_pane(pane_id) else {
            return;
        };
        let project_available = self
            .workspace
            .project(project_id)
            .is_some_and(|project| project.root.is_dir());
        if !project_available || !pane.cwd.is_dir() {
            return;
        }
        self.start_pane(pane_id);
    }

    fn start_pane(&mut self, pane_id: PaneId) {
        let Some(pane) = self.workspace.pane(pane_id) else {
            return;
        };
        let cwd = pane.cwd.clone();
        let size = pane.size;
        self.runtime.starting.insert(pane_id);
        self.runtime.spawn_failed.remove(&pane_id);
        let _ = self.effects.send(Effect::Spawn { pane_id, cwd, size });
    }

    fn pane_status(&self, pane_id: PaneId) -> Option<PaneStatus> {
        let (project_id, _, pane) = self.workspace.locate_pane(pane_id)?;
        let project_available = self
            .workspace
            .project(project_id)
            .is_some_and(|project| project.root.is_dir());
        if self.runtime.pane_session.contains_key(&pane_id) {
            return Some(PaneStatus::Live);
        }
        if self.runtime.starting.contains(&pane_id) {
            return Some(PaneStatus::Starting);
        }
        if !project_available {
            return Some(PaneStatus::Unavailable {
                reason: PaneStatusReason::ProjectRootMissing,
            });
        }
        if !pane.cwd.is_dir() {
            return Some(PaneStatus::Unavailable {
                reason: PaneStatusReason::CwdMissing,
            });
        }
        Some(PaneStatus::Dormant {
            reason: if self.runtime.spawn_failed.contains(&pane_id) {
                PaneStatusReason::SpawnFailed
            } else {
                PaneStatusReason::NotStarted
            },
        })
    }

    fn panes_to_unbind(&self, op: &WorkspaceOp) -> Vec<PaneId> {
        match op {
            WorkspaceOp::ClosePane { pane_id } => vec![*pane_id],
            WorkspaceOp::CloseTab { tab_id } => self.workspace.tab_panes(*tab_id),
            WorkspaceOp::StopProject { project_id } | WorkspaceOp::ForgetProject { project_id } => {
                project_id
                    .or(self.workspace.active_project)
                    .map(|project_id| self.workspace.project_panes(project_id))
                    .unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    fn unbind_panes(&mut self, panes: &[PaneId]) {
        for pane_id in panes {
            if let Some(session_id) = self.runtime.pane_session.remove(pane_id) {
                self.runtime.session_pane.remove(&session_id);
                if let Ok(mut mirror) = self.mirror.write() {
                    mirror.remove(&session_id);
                }
            }
            self.runtime.starting.remove(pane_id);
            self.runtime.spawn_failed.remove(pane_id);
        }
    }

    fn bind_session(&mut self, pane_id: PaneId, session_id: SessionId) {
        if let Some(previous) = self
            .runtime
            .pane_session
            .insert(pane_id, session_id.clone())
        {
            self.runtime.session_pane.remove(&previous);
            if let Ok(mut mirror) = self.mirror.write() {
                mirror.remove(&previous);
            }
        }
        self.runtime
            .session_pane
            .insert(session_id.clone(), pane_id);
        self.runtime.starting.remove(&pane_id);
        self.runtime.spawn_failed.remove(&pane_id);
        if let Some((project_id, tab_id, _)) = self.workspace.locate_pane(pane_id)
            && let Ok(mut mirror) = self.mirror.write()
        {
            mirror.insert(
                session_id,
                PaneRef {
                    project_id,
                    tab_id,
                    pane_id,
                },
            );
        }
        self.publish();
    }

    /// shell 自然退出：关闭对应 pane 并折叠父分屏，最后一个 pane 关闭后删除 tab。
    ///
    /// 显式关闭会先解绑会话，因此迟到的退出事件查不到映射，直接忽略；
    /// daemon 自身停止时也不改树。
    async fn on_session_exited(&mut self, session_id: &SessionId) {
        if self.shutting_down {
            return;
        }
        let Some(pane_id) = self.runtime.session_pane.get(session_id).copied() else {
            return;
        };
        self.run_internal(WorkspaceOp::ClosePane { pane_id }).await;
    }

    fn on_pane_cwd_changed(&mut self, session_id: &SessionId, cwd: PathBuf) {
        let Some(pane_id) = self.runtime.session_pane.get(session_id).copied() else {
            return;
        };
        let cwd = crate::workspace::normalize_pane_cwd(&cwd);
        let changed = self
            .workspace
            .pane(pane_id)
            .is_some_and(|pane| pane.cwd != cwd);
        if !changed {
            return;
        }
        if let Some(pane) = self.workspace.pane_mut(pane_id) {
            pane.cwd = cwd;
        }
        self.workspace.revision = self.workspace.revision.saturating_add(1);
        self.queue_metadata();
        self.publish();
    }

    fn on_resize(&mut self, session_id: &SessionId, size: TermSize) {
        let Some(pane_id) = self.runtime.session_pane.get(session_id).copied() else {
            return;
        };
        let Some(pane) = self.workspace.pane_mut(pane_id) else {
            return;
        };
        if pane.size == size {
            return;
        }
        pane.size = size;
        self.workspace.revision = self.workspace.revision.saturating_add(1);
        self.queue_metadata();
        self.publish();
        let _ = self.effects.send(Effect::ResizeSession {
            session_id: session_id.clone(),
            size,
        });
    }

    fn on_persistence(&mut self, result: PersistenceResult) {
        self.runtime.storage_error = result.result.err();
        self.publish();
    }

    fn queue_metadata(&mut self) {
        let Ok(bytes) = serde_json::to_vec_pretty(&self.workspace) else {
            log::error!("failed to serialize workspace metadata");
            return;
        };
        self.pending_metadata = Some((self.workspace.revision, bytes));
        self.metadata_deadline = Some(Instant::now() + Duration::from_millis(300));
    }

    fn flush_metadata(&mut self) {
        self.metadata_deadline = None;
        let Some((revision, bytes)) = self.pending_metadata.take() else {
            return;
        };
        self.writer.submit(SaveRequest {
            revision,
            bytes,
            ack: None,
        });
    }

    async fn persist_structural(&mut self) -> Result<(), StorageErrorInfo> {
        self.flush_metadata();
        let bytes =
            serde_json::to_vec_pretty(&self.workspace).map_err(|error| StorageErrorInfo {
                kind: StorageErrorKind::Format,
                message: error.to_string(),
                path: None,
            })?;
        let (ack_tx, ack_rx) = oneshot::channel();
        self.writer.submit(SaveRequest {
            revision: self.workspace.revision,
            bytes,
            ack: Some(ack_tx),
        });
        match tokio::time::timeout(Duration::from_secs(10), ack_rx).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(error))) => Err(error),
            Ok(Err(_)) => Err(StorageErrorInfo {
                kind: StorageErrorKind::Io,
                message: "workspace writer stopped".to_string(),
                path: None,
            }),
            Err(_) => Err(StorageErrorInfo {
                kind: StorageErrorKind::Io,
                message: "workspace write timed out".to_string(),
                path: None,
            }),
        }
    }

    fn publish(&mut self) {
        let snapshot = self.workspace.snapshot(&self.runtime);
        self.publish_with(snapshot);
    }

    fn publish_with(&mut self, snapshot: WorkspaceSnapshot) {
        self.last_published = snapshot.clone();
        self.snapshots.send_replace(Arc::new(snapshot));
    }

    async fn shutdown(&mut self) {
        self.shutting_down = true;
        self.flush_metadata();
        let flush = self.writer.flush();
        let _ = tokio::time::timeout(Duration::from_secs(5), flush).await;
        let sessions: Vec<SessionId> = self.runtime.pane_session.values().cloned().collect();
        for session_id in sessions {
            let _ = self.effects.send(Effect::CloseSession { session_id });
        }
        // shutdown 不改树。
    }
}

fn is_structural(op: &WorkspaceOp) -> bool {
    matches!(
        op,
        WorkspaceOp::OpenProject { .. }
            | WorkspaceOp::StopProject { .. }
            | WorkspaceOp::ForgetProject { .. }
            | WorkspaceOp::CreateTab { .. }
            | WorkspaceOp::CloseTab { .. }
            | WorkspaceOp::ActivateTab { .. }
            | WorkspaceOp::ReorderTabs { .. }
            | WorkspaceOp::SplitPane { .. }
            | WorkspaceOp::ClosePane { .. }
            | WorkspaceOp::ActivatePane { .. }
            | WorkspaceOp::RenameTab { .. }
            | WorkspaceOp::SetTabCommand { .. }
    )
}

fn runtime_fields_equal(a: &WorkspaceSnapshot, b: &WorkspaceSnapshot) -> bool {
    if a.storage_error != b.storage_error {
        return false;
    }
    a.projects.len() == b.projects.len()
        && a.projects.iter().zip(&b.projects).all(|(left, right)| {
            left.available == right.available && panes_equal(&left.tabs, &right.tabs)
        })
}

fn panes_equal(a: &[crate::workspace::TabSnapshot], b: &[crate::workspace::TabSnapshot]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(left, right)| node_equal(&left.layout, &right.layout))
}

fn node_equal(
    a: &crate::workspace::PaneNodeSnapshot,
    b: &crate::workspace::PaneNodeSnapshot,
) -> bool {
    use crate::workspace::PaneNodeSnapshot;
    match (a, b) {
        (PaneNodeSnapshot::Leaf(left), PaneNodeSnapshot::Leaf(right)) => {
            left.session == right.session
                && left.cwd_available == right.cwd_available
                && left.status == right.status
        }
        (
            PaneNodeSnapshot::Split {
                first: left_first,
                second: left_second,
                ..
            },
            PaneNodeSnapshot::Split {
                first: right_first,
                second: right_second,
                ..
            },
        ) => node_equal(left_first, right_first) && node_equal(left_second, right_second),
        _ => false,
    }
}

pub fn empty_mirror() -> OwnershipMirror {
    Arc::new(RwLock::new(BTreeMap::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::sync::oneshot;

    struct OkSaver;

    impl WorkspaceSaver for OkSaver {
        fn save_bytes(&self, _bytes: &[u8]) -> Result<(), StorageErrorInfo> {
            Ok(())
        }
    }

    struct ToggleSaver {
        fail: AtomicBool,
    }

    impl ToggleSaver {
        fn new(fail: bool) -> Self {
            Self {
                fail: AtomicBool::new(fail),
            }
        }

        fn set_fail(&self, fail: bool) {
            self.fail.store(fail, Ordering::SeqCst);
        }
    }

    impl WorkspaceSaver for ToggleSaver {
        fn save_bytes(&self, _bytes: &[u8]) -> Result<(), StorageErrorInfo> {
            if self.fail.load(Ordering::SeqCst) {
                Err(StorageErrorInfo {
                    kind: StorageErrorKind::Io,
                    message: "disk full".to_string(),
                    path: None,
                })
            } else {
                Ok(())
            }
        }
    }

    struct GatedSaver {
        started: mpsc::UnboundedSender<()>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl WorkspaceSaver for GatedSaver {
        fn save_bytes(&self, _bytes: &[u8]) -> Result<(), StorageErrorInfo> {
            let _ = self.started.send(());
            let _ = self.release.lock().unwrap().recv();
            Ok(())
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "crossh-actor-{name}-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn spawn(store: Arc<dyn WorkspaceSaver>) -> (WorkspaceHandle, mpsc::UnboundedReceiver<Effect>) {
        let (effects, effects_rx) = mpsc::unbounded_channel();
        (
            spawn_workspace_actor(Workspace::new(), store, effects, empty_mirror()),
            effects_rx,
        )
    }

    async fn mutate(
        handle: &WorkspaceHandle,
        op: WorkspaceOp,
        expected_revision: Option<u64>,
    ) -> WorkspaceMutateReply {
        let (reply, reply_rx) = oneshot::channel();
        handle
            .commands
            .send(WorkspaceCommand::Mutate {
                request: WorkspaceMutateRequest {
                    request_id: 1,
                    expected_revision,
                    op,
                },
                reply,
            })
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), reply_rx)
            .await
            .expect("mutate reply timed out")
            .expect("mutate reply dropped")
    }

    fn project(snapshot: &WorkspaceSnapshot, id: ProjectId) -> &crate::workspace::ProjectSnapshot {
        snapshot
            .projects
            .iter()
            .find(|project| project.id == id)
            .expect("project in snapshot")
    }

    fn pane(snapshot: &WorkspaceSnapshot, id: PaneId) -> &crate::workspace::PaneSnapshot {
        snapshot
            .projects
            .iter()
            .flat_map(|project| &project.tabs)
            .flat_map(|tab| tab.layout.leaves())
            .find(|pane| pane.id == id)
            .expect("pane in snapshot")
    }

    async fn wait_for(
        handle: &mut WorkspaceHandle,
        what: &str,
        pred: impl Fn(&WorkspaceSnapshot) -> bool,
    ) -> Arc<WorkspaceSnapshot> {
        for _ in 0..200 {
            let current = handle.snapshots.borrow().clone();
            if pred(&current) {
                return current;
            }
            if tokio::time::timeout(Duration::from_millis(50), handle.snapshots.changed())
                .await
                .is_err()
            {
                continue;
            }
        }
        panic!("timed out waiting for {what}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn structural_reply_waits_for_durable_save() {
        let root = temp_dir("durable");
        let (started_tx, mut started_rx) = mpsc::unbounded_channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let store = Arc::new(GatedSaver {
            started: started_tx,
            release: Mutex::new(release_rx),
        });
        let (handle, _effects) = spawn(store);

        let (reply_tx, mut reply_rx) = oneshot::channel();
        handle
            .commands
            .send(WorkspaceCommand::Mutate {
                request: WorkspaceMutateRequest {
                    request_id: 1,
                    expected_revision: None,
                    op: WorkspaceOp::OpenProject { root: root.clone() },
                },
                reply: reply_tx,
            })
            .unwrap();

        tokio::time::timeout(Duration::from_secs(5), started_rx.recv())
            .await
            .expect("writer must start saving")
            .expect("saver alive");
        assert!(
            reply_rx.try_recv().is_err(),
            "structural reply must wait for the durable save"
        );

        release_tx.send(()).unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(5), reply_rx)
            .await
            .expect("reply after release")
            .expect("reply dropped");
        assert_eq!(reply.outcome, MutateOutcome::Applied);
        assert!(reply.durable);
        assert_eq!(reply.revision, 1);
        assert!(reply.storage_error.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn stale_expected_revision_is_a_conflict() {
        let first_root = temp_dir("cas-first");
        let second_root = temp_dir("cas-second");
        let (handle, _effects) = spawn(Arc::new(OkSaver));

        let first = mutate(
            &handle,
            WorkspaceOp::OpenProject {
                root: first_root.clone(),
            },
            None,
        )
        .await;
        assert_eq!(first.outcome, MutateOutcome::Applied);
        assert_eq!(first.revision, 1);

        let stale = mutate(
            &handle,
            WorkspaceOp::OpenProject {
                root: second_root.clone(),
            },
            Some(0),
        )
        .await;
        assert_eq!(stale.outcome, MutateOutcome::Conflict);
        assert_eq!(stale.revision, 1);
        assert_eq!(stale.error.as_ref().unwrap().code, "conflict");

        let fresh = mutate(
            &handle,
            WorkspaceOp::OpenProject {
                root: second_root.clone(),
            },
            Some(1),
        )
        .await;
        assert_eq!(fresh.outcome, MutateOutcome::Applied);
        assert_eq!(fresh.revision, 2);

        let _ = std::fs::remove_dir_all(first_root);
        let _ = std::fs::remove_dir_all(second_root);
    }

    #[tokio::test]
    async fn failed_structural_write_reports_storage_error_and_recovers() {
        let root = temp_dir("storage");
        let store = Arc::new(ToggleSaver::new(true));
        let (mut handle, _effects) = spawn(store.clone());

        let reply = mutate(
            &handle,
            WorkspaceOp::OpenProject { root: root.clone() },
            None,
        )
        .await;
        assert_eq!(reply.outcome, MutateOutcome::Applied);
        assert!(!reply.durable);
        assert_eq!(reply.revision, 1);
        assert_eq!(
            reply.storage_error.as_ref().unwrap().kind,
            StorageErrorKind::Io
        );

        let failed = wait_for(&mut handle, "storage error snapshot", |snapshot| {
            snapshot.storage_error.is_some()
        })
        .await;
        assert_eq!(failed.storage_error.as_ref().unwrap().message, "disk full");
        assert_eq!(failed.revision, 1);

        store.set_fail(false);
        let recovered = wait_for(&mut handle, "storage error cleared", |snapshot| {
            snapshot.storage_error.is_none()
        })
        .await;
        assert_eq!(recovered.revision, 1, "recovery must not advance revision");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn live_pane_survives_missing_project_root() {
        let root = temp_dir("missing-root");
        let (mut handle, _effects) = spawn(Arc::new(OkSaver));
        let created = mutate(
            &handle,
            WorkspaceOp::OpenProject { root: root.clone() },
            None,
        )
        .await;
        let project_id = created.created.project.expect("project id");
        let pane_id = created.created.pane.expect("pane id");
        handle
            .commands
            .send(WorkspaceCommand::BindSession {
                pane_id,
                session_id: "s1".to_string(),
            })
            .unwrap();
        wait_for(&mut handle, "pane live", |snapshot| {
            pane(snapshot, pane_id).status == PaneStatus::Live
        })
        .await;

        std::fs::remove_dir_all(&root).unwrap();
        let revision = handle.snapshots.borrow().revision;
        let refresh = mutate(&handle, WorkspaceOp::RefreshAvailability, None).await;
        assert_eq!(refresh.outcome, MutateOutcome::Applied);
        assert_eq!(refresh.revision, revision);

        let snapshot = handle.snapshots.borrow().clone();
        assert!(!project(&snapshot, project_id).available);
        let live = pane(&snapshot, pane_id);
        assert_eq!(live.status, PaneStatus::Live);
        assert_eq!(live.session.as_deref(), Some("s1"));

        handle
            .commands
            .send(WorkspaceCommand::SessionExited {
                session_id: "s1".to_string(),
            })
            .unwrap();
        let exited = wait_for(&mut handle, "tab removed after exit", |snapshot| {
            snapshot.projects[0].tabs.is_empty()
        })
        .await;
        assert_eq!(exited.revision, revision + 1);
        assert_eq!(exited.projects[0].active_tab, None);

        // 重复的退出事件必须被忽略：映射已解绑，布局不再变化。
        handle
            .commands
            .send(WorkspaceCommand::SessionExited {
                session_id: "s1".to_string(),
            })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(handle.snapshots.borrow().projects[0].tabs.is_empty());
        assert_eq!(handle.snapshots.borrow().revision, revision + 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_exit_closes_its_pane_and_folds_split() {
        let root = temp_dir("split-exit");
        let (mut handle, _effects) = spawn(Arc::new(OkSaver));
        let created = mutate(
            &handle,
            WorkspaceOp::OpenProject { root: root.clone() },
            None,
        )
        .await;
        let tab_id = created.created.tab.expect("tab id");
        let left = created.created.pane.expect("left pane id");
        let right = mutate(
            &handle,
            WorkspaceOp::SplitPane {
                pane_id: left,
                axis: crate::workspace::Axis::Row,
                cwd: None,
            },
            None,
        )
        .await
        .created
        .pane
        .expect("right pane id");
        for (pane_id, session_id) in [(left, "s1"), (right, "s2")] {
            handle
                .commands
                .send(WorkspaceCommand::BindSession {
                    pane_id,
                    session_id: session_id.to_string(),
                })
                .unwrap();
        }
        wait_for(&mut handle, "both panes live", |snapshot| {
            pane(snapshot, left).status == PaneStatus::Live
                && pane(snapshot, right).status == PaneStatus::Live
        })
        .await;
        let revision = handle.snapshots.borrow().revision;

        handle
            .commands
            .send(WorkspaceCommand::SessionExited {
                session_id: "s2".to_string(),
            })
            .unwrap();
        let after = wait_for(&mut handle, "right pane removed", |snapshot| {
            snapshot.projects[0]
                .tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .is_some_and(|tab| tab.layout.leaves().count() == 1)
        })
        .await;

        assert_eq!(after.revision, revision + 1);
        let tab = after.projects[0]
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .expect("tab survives sibling exit");
        assert_eq!(tab.active_pane, Some(left));
        assert!(matches!(
            tab.layout,
            crate::workspace::PaneNodeSnapshot::Leaf(ref pane) if pane.id == left
        ));
        assert_eq!(pane(&after, left).session.as_deref(), Some("s1"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn late_session_exit_after_close_tab_keeps_layout() {
        let root = temp_dir("late-exit");
        let (mut handle, mut effects) = spawn(Arc::new(OkSaver));
        let created = mutate(
            &handle,
            WorkspaceOp::OpenProject { root: root.clone() },
            None,
        )
        .await;
        let tab_id = created.created.tab.expect("tab id");
        let pane_id = created.created.pane.expect("pane id");
        handle
            .commands
            .send(WorkspaceCommand::BindSession {
                pane_id,
                session_id: "s1".to_string(),
            })
            .unwrap();
        wait_for(&mut handle, "pane bound", |snapshot| {
            pane(snapshot, pane_id).session.as_deref() == Some("s1")
        })
        .await;

        handle
            .commands
            .send(WorkspaceCommand::CloseTab { tab_id })
            .unwrap();
        let closed = wait_for(&mut handle, "tab closed", |snapshot| {
            snapshot.projects[0].tabs.is_empty()
        })
        .await;
        let revision = closed.revision;
        assert!(matches!(
            effects.try_recv(),
            Ok(Effect::CloseSession { session_id }) if session_id == "s1"
        ));

        for _ in 0..2 {
            handle
                .commands
                .send(WorkspaceCommand::SessionExited {
                    session_id: "s1".to_string(),
                })
                .unwrap();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;

        let after = handle.snapshots.borrow().clone();
        assert!(after.projects[0].tabs.is_empty());
        assert_eq!(after.revision, revision);
        assert!(effects.try_recv().is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn ensure_session_respects_attach_and_spawn_failure() {
        let root = temp_dir("ensure");
        let (mut handle, mut effects) = spawn(Arc::new(OkSaver));
        let created = mutate(
            &handle,
            WorkspaceOp::OpenProject { root: root.clone() },
            None,
        )
        .await;
        let pane_id = created.created.pane.expect("pane id");
        let revision = created.revision;
        let initial = handle.snapshots.borrow().clone();
        assert!(matches!(
            pane(&initial, pane_id).status,
            PaneStatus::Dormant {
                reason: PaneStatusReason::NotStarted
            }
        ));

        let detached = mutate(&handle, WorkspaceOp::EnsurePaneSession { pane_id }, None).await;
        assert_eq!(detached.outcome, MutateOutcome::Unchanged);
        assert_eq!(detached.revision, revision);

        let (reply, reply_rx) = oneshot::channel();
        handle
            .commands
            .send(WorkspaceCommand::Attach { reply })
            .unwrap();
        reply_rx.await.unwrap();
        wait_for(&mut handle, "pane starting after attach", |snapshot| {
            pane(snapshot, pane_id).status == PaneStatus::Starting
        })
        .await;
        assert!(matches!(
            effects.try_recv(),
            Ok(Effect::Spawn { pane_id: spawned, .. }) if spawned == pane_id
        ));

        handle
            .commands
            .send(WorkspaceCommand::SessionSpawnFailed { pane_id })
            .unwrap();
        let failed = wait_for(
            &mut handle,
            "pane dormant after spawn failure",
            |snapshot| {
                matches!(
                    pane(snapshot, pane_id).status,
                    PaneStatus::Dormant {
                        reason: PaneStatusReason::SpawnFailed
                    }
                )
            },
        )
        .await;
        assert_eq!(failed.revision, revision);

        let retry = mutate(&handle, WorkspaceOp::EnsurePaneSession { pane_id }, None).await;
        assert_eq!(retry.outcome, MutateOutcome::Applied);
        assert_eq!(retry.revision, revision);
        let _ = std::fs::remove_dir_all(root);
    }
}

//! daemon 服务循环：握手 → 会话表 → 帧路由。
//!
//! - 连接先 `Hello` 握手（版本 + 令牌），失败固定文案拒绝并断开，不泄露会话。
//! - 每个连接至多绑定一个会话；`Create` 成功后自动绑定（创建者即首个接入）。
//! - 输入权：已是 controller 的输入直写 PTY；observer 的第一次输入被丢弃但
//!   完成接管（"发送一次输入即取得，从此的按键进入"）。
//! - 接入同步"先订阅、再快照、序号过滤"：订阅与快照之间无 `.await`，
//!   读线程的"先推进环、再广播"顺序保证不丢；序号过滤保证不重。
//! - 退出：子进程退出 → 标记 + 广播 `Exited` → 从表移除；绑定者后续输入收到
//!   `session-closed`。

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{
    TcpListener, TcpStream,
    tcp::{OwnedReadHalf, OwnedWriteHalf},
};
use tokio::sync::{Mutex, broadcast, mpsc, oneshot, watch};

use crate::feed::program_state_name;
use crate::handshake::{
    ErrorPayload, HandshakeRequest, PROTOCOL_VERSION, capabilities, verify_handshake,
};
use crate::protocol::{
    AttachRequest, AttachedReply, CloseRequest, CreateRequest, CreatedReply, ExitedNotice,
    ListReply, MAX_FRAME, MessageKind, ReplayEndMarker, ResizeRequest, SessionInfo,
    WorkspaceMutateRequest, decode_frame, encode_frame, encode_output,
};
use crate::session::{ControllerId, Ownership, OwnershipEvent, SessionMeta};
use crate::spawn::{ChildSession, OutputChunk, SpawnOptions};
use crate::workspace::{SessionId, TermSize, Workspace, WorkspaceSnapshot};
use crate::workspace_actor::{
    Effect, OwnershipMirror, PaneRef, WorkspaceCommand, spawn_workspace_actor,
};
use crate::workspace_store::WorkspaceSaver;

/// 服务端声明的能力（交集后回给客户端）。
fn server_capabilities() -> Vec<String> {
    [
        capabilities::WORKSPACE_V1,
        capabilities::REPLAY_MARK,
        capabilities::OBSERVER_LOCAL_LAYOUT,
        capabilities::READONLY_BADGE,
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// 回放/重同步分块（32 KiB 一块，与读线程缓冲同量级）。
const SNAPSHOT_CHUNK: usize = 32 * 1024;

/// 会话表条目。
struct SessionEntry {
    child: ChildSession,
    ownership: Ownership,
    meta: SessionMeta,
    /// 退出广播（退出码）。接入者同时订阅它与输出流。
    exit_tx: broadcast::Sender<i32>,
}

type Handle = Arc<Mutex<SessionEntry>>;
type Sessions = Arc<Mutex<HashMap<String, Handle>>>;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_CLIENT: AtomicU64 = AtomicU64::new(1);

fn new_session_id() -> String {
    format!("s{}", NEXT_ID.fetch_add(1, Ordering::Relaxed))
}

/// 总字节流切块并标注每块末端序号（回放发送用）。
fn chunk_with_seq(start_seq: u64, bytes: &[u8]) -> Vec<OutputChunk> {
    let mut out = Vec::new();
    let mut seq = start_seq;
    for piece in bytes.chunks(SNAPSHOT_CHUNK) {
        seq += piece.len() as u64;
        out.push((seq, piece.to_vec()));
    }
    out
}

fn info_of(entry: &SessionEntry) -> SessionInfo {
    SessionInfo {
        id: entry.meta.id.clone(),
        cwd: entry.meta.cwd.clone(),
        program: entry.meta.program.clone(),
        controller: entry.ownership.controller(),
        alive: entry.meta.alive,
        exit_code: entry.meta.exit_code,
        project_id: None,
        tab_id: None,
        pane_id: None,
    }
}

/// 归属镜像合并：在释放会话锁之后调用。
fn merge_ownership(info: &mut SessionInfo, mirror: &OwnershipMirror) {
    let pane = mirror
        .read()
        .ok()
        .and_then(|mirror| mirror.get(&info.id).copied());
    if let Some(pane) = pane {
        info.project_id = Some(pane.project_id.0);
        info.tab_id = Some(pane.tab_id.0);
        info.pane_id = Some(pane.pane_id.0);
    }
}

#[derive(Clone)]
struct WorkspaceRuntime {
    commands: mpsc::UnboundedSender<WorkspaceCommand>,
    snapshots: watch::Receiver<Arc<WorkspaceSnapshot>>,
    mirror: OwnershipMirror,
}

/// 从 feed 刷新元数据的 cwd/程序状态（`ls`/应答前调用）。
fn refresh_meta(entry: &mut SessionEntry) {
    let Ok(feed) = entry.child.feed.lock() else {
        return;
    };
    entry.meta.cwd = feed.cwd().map(str::to_string);
    entry.meta.program = feed
        .program()
        .map(|p| program_state_name(p.state).to_string());
}

async fn read_frame(r: &mut OwnedReadHalf) -> io::Result<(MessageKind, Vec<u8>)> {
    let mut head = [0u8; 5];
    r.read_exact(&mut head).await?;
    let len = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
    if !(1..=MAX_FRAME).contains(&len) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad frame length",
        ));
    }
    let mut rest = vec![0u8; len - 1];
    r.read_exact(&mut rest).await?;
    let mut full = Vec::with_capacity(4 + len);
    full.extend_from_slice(&head[..4]);
    full.push(head[4]);
    full.extend_from_slice(&rest);
    decode_frame(&full)
}

async fn send_frame(
    w: &Arc<Mutex<OwnedWriteHalf>>,
    kind: MessageKind,
    payload: &[u8],
) -> io::Result<()> {
    let frame = encode_frame(kind, payload)?;
    let mut w = w.lock().await;
    w.write_all(&frame).await
}

fn err_payload(code: &'static str, message: impl Into<String>) -> Vec<u8> {
    serde_json::to_vec(&ErrorPayload {
        code: code.to_string(),
        message: message.into(),
    })
    .unwrap_or_default()
}

pub async fn run(
    listener: TcpListener,
    token: String,
    workspace: Workspace,
    store: Arc<dyn WorkspaceSaver>,
) -> ! {
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
    let mirror = crate::workspace_actor::empty_mirror();
    let (effects_tx, effects_rx) = mpsc::unbounded_channel::<Effect>();
    let handle = spawn_workspace_actor(workspace, store, effects_tx, mirror.clone());
    spawn_effect_runtime(sessions.clone(), handle.commands.clone(), effects_rx);
    spawn_cwd_poller(sessions.clone(), handle.commands.clone());
    let runtime = WorkspaceRuntime {
        commands: handle.commands,
        snapshots: handle.snapshots,
        mirror,
    };
    loop {
        let Ok((sock, _)) = listener.accept().await else {
            continue;
        };
        let _ = sock.set_nodelay(true);
        let sessions = sessions.clone();
        let token = token.clone();
        let runtime = runtime.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_conn(sock, sessions, token, runtime).await {
                log::debug!("conn ended: {e}");
            }
        });
    }
}

/// 运行时副作用：actor 不碰 PTY，spawn/close/resize 都在这里执行。
fn spawn_effect_runtime(
    sessions: Sessions,
    commands: mpsc::UnboundedSender<WorkspaceCommand>,
    mut effects: mpsc::UnboundedReceiver<Effect>,
) {
    tokio::spawn(async move {
        while let Some(effect) = effects.recv().await {
            match effect {
                Effect::Spawn { pane_id, cwd, size } => {
                    let req = CreateRequest {
                        shell: None,
                        cwd: Some(cwd.to_string_lossy().into_owned()),
                        cols: size.cols,
                        rows: size.rows,
                        command: None,
                        env: None,
                    };
                    match spawn_session(&req).await {
                        Ok(handle) => {
                            let id = handle.lock().await.meta.id.clone();
                            sessions.lock().await.insert(id.clone(), handle.clone());
                            spawn_exit_monitor(
                                sessions.clone(),
                                id.clone(),
                                handle,
                                commands.clone(),
                            );
                            let _ = commands.send(WorkspaceCommand::BindSession {
                                pane_id,
                                session_id: id,
                            });
                        }
                        Err(error) => {
                            log::warn!("workspace pane {pane_id} spawn failed: {error}");
                            let _ = commands.send(WorkspaceCommand::SessionSpawnFailed { pane_id });
                        }
                    }
                }
                Effect::CloseSession { session_id } => {
                    if let Some(handle) = sessions.lock().await.remove(&session_id) {
                        log::info!("workspace session {session_id} closed");
                        handle.lock().await.child.kill();
                    }
                }
                Effect::ResizeSession { session_id, size } => {
                    if let Some(handle) = sessions.lock().await.get(&session_id).cloned() {
                        let mut entry = handle.lock().await;
                        if entry.meta.alive {
                            let _ = entry.child.resize(size.cols.max(1), size.rows.max(1));
                        }
                    }
                }
            }
        }
    });
}

/// 轮询 PTY 的 OSC 7 cwd，变化时通知 actor（会话锁内不调用 actor）。
fn spawn_cwd_poller(sessions: Sessions, commands: mpsc::UnboundedSender<WorkspaceCommand>) {
    tokio::spawn(async move {
        let mut seen: HashMap<SessionId, String> = HashMap::new();
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let mut updates = Vec::new();
            {
                let map = sessions.lock().await;
                for (id, handle) in map.iter() {
                    let Ok(entry) = handle.try_lock() else {
                        continue;
                    };
                    let Ok(feed) = entry.child.feed.lock() else {
                        continue;
                    };
                    let Some(cwd) = feed.cwd().map(str::to_string) else {
                        continue;
                    };
                    if seen.get(id) != Some(&cwd) {
                        seen.insert(id.clone(), cwd.clone());
                        updates.push((id.clone(), cwd));
                    }
                }
            }
            for (session_id, cwd) in updates {
                let _ = commands.send(WorkspaceCommand::PaneCwdChanged {
                    session_id,
                    cwd: std::path::PathBuf::from(cwd),
                });
            }
        }
    });
}

async fn handle_conn(
    sock: TcpStream,
    sessions: Sessions,
    token: String,
    runtime: WorkspaceRuntime,
) -> io::Result<()> {
    let client_id: ControllerId = NEXT_CLIENT.fetch_add(1, Ordering::Relaxed);
    let (mut reader, writer) = sock.into_split();
    let writer = Arc::new(Mutex::new(writer));

    // 握手必须是第一帧。
    let (kind, payload) = read_frame(&mut reader).await?;
    if kind != MessageKind::Hello {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "expected hello"));
    }
    let req: HandshakeRequest = serde_json::from_slice(&payload)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad hello"))?;
    match verify_handshake(
        &req,
        PROTOCOL_VERSION,
        &token,
        &server_capabilities(),
        client_id,
    ) {
        Ok(ok) => {
            let body = serde_json::to_vec(&ok).unwrap_or_default();
            send_frame(&writer, MessageKind::Welcome, &body).await?;
        }
        Err(e) => {
            let body = err_payload("handshake", e.to_string());
            send_frame(&writer, MessageKind::Error, &body).await?;
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "handshake failed",
            ));
        }
    }

    let mut bound: Option<Handle> = None;
    // 当前接入的转发任务（切换绑定/断开时 abort）。
    let mut fwd: Option<tokio::task::JoinHandle<()>> = None;
    let mut workspace_attached = false;
    let mut workspace_forward: Option<tokio::task::JoinHandle<()>> = None;

    loop {
        let (kind, payload) = match read_frame(&mut reader).await {
            Ok(f) => f,
            Err(_) => {
                detach_conn(&mut bound, &mut fwd, client_id).await;
                if workspace_attached {
                    let _ = runtime.commands.send(WorkspaceCommand::AttachEnded);
                }
                if let Some(task) = workspace_forward.take() {
                    task.abort();
                }
                return Ok(());
            }
        };
        match kind {
            MessageKind::List => {
                let mut out = {
                    let map = sessions.lock().await;
                    let mut out = Vec::with_capacity(map.len());
                    for h in map.values() {
                        let mut e = h.lock().await;
                        refresh_meta(&mut e);
                        out.push(info_of(&e));
                    }
                    out
                };
                for info in &mut out {
                    merge_ownership(info, &runtime.mirror);
                }
                let body = serde_json::to_vec(&ListReply { sessions: out }).unwrap_or_default();
                send_frame(&writer, MessageKind::Listed, &body).await?;
            }
            MessageKind::Create => {
                let req: CreateRequest = serde_json::from_slice(&payload)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad create"))?;
                let handle = spawn_session(&req).await?;
                let id = handle.lock().await.meta.id.clone();
                sessions.lock().await.insert(id.clone(), handle.clone());
                log::info!("session {id} created ({}x{})", req.cols, req.rows);
                spawn_exit_monitor(
                    sessions.clone(),
                    id.clone(),
                    handle.clone(),
                    runtime.commands.clone(),
                );
                // 创建者自动绑定（首个接入 = controller）。
                detach_conn(&mut bound, &mut fwd, client_id).await;
                let created = {
                    let e = handle.lock().await;
                    CreatedReply {
                        session: info_of(&e),
                    }
                };
                let body = serde_json::to_vec(&created).unwrap_or_default();
                send_frame(&writer, MessageKind::Created, &body).await?;
                bound = Some(handle.clone());
                fwd = Some(attach_and_stream(&writer, handle, client_id, &runtime.mirror).await?);
            }
            MessageKind::Attach => {
                let req: AttachRequest = serde_json::from_slice(&payload)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad attach"))?;
                let handle = sessions.lock().await.get(&req.id).cloned();
                let Some(handle) = handle else {
                    send_frame(
                        &writer,
                        MessageKind::Error,
                        &err_payload("session-gone", &req.id),
                    )
                    .await?;
                    continue;
                };
                if !handle.lock().await.meta.alive {
                    send_frame(
                        &writer,
                        MessageKind::Error,
                        &err_payload("session-gone", &req.id),
                    )
                    .await?;
                    continue;
                }
                detach_conn(&mut bound, &mut fwd, client_id).await;
                bound = Some(handle.clone());
                log::info!("client {client_id} attached to {}", req.id);
                fwd = Some(attach_and_stream(&writer, handle, client_id, &runtime.mirror).await?);
            }
            MessageKind::Input => {
                let Some(h) = &bound else {
                    send_frame(
                        &writer,
                        MessageKind::Error,
                        &err_payload("no-session", "attach first"),
                    )
                    .await?;
                    continue;
                };
                let mut e = h.lock().await;
                if !e.meta.alive {
                    let id = e.meta.id.clone();
                    send_frame(
                        &writer,
                        MessageKind::Error,
                        &err_payload("session-closed", id),
                    )
                    .await?;
                    continue;
                }
                match e.ownership.on_input(client_id) {
                    // 已是 controller：直写 PTY。
                    None => {
                        log::debug!(
                            "input {}B to {}: {:?}",
                            payload.len(),
                            e.meta.id,
                            &payload[..payload.len().min(32)],
                        );
                        if let Err(err) = e.child.write_all(&payload) {
                            log::debug!("write to pty failed: {err}");
                        }
                    }
                    // observer 的第一次输入：完成接管，本次丢弃，从此进入。
                    Some(OwnershipEvent::Takeover { new_controller }) => {
                        e.meta.controller = Some(new_controller);
                        log::info!(
                            "session {} controller -> {new_controller} (takeover)",
                            e.meta.id
                        );
                    }
                    Some(OwnershipEvent::Released)
                    | Some(OwnershipEvent::ObserverInputDropped { .. }) => {}
                }
            }
            MessageKind::Resize => {
                let req: ResizeRequest = serde_json::from_slice(&payload)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad resize"))?;
                if let Some(h) = &bound {
                    let session_id = h.lock().await.meta.id.clone();
                    let workspace_pane = runtime
                        .mirror
                        .read()
                        .ok()
                        .is_some_and(|mirror| mirror.contains_key(&session_id));
                    if workspace_pane {
                        let (session_id, accepted) = {
                            let e = h.lock().await;
                            (
                                e.meta.id.clone(),
                                e.ownership.accepts_resize(client_id) && e.meta.alive,
                            )
                        };
                        if accepted {
                            let _ = runtime.commands.send(WorkspaceCommand::ResizePane {
                                session_id,
                                size: TermSize {
                                    cols: req.cols.max(1),
                                    rows: req.rows.max(1),
                                },
                            });
                        }
                    } else {
                        // 临时会话不经过工作区 actor，直接调整 PTY。
                        let mut e = h.lock().await;
                        if e.ownership.accepts_resize(client_id) && e.meta.alive {
                            let _ = e.child.resize(req.cols.max(1), req.rows.max(1));
                        }
                    }
                    // observer 上报静默忽略（尺寸权威在 controller）。
                }
            }
            MessageKind::Close => {
                let req: CloseRequest = serde_json::from_slice(&payload)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad close"))?;
                let owner = runtime
                    .mirror
                    .read()
                    .ok()
                    .and_then(|mirror| mirror.get(&req.id).copied());
                if let Some(PaneRef { pane_id, .. }) = owner {
                    // 布局会话：actor 先解绑、更新布局、持久化，再投 CloseSession。
                    let _ = runtime
                        .commands
                        .send(WorkspaceCommand::ClosePane { pane_id });
                } else if let Some(h) = sessions.lock().await.remove(&req.id) {
                    // 临时会话：沿用直接 kill 路径。
                    log::info!("session {} closed by client", req.id);
                    h.lock().await.child.kill();
                }
            }
            MessageKind::Shutdown => {
                // 显式停止：flush 工作区、投递关闭效果、删端口文件后退出。
                log::info!("shutdown requested, exiting");
                let (reply, wait) = oneshot::channel();
                let _ = runtime.commands.send(WorkspaceCommand::Shutdown { reply });
                let _ = tokio::time::timeout(std::time::Duration::from_secs(6), wait).await;
                crate::instance::remove_port_file();
                std::process::exit(0);
            }
            MessageKind::WorkspaceGet => {
                let (reply, wait) = oneshot::channel();
                let _ = runtime.commands.send(WorkspaceCommand::Get { reply });
                match wait.await {
                    Ok(snapshot) => {
                        let body = serde_json::to_vec(&*snapshot).unwrap_or_default();
                        send_frame(&writer, MessageKind::WorkspaceSnapshot, &body).await?;
                    }
                    Err(_) => {
                        send_frame(
                            &writer,
                            MessageKind::Error,
                            &err_payload("workspace", "workspace unavailable"),
                        )
                        .await?;
                    }
                }
            }
            MessageKind::WorkspaceAttach => {
                // 先订阅再 attach：订阅与快照之间不会漏掉发布。
                let mut updates = runtime.snapshots.clone();
                let (reply, wait) = oneshot::channel();
                let _ = runtime.commands.send(WorkspaceCommand::Attach { reply });
                if let Ok(snapshot) = wait.await {
                    let body = serde_json::to_vec(&*snapshot).unwrap_or_default();
                    send_frame(&writer, MessageKind::WorkspaceSnapshot, &body).await?;
                }
                workspace_attached = true;
                let forward_writer = writer.clone();
                workspace_forward = Some(tokio::spawn(async move {
                    while updates.changed().await.is_ok() {
                        let snapshot = updates.borrow_and_update().clone();
                        let body = serde_json::to_vec(&*snapshot).unwrap_or_default();
                        if send_frame(&forward_writer, MessageKind::WorkspaceSnapshot, &body)
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }));
            }
            MessageKind::WorkspaceMutate => {
                let request: WorkspaceMutateRequest = serde_json::from_slice(&payload)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad mutate"))?;
                let (reply, wait) = oneshot::channel();
                let _ = runtime
                    .commands
                    .send(WorkspaceCommand::Mutate { request, reply });
                match wait.await {
                    Ok(reply) => {
                        let body = serde_json::to_vec(&reply).unwrap_or_default();
                        send_frame(&writer, MessageKind::WorkspaceMutateReply, &body).await?;
                    }
                    Err(_) => {
                        send_frame(
                            &writer,
                            MessageKind::Error,
                            &err_payload("workspace", "workspace unavailable"),
                        )
                        .await?;
                    }
                }
            }
            _ => {
                send_frame(
                    &writer,
                    MessageKind::Error,
                    &err_payload("bad-request", "unexpected kind"),
                )
                .await?;
            }
        }
    }
}

async fn detach_conn(
    bound: &mut Option<Handle>,
    fwd: &mut Option<tokio::task::JoinHandle<()>>,
    client: ControllerId,
) {
    if let Some(t) = fwd.take() {
        t.abort();
    }
    if let Some(h) = bound.take() {
        let mut e = h.lock().await;
        if e.ownership.on_detach(client) == Some(OwnershipEvent::Released) {
            e.meta.controller = None;
        }
    }
}

/// 接入并起流：所有权登记 → 订阅 → 快照 → `Attached` → 回放 → `ReplayEnd` →
/// 起转发任务（只跟进 `seq > end_seq` 的块）。
async fn attach_and_stream(
    writer: &Arc<Mutex<OwnedWriteHalf>>,
    handle: Handle,
    client: ControllerId,
    mirror: &OwnershipMirror,
) -> io::Result<tokio::task::JoinHandle<()>> {
    // 订阅与快照之间无 `.await`：读线程"先推进环、再广播"的顺序
    // 保证订阅后到达的块序号一定大于快照序号，不丢不重。
    let (out_rx, exit_rx, attached, replay) = {
        let mut e = handle.lock().await;
        if e.ownership.on_attach(client)
            == Some(OwnershipEvent::Takeover {
                new_controller: client,
            })
        {
            e.meta.controller = Some(client);
        }
        let out_rx = e.child.output_tx.subscribe();
        let exit_rx = e.exit_tx.subscribe();
        refresh_meta(&mut e);
        let (end_seq, bytes) = e
            .child
            .feed
            .lock()
            .map(|f| f.snapshot())
            .unwrap_or_default();
        let attached = AttachedReply {
            session: info_of(&e),
            end_seq,
        };
        let start = end_seq.saturating_sub(bytes.len() as u64);
        (out_rx, exit_rx, attached, chunk_with_seq(start, &bytes))
    };
    let mut attached = attached;
    merge_ownership(&mut attached.session, mirror);
    let id = attached.session.id.clone();
    let body = serde_json::to_vec(&attached).unwrap_or_default();
    send_frame(writer, MessageKind::Attached, &body).await?;
    for (seq, chunk) in &replay {
        send_frame(writer, MessageKind::Output, &encode_output(*seq, chunk)).await?;
    }
    let end = serde_json::to_vec(&ReplayEndMarker {
        end_seq: attached.end_seq,
    })
    .unwrap_or_default();
    send_frame(writer, MessageKind::ReplayEnd, &end).await?;
    Ok(spawn_forwarder(
        writer.clone(),
        out_rx,
        exit_rx,
        id,
        attached.end_seq,
    ))
}

/// 转发任务：实时块（`seq > end_seq` 才发）与退出通知。
fn spawn_forwarder(
    writer: Arc<Mutex<OwnedWriteHalf>>,
    mut rx: broadcast::Receiver<OutputChunk>,
    mut exit_rx: broadcast::Receiver<i32>,
    id: String,
    end_seq: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut last = end_seq;
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Ok((seq, chunk)) => {
                        if seq <= last {
                            continue;
                        }
                        last = seq;
                        if send_frame(&writer, MessageKind::Output, &encode_output(seq, &chunk))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    // 消费慢丢了块：环上限内的数据已在回放里，
                    // 运行中 lag 的丢失区间直接跳过（序号天然对齐）。
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                code = exit_rx.recv() => {
                    let code = code.unwrap_or(-1);
                    let body = serde_json::to_vec(&ExitedNotice { id: id.clone(), code })
                        .unwrap_or_default();
                    let _ = send_frame(&writer, MessageKind::Exited, &body).await;
                    break;
                }
            }
        }
    })
}

/// 退出监视：子进程退出 → 标记 + 广播 `Exited` → 从表移除。
fn spawn_exit_monitor(
    sessions: Sessions,
    id: String,
    handle: Handle,
    commands: mpsc::UnboundedSender<WorkspaceCommand>,
) {
    tokio::spawn(async move {
        loop {
            let exited = {
                let mut e = handle.lock().await;
                match e.child.try_wait() {
                    Ok(Some(code)) => {
                        e.meta.alive = false;
                        e.meta.exit_code = Some(code);
                        refresh_meta(&mut e);
                        let _ = e.exit_tx.send(code);
                        Some(code)
                    }
                    Ok(None) => None,
                    Err(err) => {
                        log::debug!("try_wait failed for {id}: {err}");
                        Some(-1)
                    }
                }
            };
            if let Some(code) = exited {
                sessions.lock().await.remove(&id);
                log::info!("session {id} exited with {code}");
                let _ = commands.send(WorkspaceCommand::SessionExited {
                    session_id: id.clone(),
                });
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    });
}

async fn spawn_session(req: &CreateRequest) -> io::Result<Handle> {
    let cwd = req.cwd.clone().map(std::path::PathBuf::from);
    let cols = req.cols.max(1);
    let rows = req.rows.max(1);
    // 基线环境：调用方快照优先（`run --` 在哪调起就用哪的环境）；
    // 老客户端不带 env 时回落 daemon 自身（0.44.x 行为）。
    let mut base: HashMap<String, String> = match &req.env {
        Some(env) => env.iter().cloned().collect(),
        None => std::env::vars_os()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
            .collect(),
    };
    // PWD 跟随请求的 cwd（调用方 PWD 是旧的）；OLDPWD 丢弃。
    if let Some(cwd) = &cwd {
        base.insert("PWD".to_string(), cwd.to_string_lossy().into_owned());
        base.remove("OLDPWD");
    }
    // `run --` 一次性命令走裸启动（无集成脚本）；否则按 shell 装配集成。
    let raw_cmd = req.command.clone().filter(|c| !c.is_empty());
    let shell_prog = req
        .shell
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| base.get("SHELL").cloned().filter(|s| !s.is_empty()))
        .unwrap_or_else(crossh_core::terminal::get_system_shell);
    let id = new_session_id();
    let spawn_id = id.clone();
    let opt = tokio::task::spawn_blocking(move || match raw_cmd {
        Some(argv) => SpawnOptions::raw(&argv, cwd, cols, rows, base),
        None => SpawnOptions::interactive_shell(&shell_prog, cwd, cols, rows, base),
    })
    .await
    .map_err(|e| io::Error::other(e.to_string()))??;
    let child = tokio::task::spawn_blocking(move || ChildSession::spawn(&spawn_id, opt))
        .await
        .map_err(|e| io::Error::other(e.to_string()))??;
    let meta = SessionMeta {
        id: id.clone(),
        cwd: req.cwd.clone(),
        title: None,
        program: None,
        controller: None,
        alive: true,
        exit_code: None,
    };
    let (exit_tx, _) = broadcast::channel(1);
    Ok(Arc::new(Mutex::new(SessionEntry {
        child,
        ownership: Ownership::new(),
        meta,
        exit_tx,
    })))
}

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
use std::time::Instant;

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
use crate::instance::{DaemonAddr, remove_port_file_if_matches};
use crate::protocol::{
    AttachRequest, AttachedReply, CloseRequest, CreateRequest, CreatedReply, ExitedNotice,
    ListReply, MAX_FRAME, MessageKind, ReplayEndMarker, ResizeRequest, SessionInfo,
    WorkspaceMutateRequest, decode_frame, encode_frame, encode_output,
};
use crate::session::{
    ControllerId, Ownership, OwnershipEvent, REDRAW_NUDGE_SETTLE, RingSegment, SessionMeta,
    run_redraw_nudge,
};
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
    // 新客户端的未来帧旧 daemon 跳过（与客户端侧对称容错）。
    loop {
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
        let (kind, payload) = decode_frame(&full)?;
        if kind == MessageKind::Unknown {
            continue;
        }
        return Ok((kind, payload));
    }
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
    // 自己的认领地址：退出时只删“仍是自己”的端口文件，
    // 避免 6s flush 窗口内新 daemon 已认领时误删新主人的文件。
    let own_addr = DaemonAddr {
        port: listener.local_addr().expect("daemon listener addr").port(),
        token: token.clone(),
    };
    loop {
        let Ok((sock, _)) = listener.accept().await else {
            continue;
        };
        let _ = sock.set_nodelay(true);
        let sessions = sessions.clone();
        let token = token.clone();
        let runtime = runtime.clone();
        let own_addr = own_addr.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_conn(sock, sessions, token, runtime, own_addr).await {
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
                            let size = TermSize {
                                cols: size.cols.max(1),
                                rows: size.rows.max(1),
                            };
                            let _ = entry.child.resize(size.cols, size.rows);
                            // 真实尺寸封段记环（nudge 临时尺寸从不走这里），恢复读最新值用。
                            if let Ok(mut feed) = entry.child.feed.lock() {
                                feed.resize(size);
                            }
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

/// 连接级可变状态：当前绑定会话、输出转发任务与工作区订阅。
struct ConnState {
    client_id: ControllerId,
    bound: Option<Handle>,
    /// 当前接入的转发任务（切换绑定/断开时 abort）。
    fwd: Option<tokio::task::JoinHandle<()>>,
    workspace_attached: bool,
    workspace_forward: Option<tokio::task::JoinHandle<()>>,
}

impl ConnState {
    /// 读帧失败（对端断开）后的清理：解绑 → 结束工作区接入 → 停快照转发。
    async fn disconnect(&mut self, commands: &mpsc::UnboundedSender<WorkspaceCommand>) {
        detach_conn(&mut self.bound, &mut self.fwd, self.client_id).await;
        if self.workspace_attached {
            let _ = commands.send(WorkspaceCommand::AttachEnded);
        }
        if let Some(task) = self.workspace_forward.take() {
            task.abort();
        }
    }
}

async fn handle_conn(
    sock: TcpStream,
    sessions: Sessions,
    token: String,
    runtime: WorkspaceRuntime,
    own_addr: DaemonAddr,
) -> io::Result<()> {
    let client_id: ControllerId = NEXT_CLIENT.fetch_add(1, Ordering::Relaxed);
    let (mut reader, writer) = sock.into_split();
    let writer = Arc::new(Mutex::new(writer));

    // 握手必须是第一帧。
    handshake(&mut reader, &writer, &token, client_id).await?;

    let mut state = ConnState {
        client_id,
        bound: None,
        fwd: None,
        workspace_attached: false,
        workspace_forward: None,
    };

    loop {
        let (kind, payload) = match read_frame(&mut reader).await {
            Ok(f) => f,
            Err(_) => {
                state.disconnect(&runtime.commands).await;
                return Ok(());
            }
        };
        match kind {
            MessageKind::List => handle_list(&writer, &sessions, &runtime).await?,
            MessageKind::Create => {
                handle_create(&mut state, &writer, &sessions, &runtime, &payload).await?;
            }
            MessageKind::Attach => {
                handle_attach(&mut state, &writer, &sessions, &runtime, &payload).await?;
            }
            MessageKind::Input => handle_input(&state, &writer, &payload).await?,
            MessageKind::Resize => handle_resize(&state, &runtime, &payload).await?,
            MessageKind::Close => handle_close(&sessions, &runtime, &payload).await?,
            MessageKind::Shutdown => handle_shutdown(&runtime, &own_addr).await,
            MessageKind::WorkspaceGet => handle_workspace_get(&writer, &runtime).await?,
            MessageKind::WorkspaceAttach => {
                handle_workspace_attach(&mut state, &writer, &runtime).await?;
            }
            MessageKind::WorkspaceMutate => {
                handle_workspace_mutate(&writer, &runtime, &payload).await?;
            }
            _ => handle_unknown(&writer).await?,
        }
    }
}

/// 握手：校验首帧版本与令牌；失败回 `Error` 后拒绝连接。
async fn handshake(
    reader: &mut OwnedReadHalf,
    writer: &Arc<Mutex<OwnedWriteHalf>>,
    token: &str,
    client_id: ControllerId,
) -> io::Result<()> {
    let (kind, payload) = read_frame(reader).await?;
    if kind != MessageKind::Hello {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "expected hello"));
    }
    let req: HandshakeRequest = serde_json::from_slice(&payload)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad hello"))?;
    match verify_handshake(
        &req,
        PROTOCOL_VERSION,
        token,
        &server_capabilities(),
        client_id,
    ) {
        Ok(ok) => {
            let body = serde_json::to_vec(&ok).unwrap_or_default();
            send_frame(writer, MessageKind::Welcome, &body).await?;
        }
        Err(e) => {
            let body = err_payload("handshake", e.to_string());
            send_frame(writer, MessageKind::Error, &body).await?;
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "handshake failed",
            ));
        }
    }
    Ok(())
}

async fn handle_list(
    writer: &Arc<Mutex<OwnedWriteHalf>>,
    sessions: &Sessions,
    runtime: &WorkspaceRuntime,
) -> io::Result<()> {
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
    send_frame(writer, MessageKind::Listed, &body).await
}

async fn handle_create(
    state: &mut ConnState,
    writer: &Arc<Mutex<OwnedWriteHalf>>,
    sessions: &Sessions,
    runtime: &WorkspaceRuntime,
    payload: &[u8],
) -> io::Result<()> {
    let req: CreateRequest = serde_json::from_slice(payload)
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
    detach_conn(&mut state.bound, &mut state.fwd, state.client_id).await;
    let created = {
        let e = handle.lock().await;
        CreatedReply {
            session: info_of(&e),
        }
    };
    let body = serde_json::to_vec(&created).unwrap_or_default();
    send_frame(writer, MessageKind::Created, &body).await?;
    state.bound = Some(handle.clone());
    state.fwd = Some(attach_and_stream(writer, handle, state.client_id, &runtime.mirror).await?);
    Ok(())
}

async fn handle_attach(
    state: &mut ConnState,
    writer: &Arc<Mutex<OwnedWriteHalf>>,
    sessions: &Sessions,
    runtime: &WorkspaceRuntime,
    payload: &[u8],
) -> io::Result<()> {
    let req: AttachRequest = serde_json::from_slice(payload)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad attach"))?;
    let handle = sessions.lock().await.get(&req.id).cloned();
    let Some(handle) = handle else {
        send_frame(
            writer,
            MessageKind::Error,
            &err_payload("session-gone", &req.id),
        )
        .await?;
        return Ok(());
    };
    if !handle.lock().await.meta.alive {
        send_frame(
            writer,
            MessageKind::Error,
            &err_payload("session-gone", &req.id),
        )
        .await?;
        return Ok(());
    }
    detach_conn(&mut state.bound, &mut state.fwd, state.client_id).await;
    state.bound = Some(handle.clone());
    log::info!("client {} attached to {}", state.client_id, req.id);
    state.fwd = Some(attach_and_stream(writer, handle, state.client_id, &runtime.mirror).await?);
    Ok(())
}

async fn handle_input(
    state: &ConnState,
    writer: &Arc<Mutex<OwnedWriteHalf>>,
    payload: &[u8],
) -> io::Result<()> {
    let Some(h) = &state.bound else {
        send_frame(
            writer,
            MessageKind::Error,
            &err_payload("no-session", "attach first"),
        )
        .await?;
        return Ok(());
    };
    let mut e = h.lock().await;
    if !e.meta.alive {
        let id = e.meta.id.clone();
        send_frame(
            writer,
            MessageKind::Error,
            &err_payload("session-closed", id),
        )
        .await?;
        return Ok(());
    }
    match e.ownership.on_input(state.client_id) {
        // 已是 controller：直写 PTY。
        None => {
            log::debug!(
                "input {}B to {}: {:?}",
                payload.len(),
                e.meta.id,
                &payload[..payload.len().min(32)],
            );
            if let Err(err) = e.child.write_all(payload) {
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
        Some(OwnershipEvent::Released) | Some(OwnershipEvent::ObserverInputDropped { .. }) => {}
    }
    Ok(())
}

async fn handle_resize(
    state: &ConnState,
    runtime: &WorkspaceRuntime,
    payload: &[u8],
) -> io::Result<()> {
    let req: ResizeRequest = serde_json::from_slice(payload)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad resize"))?;
    if let Some(h) = &state.bound {
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
                    e.ownership.accepts_resize(state.client_id) && e.meta.alive,
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
            if e.ownership.accepts_resize(state.client_id) && e.meta.alive {
                let size = TermSize {
                    cols: req.cols.max(1),
                    rows: req.rows.max(1),
                };
                let _ = e.child.resize(size.cols, size.rows);
                // 真实尺寸封段记环（nudge 临时尺寸从不走这里），恢复读最新值用。
                if let Ok(mut feed) = e.child.feed.lock() {
                    feed.resize(size);
                }
            }
        }
        // observer 上报静默忽略（尺寸权威在 controller）。
    }
    Ok(())
}

async fn handle_close(
    sessions: &Sessions,
    runtime: &WorkspaceRuntime,
    payload: &[u8],
) -> io::Result<()> {
    let req: CloseRequest = serde_json::from_slice(payload)
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
    Ok(())
}

async fn handle_shutdown(runtime: &WorkspaceRuntime, own: &DaemonAddr) {
    // 显式停止：flush 工作区、投递关闭效果；只删仍属于自己的端口文件
    // （flush 窗口内新 daemon 若已认领，文件已是新主人的，直接退出）。
    log::info!("shutdown requested, exiting");
    let (reply, wait) = oneshot::channel();
    let _ = runtime.commands.send(WorkspaceCommand::Shutdown { reply });
    let _ = tokio::time::timeout(std::time::Duration::from_secs(6), wait).await;
    remove_port_file_if_matches(own);
    std::process::exit(0);
}

async fn handle_workspace_get(
    writer: &Arc<Mutex<OwnedWriteHalf>>,
    runtime: &WorkspaceRuntime,
) -> io::Result<()> {
    let (reply, wait) = oneshot::channel();
    let _ = runtime.commands.send(WorkspaceCommand::Get { reply });
    match wait.await {
        Ok(snapshot) => {
            let body = serde_json::to_vec(&*snapshot).unwrap_or_default();
            send_frame(writer, MessageKind::WorkspaceSnapshot, &body).await
        }
        Err(_) => {
            send_frame(
                writer,
                MessageKind::Error,
                &err_payload("workspace", "workspace unavailable"),
            )
            .await
        }
    }
}

async fn handle_workspace_attach(
    state: &mut ConnState,
    writer: &Arc<Mutex<OwnedWriteHalf>>,
    runtime: &WorkspaceRuntime,
) -> io::Result<()> {
    // 先订阅再 attach：订阅与快照之间不会漏掉发布。
    let mut updates = runtime.snapshots.clone();
    let (reply, wait) = oneshot::channel();
    let _ = runtime.commands.send(WorkspaceCommand::Attach { reply });
    if let Ok(snapshot) = wait.await {
        let body = serde_json::to_vec(&*snapshot).unwrap_or_default();
        send_frame(writer, MessageKind::WorkspaceSnapshot, &body).await?;
    }
    state.workspace_attached = true;
    let forward_writer = writer.clone();
    state.workspace_forward = Some(tokio::spawn(async move {
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
    Ok(())
}

async fn handle_workspace_mutate(
    writer: &Arc<Mutex<OwnedWriteHalf>>,
    runtime: &WorkspaceRuntime,
    payload: &[u8],
) -> io::Result<()> {
    let request: WorkspaceMutateRequest = serde_json::from_slice(payload)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad mutate"))?;
    let (reply, wait) = oneshot::channel();
    let _ = runtime
        .commands
        .send(WorkspaceCommand::Mutate { request, reply });
    match wait.await {
        Ok(reply) => {
            let body = serde_json::to_vec(&reply).unwrap_or_default();
            send_frame(writer, MessageKind::WorkspaceMutateReply, &body).await
        }
        Err(_) => {
            send_frame(
                writer,
                MessageKind::Error,
                &err_payload("workspace", "workspace unavailable"),
            )
            .await
        }
    }
}

async fn handle_unknown(writer: &Arc<Mutex<OwnedWriteHalf>>) -> io::Result<()> {
    send_frame(
        writer,
        MessageKind::Error,
        &err_payload("bad-request", "unexpected kind"),
    )
    .await
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

/// 接入命中即起 nudge 线程：临时尺寸立刻下 PTY，暂留后读环尾最新尺寸恢复。
///
/// - 只动 PTY：不写环分段、不发客户端 `Size`（恢复读环尾，真实 `resize` 已封段）。
/// - 会话已死/判定不命中一律不打扰；`settled` 拿不到锁或已死则放弃恢复。
/// - 恢复跑独立线程，`attach` 回放不被暂留挡住；连续接入各起一线程，
///   都读最新值恢复，以后到者为准自然收敛。
async fn maybe_nudge_redraw(handle: &Handle, client: ControllerId) {
    // Windows 无 `tcgetpgrp`：恒按无前台处理，只回放不 nudge。
    #[cfg(windows)]
    {
        let _ = (handle, client);
        return;
    }
    #[cfg(not(windows))]
    {
        let ready = {
            let e = handle.lock().await;
            let foreground = e.child.foreground_program();
            let is_observer = e.ownership.controller() != Some(client);
            let alive = e.meta.alive;
            let transient = e.child.feed.lock().ok().and_then(|mut f| {
                f.poll_redraw_nudge(alive, foreground, is_observer, Instant::now())
            });
            transient.map(|t| (t, e.child.nudge_resizer(), Arc::clone(&e.child.feed)))
        };
        let Some((transient, resizer, feed)) = ready else {
            return;
        };
        let entry = handle.clone();
        let settled = move || {
            let size = feed.lock().ok().map(|f| f.current_size())?;
            // 真实 `resize` 短暂持会话锁时拿不到，按存活处理（尺寸以环为准）；
            // 拿得到则看 `alive`，死了就放弃恢复。
            let alive = entry.try_lock().map(|e| e.meta.alive).unwrap_or(true);
            alive.then_some(size)
        };
        if let Err(e) = run_redraw_nudge(transient, REDRAW_NUDGE_SETTLE, resizer, settled) {
            log::debug!("nudge spawn failed: {e}");
        }
    }
}

/// 分段回放：`Prelude`（复用 `Output` 进当前模式）→ 每段 `Size + Output*`。
/// `Size` 只做几何标记不占 `seq`；`Output` 的 `seq` 从 `start` 连续递增。
/// 返回末端 `seq`（应等于 `Attached.end_seq`，调用方保证）。
async fn send_replay(
    writer: &Arc<Mutex<OwnedWriteHalf>>,
    start: u64,
    prelude: Option<&[u8]>,
    segments: &[RingSegment],
) -> io::Result<u64> {
    let mut cur = start;
    if let Some(pre) = prelude.filter(|p| !p.is_empty()) {
        send_frame(writer, MessageKind::Output, &encode_output(cur, pre)).await?;
    }
    for seg in segments {
        let body = serde_json::to_vec(&seg.size).unwrap_or_default();
        send_frame(writer, MessageKind::Size, &body).await?;
        for (seq, chunk) in chunk_with_seq(cur, &seg.bytes) {
            cur = seq;
            send_frame(writer, MessageKind::Output, &encode_output(seq, &chunk)).await?;
        }
    }
    Ok(cur)
}

/// 接入并起流：所有权登记 → 订阅 → 快照 → `Attached` → 回放 → `ReplayEnd` →
/// 起转发任务（只跟进 `seq > end_seq` 的块）。
/// 回放按段发 `Size + Output*`，先发 `Prelude`（复用 `Output` 进当前模式）。
async fn attach_and_stream(
    writer: &Arc<Mutex<OwnedWriteHalf>>,
    handle: Handle,
    client: ControllerId,
    mirror: &OwnershipMirror,
) -> io::Result<tokio::task::JoinHandle<()>> {
    // 订阅与快照之间无 `.await`：读线程"先推进环、再广播"的顺序
    // 保证订阅后到达的块序号一定大于快照序号，不丢不重。
    let (out_rx, exit_rx, attached, start, prelude, segments) = {
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
        let (end_seq, prelude, segs) = e
            .child
            .feed
            .lock()
            .map(|mut f| {
                // 旧查询不补发：脱离期已由读线程代答（无订阅直写 PTY），
                // 此处取走防堆积；回放字节里的查询由新客户端正常应答。
                let _ = f.take_queries();
                (f.seq(), f.replay_prelude(), f.snapshot_segments().1)
            })
            .unwrap_or_default();
        let attached = AttachedReply {
            session: info_of(&e),
            end_seq,
        };
        let total: usize = segs.iter().map(|s| s.bytes.len()).sum();
        let start = end_seq.saturating_sub(total as u64);
        (out_rx, exit_rx, attached, start, prelude, segs)
    };
    let mut attached = attached;
    merge_ownership(&mut attached.session, mirror);
    let id = attached.session.id.clone();
    log::debug!(
        "attach {id}: alive={} exit={:?} end_seq={} replay={}B segs={}",
        attached.session.alive,
        attached.session.exit_code,
        attached.end_seq,
        segments.iter().map(|s| s.bytes.len()).sum::<usize>(),
        segments.len(),
    );
    let body = serde_json::to_vec(&attached).unwrap_or_default();
    send_frame(writer, MessageKind::Attached, &body).await?;
    send_replay(writer, start, prelude.as_deref(), &segments).await?;
    let end = serde_json::to_vec(&ReplayEndMarker {
        end_seq: attached.end_seq,
    })
    .unwrap_or_default();
    send_frame(writer, MessageKind::ReplayEnd, &end).await?;
    // 首画已丢的全屏前台程序：请它重画（只动 PTY，独立线程不挡回放）。
    maybe_nudge_redraw(&handle, client).await;
    // 退出发在订阅之前或回放期间时，broadcast 不会补发；按会话状态补一条。
    let exit_code = {
        let e = handle.lock().await;
        (!e.meta.alive).then(|| e.meta.exit_code.unwrap_or(-1))
    };
    if let Some(code) = exit_code {
        let body = serde_json::to_vec(&ExitedNotice { id, code }).unwrap_or_default();
        send_frame(writer, MessageKind::Exited, &body).await?;
        return Ok(tokio::spawn(async {}));
    }
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
                    log::debug!("forwarder {id}: exit {code}");
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
        log::debug!("exit monitor {id}: start");
        let mut polls: u64 = 0;
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
                    Ok(None) => {
                        polls += 1;
                        if polls.is_multiple_of(50) {
                            log::debug!("exit monitor {id}: still alive after {polls} polls");
                        }
                        None
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::FeedState;
    use crate::protocol::decode_output;
    use std::time::Duration;

    /// 首画已丢的 feed：全量在备用屏，环内已无切换（与 `feed.rs` 同构）。
    fn lost_feed() -> FeedState {
        let mut feed = FeedState::new();
        feed.push(b"\x1b[?1049h");
        feed.push(&vec![b'.'; crate::session::REPLAY_LIMIT_BYTES - 8]);
        feed.push(b"!");
        assert!(feed.alt_screen_now());
        assert!(!feed.alt_screen_in_ring());
        feed
    }

    /// 分段回放后跟 `ReplayEnd`，`Size` 不占 `seq`，冷却内只 nudge 一次。
    #[tokio::test]
    async fn segmented_replay_ends_with_replay_end_and_nudges_once() {
        let mut feed = FeedState::new();
        feed.push(b"a");
        feed.resize(TermSize {
            cols: 100,
            rows: 30,
        });
        feed.push(b"b");
        let (end_seq, segs) = feed.snapshot_segments();
        assert_eq!((end_seq, segs.len()), (2, 2));
        assert_eq!(feed.replay_prelude(), None, "主屏无前导");
        // 走真实 `send_replay` 发帧，对端按序收。
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accept = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let (mut r, _) = sock.into_split();
            // 按序收：`Size, Output(a), Size, Output(b), ReplayEnd`。
            let mut sizes = Vec::new();
            let mut bytes = Vec::new();
            let mut seqs = Vec::new();
            loop {
                let (kind, payload) = read_frame(&mut r).await.unwrap();
                match kind {
                    MessageKind::Size => {
                        let s: TermSize = serde_json::from_slice(&payload).unwrap();
                        sizes.push(s);
                    }
                    MessageKind::Output => {
                        let (seq, chunk) = decode_output(&payload).unwrap();
                        seqs.push(seq);
                        bytes.extend_from_slice(&chunk);
                    }
                    MessageKind::ReplayEnd => {
                        let mark: ReplayEndMarker = serde_json::from_slice(&payload).unwrap();
                        return (sizes, bytes, seqs, mark.end_seq);
                    }
                    _ => panic!("回放段只含 Size/Output/ReplayEnd"),
                }
            }
        });
        let sock = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let (_, w) = sock.into_split();
        let writer = Arc::new(Mutex::new(w));
        let total: usize = segs.iter().map(|s| s.bytes.len()).sum();
        let start = end_seq.saturating_sub(total as u64);
        let last = send_replay(&writer, start, None, &segs).await.unwrap();
        assert_eq!(last, end_seq, "`Size` 不占 `seq`，末端即总流偏移");
        let end = serde_json::to_vec(&ReplayEndMarker { end_seq }).unwrap_or_default();
        send_frame(&writer, MessageKind::ReplayEnd, &end)
            .await
            .unwrap();
        drop(writer);
        let (sizes, bytes, seqs, mark_seq) = accept.await.unwrap();
        assert_eq!(
            sizes,
            vec![
                TermSize::default(),
                TermSize {
                    cols: 100,
                    rows: 30
                }
            ]
        );
        assert_eq!(bytes, b"ab");
        assert_eq!(mark_seq, end_seq, "回放后跟 `ReplayEnd`");
        assert_eq!(seqs, vec![1, 2], "`seq` 只随字节递增");
        // 冷却内只 nudge 一次：首次命中，500ms 内不再请。
        let mut nudge = lost_feed();
        let now = Instant::now();
        assert!(nudge.poll_redraw_nudge(true, true, false, now).is_some());
        assert_eq!(
            nudge.poll_redraw_nudge(true, true, false, now + Duration::from_millis(500)),
            None
        );
    }

    #[test]
    fn attach_drops_stale_queries_but_keeps_bytes_for_new_client() {
        // 脱离期已代答的旧查询：attach 取走防堆积，回放字节仍带原文供新客户端应答。
        let mut feed = FeedState::new();
        feed.push(b"\x1b[c");
        // 模拟 `attach_and_stream` 快照时的取走逻辑（与实现同序）。
        let _ = feed.take_queries();
        assert!(feed.take_queries().is_empty());
        let (_, bytes) = feed.snapshot();
        assert!(bytes.windows(3).any(|w| w == b"\x1b[c"));
        // 全屏残留 `133;B` + 前台有程序：不采信在提示符，保留重建的屏。
        let mut full = FeedState::new();
        full.push(b"\x1b]133;B\x07\x1b[?1049h");
        assert!(full.stored_at_prompt());
        assert!(!full.replayed_at_prompt(true));
        assert!(full.replayed_at_prompt(false));
    }
}

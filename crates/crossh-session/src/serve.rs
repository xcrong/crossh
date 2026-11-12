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
use tokio::sync::{Mutex, broadcast};

use crate::feed::program_state_name;
use crate::handshake::{
    ErrorPayload, HandshakeRequest, PROTOCOL_VERSION, capabilities, verify_handshake,
};
use crate::protocol::{
    AttachRequest, AttachedReply, CloseRequest, CreateRequest, CreatedReply, ExitedNotice,
    ListReply, MAX_FRAME, MessageKind, ReplayEndMarker, ResizeRequest, SessionInfo, decode_frame,
    encode_frame, encode_output,
};
use crate::session::{ControllerId, Ownership, OwnershipEvent, SessionMeta};
use crate::spawn::{ChildSession, OutputChunk, SpawnOptions};

/// 服务端声明的能力（交集后回给客户端）。
fn server_capabilities() -> Vec<String> {
    [
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
    }
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

pub async fn run(listener: TcpListener, token: String) -> ! {
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
    loop {
        let Ok((sock, _)) = listener.accept().await else {
            continue;
        };
        let _ = sock.set_nodelay(true);
        let sessions = sessions.clone();
        let token = token.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_conn(sock, sessions, token).await {
                log::debug!("conn ended: {e}");
            }
        });
    }
}

async fn handle_conn(sock: TcpStream, sessions: Sessions, token: String) -> io::Result<()> {
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

    loop {
        let (kind, payload) = match read_frame(&mut reader).await {
            Ok(f) => f,
            Err(_) => {
                detach_conn(&mut bound, &mut fwd, client_id).await;
                return Ok(());
            }
        };
        match kind {
            MessageKind::List => {
                let map = sessions.lock().await;
                let mut out = Vec::with_capacity(map.len());
                for h in map.values() {
                    let mut e = h.lock().await;
                    refresh_meta(&mut e);
                    out.push(info_of(&e));
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
                spawn_exit_monitor(sessions.clone(), id.clone(), handle.clone());
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
                fwd = Some(attach_and_stream(&writer, handle, client_id).await?);
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
                fwd = Some(attach_and_stream(&writer, handle, client_id).await?);
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
                    let mut e = h.lock().await;
                    if e.ownership.accepts_resize(client_id) && e.meta.alive {
                        let _ = e.child.resize(req.cols.max(1), req.rows.max(1));
                    }
                    // observer 上报静默忽略（尺寸权威在 controller）。
                }
            }
            MessageKind::Close => {
                let req: CloseRequest = serde_json::from_slice(&payload)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad close"))?;
                if let Some(h) = sessions.lock().await.remove(&req.id) {
                    // 发信号；退出码由监视任务收割并广播（监视任务随后发现
                    // 表中已无该会话，仅跳过移除）。
                    log::info!("session {} closed by client", req.id);
                    h.lock().await.child.kill();
                }
            }
            MessageKind::Shutdown => {
                // 显式停止：删端口文件后直接退出进程（会话随之结束，见规格）。
                log::info!("shutdown requested, exiting");
                crate::instance::remove_port_file();
                std::process::exit(0);
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
fn spawn_exit_monitor(sessions: Sessions, id: String, handle: Handle) {
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
    // `run --` 一次性命令走裸启动（无集成脚本）；否则按 shell 装配集成。
    let raw_cmd = req.command.clone().filter(|c| !c.is_empty());
    let shell_prog = req
        .shell
        .clone()
        .unwrap_or_else(crossh_core::terminal::get_system_shell);
    let id = new_session_id();
    let spawn_id = id.clone();
    let opt = tokio::task::spawn_blocking(move || match raw_cmd {
        Some(argv) => SpawnOptions::raw(&argv, cwd, cols, rows),
        None => SpawnOptions::interactive_shell(&shell_prog, cwd, cols, rows),
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

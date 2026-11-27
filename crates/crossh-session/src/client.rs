//! 第二前端的客户端：连接、握手、会话操作的高层封装。
//!
//! - GUI 与 CLI 共用同一套接入流程（发现 → 建/接会话 → 输入输出 → 结束）。
//! - `ensure_daemon` 按需拉起：探活失败则 detached  spawn（unix `setsid`，
//!   Windows `DETACHED_PROCESS`），环境照常继承（D6）。

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex, oneshot, watch};

use crate::handshake::{
    ErrorPayload, HandshakeOk, HandshakeRequest, PROTOCOL_VERSION, capabilities,
};
use crate::instance::{
    DaemonAddr, daemon_port_file_path, read_daemon_addr, remove_port_file_if_matches,
};
use crate::protocol::{
    AttachRequest, AttachedReply, CloseRequest, CreateRequest, CreatedReply, ExitedNotice,
    ListReply, MAX_FRAME, MessageKind, ReplayEndMarker, ResizeRequest, SessionInfo,
    WorkspaceMutateReply, WorkspaceMutateRequest, decode_frame, decode_output, encode_frame,
};
use crate::workspace::{WorkspaceOp, WorkspaceSnapshot};

/// 握手等待上限（daemon 本地回环，常时毫秒级）。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// 拉起后等端口文件出现的最长等待。
const SPAWN_WAIT: Duration = Duration::from_secs(10);

async fn read_frame(reader: &mut OwnedReadHalf) -> io::Result<(MessageKind, Vec<u8>)> {
    // 未知帧循环跳过：新 daemon 的 `Size=22` 等旧端不认得，老读路径不断连。
    loop {
        let mut head = [0u8; 5];
        reader.read_exact(&mut head).await?;
        let len = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
        if !(1..=MAX_FRAME).contains(&len) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bad frame length",
            ));
        }
        let mut rest = vec![0u8; len - 1];
        reader.read_exact(&mut rest).await?;
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
    writer: &mut OwnedWriteHalf,
    kind: MessageKind,
    payload: &[u8],
) -> io::Result<()> {
    writer.write_all(&encode_frame(kind, payload)?).await
}

fn json<T: serde::Serialize>(v: &T) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_default()
}

/// daemon 二进制路径：与当前可执行文件同目录的 `crossh-daemon`；
/// 找不到精确名时按前缀匹配（覆盖测试目标的哈希后缀名）。
pub fn daemon_bin_path() -> io::Result<PathBuf> {
    // 测试/排障钩子：显式指定 daemon 二进制。
    if let Some(p) = std::env::var_os("CROSSH_DAEMON_BIN") {
        return Ok(PathBuf::from(p));
    }
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no exe dir"))?;
    #[cfg(windows)]
    let exact = "crossh-daemon.exe";
    #[cfg(not(windows))]
    let exact = "crossh-daemon";
    let candidate = dir.join(exact);
    if candidate.is_file() {
        return Ok(candidate);
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("crossh-daemon"))
        })
        .collect();
    entries.sort();
    entries.into_iter().next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("crossh-daemon not found next to {}", exe.display()),
        )
    })
}

/// detached 启动 daemon：脱离当前进程组/控制终端（D6、任务 3.3），
/// 关闭窗口或启动它的终端退出都不会 SIGHUP 掉会话。
fn spawn_detached() -> io::Result<()> {
    let bin = daemon_bin_path()?;
    let mut cmd = std::process::Command::new(bin);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: fork 后子进程内只做 setsid，不触碰任何锁与内存分配。
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x00000008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    cmd.spawn()?;
    Ok(())
}

/// 客户端声明的能力（与 daemon 的交集才是生效能力）。
fn client_capabilities() -> Vec<String> {
    [
        capabilities::WORKSPACE_V1,
        capabilities::REPLAY_MARK,
        capabilities::OBSERVER_LOCAL_LAYOUT,
        capabilities::READONLY_BADGE,
        capabilities::REATTACH_NUDGE_V1,
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

#[derive(Debug)]
enum ProbeError {
    Version(String),
    /// TCP 连不上（拒绝/超时）：端口真死，文件可视为陈旧。
    Connect(io::Error),
    /// 已连上但握手失败（超时/令牌不对/欢迎帧损坏等）：对端还活着，
    /// MUST NOT 删文件、MUST NOT 拉新 daemon。
    Handshake(io::Error),
}

async fn probe_version(
    addr: &DaemonAddr,
    version: u32,
    capabilities: Vec<String>,
) -> Result<HandshakeOk, ProbeError> {
    let sock = tokio::time::timeout(
        Duration::from_millis(500),
        TcpStream::connect(("127.0.0.1", addr.port)),
    )
    .await
    .map_err(|_| {
        ProbeError::Connect(io::Error::new(
            io::ErrorKind::TimedOut,
            "daemon connect timeout",
        ))
    })?
    .map_err(ProbeError::Connect)?;
    let (mut r, mut w) = sock.into_split();
    let hello = HandshakeRequest {
        version,
        capabilities,
        token: addr.token.clone(),
    };
    send_frame(&mut w, MessageKind::Hello, &json(&hello))
        .await
        .map_err(ProbeError::Handshake)?;
    let (kind, payload) = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(&mut r))
        .await
        .map_err(|_| {
            ProbeError::Handshake(io::Error::new(
                io::ErrorKind::TimedOut,
                "daemon handshake timeout",
            ))
        })?
        .map_err(ProbeError::Handshake)?;
    if kind != MessageKind::Welcome {
        let msg = serde_json::from_slice::<ErrorPayload>(&payload)
            .map(|e| e.message)
            .unwrap_or_else(|_| "handshake failed".to_string());
        if msg.contains("protocol version mismatch") {
            return Err(ProbeError::Version(msg));
        }
        return Err(ProbeError::Handshake(io::Error::new(
            io::ErrorKind::PermissionDenied,
            msg,
        )));
    }
    serde_json::from_slice(&payload).map_err(|_| {
        ProbeError::Handshake(io::Error::new(io::ErrorKind::InvalidData, "bad welcome"))
    })
}

/// daemon 状态三态；`probe_daemon` 无副作用（不删文件、不 spawn）。
///
/// 只有“端口连不上”才算陈旧；连上但握手失败说明有活进程在听，
/// 归为 `Absent { stale: false }`（`ensure_daemon` 见到有文件残留会
/// 直接报错，不删不拉；`status` 沿用“not running”文案）。
/// 活着但握手失败时 `status` 显示 not running 是已知近似（无副作用、安全方向）。
#[derive(Debug)]
pub enum DaemonState {
    /// 无端口文件，或端口不可连接（`stale = true`）。
    Absent { stale: bool },
    /// TCP 可达但握手版本不兼容。
    Incompatible { message: String },
    /// 当前版本握手成功。
    Ready(DaemonAddr),
}

/// 只读探测：不删除端口文件、不 spawn、不调用 `ensure_daemon`。
pub async fn probe_daemon() -> DaemonState {
    let Some(addr) = read_daemon_addr() else {
        return DaemonState::Absent { stale: false };
    };
    match probe_version(&addr, PROTOCOL_VERSION, client_capabilities()).await {
        Ok(_) => DaemonState::Ready(addr),
        Err(ProbeError::Version(message)) => DaemonState::Incompatible { message },
        Err(ProbeError::Connect(_)) => DaemonState::Absent { stale: true },
        Err(ProbeError::Handshake(_)) => DaemonState::Absent { stale: false },
    }
}

fn parse_found_version(message: &str) -> Option<u32> {
    let rest = message.split("daemon ").nth(1)?;
    rest.split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|value| value.parse().ok())
}

/// 兼容性错误文案：包含两侧版本与恢复命令。
pub fn incompatible_message(found: Option<u32>) -> String {
    let found = found
        .map(|version| format!("v{version}"))
        .unwrap_or_else(|| "unknown".to_string());
    format!(
        "daemon protocol {found} is incompatible with this client (expected v{}); run `crossh daemon stop`",
        PROTOCOL_VERSION
    )
}

/// 按需拉起并返回可用地址：兼容 daemon 直接用；无监听时 detached 重拉；
/// 存活但协议不兼容时先按对端版本停掉旧 daemon，再拉起本版本
/// （旧 daemon 的会话随之结束，工作区布局在 Shutdown 时落盘）。
pub async fn ensure_daemon() -> io::Result<DaemonAddr> {
    let mut stop_failure = None;
    // 探活前快照：条件删除只认这个地址；期间文件被新主人替换则不动。
    let probed_addr = read_daemon_addr();
    match probe_daemon().await {
        DaemonState::Ready(addr) => return Ok(addr),
        DaemonState::Incompatible { message } => {
            let found = parse_found_version(&message);
            if let Some(addr) = read_daemon_addr() {
                match stop_incompatible_daemon(&addr, found).await {
                    Ok(()) => log::warn!(
                        "stopped incompatible daemon on port {} (protocol {found:?}); its sessions ended",
                        addr.port
                    ),
                    // 停止期间旧 daemon 可能已被并发客户端换成新版本，也可能
                    // 自行退出：能兼容连上就复用，确实为空则照常拉起；只有
                    // 仍是另一个不兼容实例时才把停止错误交给调用方。
                    Err(error) => match probe_daemon().await {
                        DaemonState::Ready(addr) => return Ok(addr),
                        DaemonState::Absent { .. } => {}
                        DaemonState::Incompatible { .. } => stop_failure = Some(error),
                    },
                }
            }
        }
        DaemonState::Absent { stale } => {
            if stale {
                // 真死（端口连不上）：只有文件仍等于探活快照才删，
                // 避免 ABA 删掉新主人刚写的文件。
                if let Some(addr) = probed_addr {
                    remove_port_file_if_matches(&addr);
                }
            } else if read_daemon_addr().is_some() {
                // 有文件但不断定陈旧 = 连上却握手失败，对端还活着
                // （忙/慢/令牌过期）：不删文件、不拉新实例，直接返回可执行错误。
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    format!(
                        "daemon is running but not responding to handshake (it may be busy, or the port file is outdated); retry, or run `crossh daemon stop` then retry ({})",
                        daemon_port_file_path().display()
                    ),
                ));
            }
        }
    }
    if let Some(error) = stop_failure {
        return Err(error);
    }
    spawn_detached()?;
    let deadline = Instant::now() + SPAWN_WAIT;
    loop {
        match probe_daemon().await {
            DaemonState::Ready(addr) => return Ok(addr),
            DaemonState::Incompatible { message } => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    incompatible_message(parse_found_version(&message)),
                ));
            }
            DaemonState::Absent { .. } => {}
        }
        if Instant::now() >= deadline {
            let reason = crate::workspace_store::read_daemon_error()
                .map(|reason| format!(": {reason}"))
                .unwrap_or_default();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("crossh-daemon did not come up{reason}"),
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// 已握手的连接（GUI/CLI 共用）。
pub struct Client {
    reader: OwnedReadHalf,
    writer: OwnedWriteHalf,
    pub client_id: u64,
    /// 握手协商出的生效能力交集（`reattach-nudge-v1` 有无决定回放形态）。
    capabilities: Vec<String>,
}

impl Client {
    /// 是否具备重连自愈能力（双方都声明 `reattach-nudge-v1` 才为真）。
    pub fn supports_reattach_nudge(&self) -> bool {
        self.capabilities
            .iter()
            .any(|c| c == capabilities::REATTACH_NUDGE_V1)
    }
    pub async fn connect() -> io::Result<Self> {
        let addr = ensure_daemon().await?;
        Self::connect_to(&addr).await
    }

    pub async fn connect_to(addr: &DaemonAddr) -> io::Result<Self> {
        let sock = TcpStream::connect(("127.0.0.1", addr.port)).await?;
        let (mut reader, mut writer) = sock.into_split();
        let hello = HandshakeRequest {
            version: PROTOCOL_VERSION,
            capabilities: client_capabilities(),
            token: addr.token.clone(),
        };
        send_frame(&mut writer, MessageKind::Hello, &json(&hello)).await?;
        let (kind, payload) = read_timeout(&mut reader).await?;
        if kind != MessageKind::Welcome {
            return Err(client_error(&payload));
        }
        let ok: HandshakeOk = serde_json::from_slice(&payload)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad welcome"))?;
        Ok(Self {
            reader,
            writer,
            client_id: ok.client_id,
            capabilities: ok.capabilities,
        })
    }

    pub async fn list(&mut self) -> io::Result<Vec<SessionInfo>> {
        send_frame(&mut self.writer, MessageKind::List, &[]).await?;
        loop {
            let (kind, payload) = read_frame(&mut self.reader).await?;
            match kind {
                MessageKind::Listed => {
                    return serde_json::from_slice::<ListReply>(&payload)
                        .map(|r| r.sessions)
                        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad listed"));
                }
                // 实时输出抢先到达：暂存丢弃（`ls` 只要列表）。
                MessageKind::Output => {}
                MessageKind::Exited => {}
                MessageKind::Error => {
                    let msg = serde_json::from_slice::<ErrorPayload>(&payload)
                        .map(|e| e.message)
                        .unwrap_or_default();
                    return Err(io::Error::other(msg));
                }
                _ => {}
            }
        }
    }

    /// 建交互 shell 会话并自动绑定（创建者即 controller）。
    pub async fn create_shell(
        &mut self,
        cwd: Option<String>,
        cols: u16,
        rows: u16,
    ) -> io::Result<Attached> {
        self.create(CreateRequest {
            shell: None,
            cwd,
            cols,
            rows,
            command: None,
            env: Some(caller_env()),
        })
        .await
    }

    async fn create(&mut self, req: CreateRequest) -> io::Result<Attached> {
        send_frame(&mut self.writer, MessageKind::Create, &json(&req)).await?;
        let (kind, payload) = read_timeout(&mut self.reader).await?;
        if kind == MessageKind::Error {
            return Err(client_error(&payload));
        }
        if kind != MessageKind::Created {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "expected created",
            ));
        }
        let _created: CreatedReply = serde_json::from_slice(&payload)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad created"))?;
        self.read_attached().await
    }

    /// 接入已有会话：返回会话信息 + 到 `ReplayEnd` 为止的回放字节。
    pub async fn attach(&mut self, id: &str) -> io::Result<Attached> {
        send_frame(
            &mut self.writer,
            MessageKind::Attach,
            &json(&AttachRequest { id: id.to_string() }),
        )
        .await?;
        self.read_attached().await
    }

    async fn read_attached(&mut self) -> io::Result<Attached> {
        let (kind, payload) = read_timeout(&mut self.reader).await?;
        if kind == MessageKind::Error {
            return Err(client_error(&payload));
        }
        if kind != MessageKind::Attached {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "expected attached",
            ));
        }
        let attached: AttachedReply = serde_json::from_slice(&payload)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad attached"))?;
        let mut replay = Vec::new();
        loop {
            let (kind, payload) = read_timeout(&mut self.reader).await?;
            match kind {
                MessageKind::Output => {
                    let (_, bytes) = decode_output(&payload)?;
                    replay.extend_from_slice(&bytes);
                }
                MessageKind::ReplayEnd => {
                    let _end: ReplayEndMarker =
                        serde_json::from_slice(&payload).unwrap_or(ReplayEndMarker {
                            end_seq: attached.end_seq,
                        });
                    break;
                }
                MessageKind::Error => return Err(client_error(&payload)),
                // 分段几何：3.2 只单流显示（拼 `Output` 字节），几何留给 4.1 前端。
                MessageKind::Size => {}
                _ => {}
            }
        }
        Ok(Attached {
            session: attached.session,
            replay,
        })
    }

    pub async fn input(&mut self, bytes: &[u8]) -> io::Result<()> {
        send_frame(&mut self.writer, MessageKind::Input, bytes).await
    }

    pub async fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        send_frame(
            &mut self.writer,
            MessageKind::Resize,
            &json(&ResizeRequest { cols, rows }),
        )
        .await
    }

    pub async fn close(&mut self, id: &str) -> io::Result<()> {
        send_frame(
            &mut self.writer,
            MessageKind::Close,
            &json(&CloseRequest { id: id.to_string() }),
        )
        .await
    }

    /// 读下一帧服务端事件：`Output` 解码为字节，`Exited` 为结束。
    pub async fn next_event(&mut self) -> io::Result<ServerEvent> {
        loop {
            let (kind, payload) = read_frame(&mut self.reader).await?;
            match kind {
                MessageKind::Output => {
                    let (seq, bytes) = decode_output(&payload)?;
                    return Ok(ServerEvent::Output { seq, bytes });
                }
                MessageKind::Exited => {
                    let ntf: ExitedNotice = serde_json::from_slice(&payload)
                        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad exited"))?;
                    return Ok(ServerEvent::Exited(ntf));
                }
                MessageKind::Error => return Err(client_error(&payload)),
                _ => {}
            }
        }
    }

    /// 读取一份工作区快照（不订阅后续更新）。
    pub async fn workspace_get(&mut self) -> io::Result<WorkspaceSnapshot> {
        send_frame(&mut self.writer, MessageKind::WorkspaceGet, &[]).await?;
        loop {
            let (kind, payload) = read_timeout(&mut self.reader).await?;
            match kind {
                MessageKind::WorkspaceSnapshot => {
                    return serde_json::from_slice(&payload).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "bad workspace snapshot")
                    });
                }
                MessageKind::Error => return Err(client_error(&payload)),
                _ => {}
            }
        }
    }

    /// 订阅工作区：返回当前快照，后续用 `next_workspace_snapshot` 读取推送。
    pub async fn workspace_attach(&mut self) -> io::Result<WorkspaceSnapshot> {
        send_frame(&mut self.writer, MessageKind::WorkspaceAttach, &[]).await?;
        self.next_workspace_snapshot().await
    }

    pub async fn next_workspace_snapshot(&mut self) -> io::Result<WorkspaceSnapshot> {
        loop {
            let (kind, payload) = read_timeout(&mut self.reader).await?;
            match kind {
                MessageKind::WorkspaceSnapshot => {
                    return serde_json::from_slice(&payload).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "bad workspace snapshot")
                    });
                }
                MessageKind::Error => return Err(client_error(&payload)),
                _ => {}
            }
        }
    }

    /// 串行修改工作区并等待回执；中途到达的快照被忽略。
    pub async fn workspace_mutate(
        &mut self,
        request: WorkspaceMutateRequest,
    ) -> io::Result<WorkspaceMutateReply> {
        send_frame(
            &mut self.writer,
            MessageKind::WorkspaceMutate,
            &json(&request),
        )
        .await?;
        loop {
            let (kind, payload) = read_timeout(&mut self.reader).await?;
            match kind {
                MessageKind::WorkspaceMutateReply => {
                    return serde_json::from_slice(&payload).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "bad workspace reply")
                    });
                }
                MessageKind::Error => return Err(client_error(&payload)),
                _ => {}
            }
        }
    }
}

/// 建/接入会话后一次性返回的信息 + 回放。
pub struct Attached {
    pub session: SessionInfo,
    pub replay: Vec<u8>,
}

/// 服务端下行事件。
pub enum ServerEvent {
    Output { seq: u64, bytes: Vec<u8> },
    Exited(ExitedNotice),
}

async fn read_timeout(reader: &mut OwnedReadHalf) -> io::Result<(MessageKind, Vec<u8>)> {
    tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(reader))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "daemon response timeout"))?
}

fn client_error(payload: &[u8]) -> io::Error {
    let msg = serde_json::from_slice::<ErrorPayload>(payload)
        .map(|e| format!("{}: {}", e.code, e.message))
        .unwrap_or_else(|_| "daemon error".to_string());
    io::Error::other(msg)
}

/// 调用方环境快照：daemon 侧以此为会话基线，而非 daemon 自身冻结的环境。
fn caller_env() -> Vec<(String, String)> {
    std::env::vars_os()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .collect()
}

/// `run -- <argv...>`：建裸命令会话 → 流式转发输出 → 原样返回退出码。
/// 不转发 stdin（一次性命令）；`cwd` 为调用方当前目录。
pub async fn run_command(argv: &[String], out: &mut dyn Write) -> io::Result<i32> {
    if argv.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: crossh run -- <command> [args...]",
        ));
    }
    let cwd = std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    let mut client = Client::connect().await?;
    let attached = client
        .create(CreateRequest {
            shell: None,
            cwd,
            cols: 80,
            rows: 24,
            command: Some(argv.to_vec()),
            env: Some(caller_env()),
        })
        .await?;
    // 一次性命令可能在订阅建立前就退出（Windows 的 `cmd /C echo` 尤其常见）。
    // 回放与退出码已在 `Attached` 里，先消费掉再等实时事件。
    out.write_all(&attached.replay)?;
    out.flush()?;
    if let Some(code) = attached.session.exit_code {
        return Ok(code);
    }
    loop {
        match client.next_event().await? {
            ServerEvent::Output { bytes, .. } => {
                out.write_all(&bytes)?;
                out.flush()?;
            }
            ServerEvent::Exited(ntf) => return Ok(ntf.code),
        }
    }
}

/// `capture <id>`：接入会话，把到 `ReplayEnd` 为止的回放字节写入 `out` 后返回。
pub async fn capture_session(id: &str, out: &mut dyn Write) -> io::Result<SessionInfo> {
    let mut client = Client::connect().await?;
    let attached = client.attach(id).await?;
    out.write_all(&attached.replay)?;
    out.flush()?;
    Ok(attached.session)
}

/// `daemon stop`：直接连接现有 daemon，任何分支都不拉起新实例。
///
/// 当前版本握手遇到版本错误时，按对端在错误里报告的版本重试握手
/// （v1 起的 `Hello` 字段与 `Shutdown = 16` 编号不变）；两种握手都
/// 失败时给出可执行错误。
pub async fn daemon_stop() -> io::Result<()> {
    let Some(addr) = read_daemon_addr() else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "daemon is not running (no {})",
                daemon_port_file_path().display()
            ),
        ));
    };
    let version = match probe_version(&addr, PROTOCOL_VERSION, client_capabilities()).await {
        Ok(_) => PROTOCOL_VERSION,
        Err(ProbeError::Version(message)) => {
            let Some(found) =
                parse_found_version(&message).filter(|version| *version != PROTOCOL_VERSION)
            else {
                return Err(stop_error(&message));
            };
            match probe_version(&addr, found, Vec::new()).await {
                Ok(_) => found,
                Err(ProbeError::Version(message)) => return Err(stop_error(&message)),
                Err(ProbeError::Connect(error)) | Err(ProbeError::Handshake(error)) => {
                    return Err(stop_io_error(error));
                }
            }
        }
        Err(ProbeError::Connect(error)) | Err(ProbeError::Handshake(error)) => {
            return Err(stop_io_error(error));
        }
    };
    shutdown_daemon(&addr, version).await
}

/// 停掉已确认版本不兼容的 daemon。只按对端报告的版本握手：并发客户端
/// 可能已拉起新 daemon，用当前版本探测成功并不代表它就是旧实例。
async fn stop_incompatible_daemon(addr: &DaemonAddr, found: Option<u32>) -> io::Result<()> {
    let Some(version) = found.filter(|version| *version != PROTOCOL_VERSION) else {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            incompatible_message(found),
        ));
    };
    match probe_version(addr, version, Vec::new()).await {
        Ok(_) => shutdown_daemon(addr, version).await,
        Err(ProbeError::Version(message)) => Err(stop_error(&message)),
        Err(ProbeError::Connect(error)) | Err(ProbeError::Handshake(error)) => {
            Err(stop_io_error(error))
        }
    }
}

fn stop_error(message: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "cannot stop daemon at {}: {message}; run `crossh daemon stop` with the matching crossh version",
            daemon_port_file_path().display()
        ),
    )
}

fn stop_io_error(error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!(
            "cannot stop daemon at {}: {error}; the daemon may be from another crossh version",
            daemon_port_file_path().display()
        ),
    )
}

/// 用指定协议版本握手并发送 `Shutdown`，等到端口不可连接。
async fn shutdown_daemon(addr: &DaemonAddr, version: u32) -> io::Result<()> {
    let sock = TcpStream::connect(("127.0.0.1", addr.port)).await?;
    let (mut r, mut w) = sock.into_split();
    let hello = HandshakeRequest {
        version,
        capabilities: if version == PROTOCOL_VERSION {
            client_capabilities()
        } else {
            Vec::new()
        },
        token: addr.token.clone(),
    };
    send_frame(&mut w, MessageKind::Hello, &json(&hello)).await?;
    let (kind, payload) = read_frame(&mut r).await?;
    if kind != MessageKind::Welcome {
        return Err(client_error(&payload));
    }
    send_frame(&mut w, MessageKind::Shutdown, &[]).await?;
    // 等端口消失或连不上（服务端删文件后退出）。
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", addr.port)).await.is_err() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "daemon did not stop",
    ))
}

/// 工作区订阅连接：一个读任务持续接收快照与回执，写端串行发送修改。
///
/// GUI 用它做主连接；每个终端的 `DaemonLink` 仍是独立连接。
pub struct WorkspaceConnection {
    writer: Arc<Mutex<OwnedWriteHalf>>,
    pending: Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<WorkspaceMutateReply>>>>,
    snapshots: watch::Receiver<Arc<WorkspaceSnapshot>>,
    next_id: AtomicU64,
}

impl WorkspaceConnection {
    pub async fn connect() -> io::Result<Self> {
        let addr = ensure_daemon().await?;
        Self::connect_to(addr).await
    }

    pub async fn connect_to(addr: DaemonAddr) -> io::Result<Self> {
        let sock = TcpStream::connect(("127.0.0.1", addr.port)).await?;
        let (mut reader, mut writer) = sock.into_split();
        let hello = HandshakeRequest {
            version: PROTOCOL_VERSION,
            capabilities: client_capabilities(),
            token: addr.token.clone(),
        };
        send_frame(&mut writer, MessageKind::Hello, &json(&hello)).await?;
        let (kind, payload) = read_timeout(&mut reader).await?;
        if kind != MessageKind::Welcome {
            return Err(client_error(&payload));
        }
        send_frame(&mut writer, MessageKind::WorkspaceAttach, &[]).await?;
        let initial: WorkspaceSnapshot = loop {
            let (kind, payload) = read_timeout(&mut reader).await?;
            match kind {
                MessageKind::WorkspaceSnapshot => {
                    break serde_json::from_slice(&payload).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "bad workspace snapshot")
                    })?;
                }
                MessageKind::Error => return Err(client_error(&payload)),
                _ => {}
            }
        };
        let (snapshot_tx, snapshots) = watch::channel(Arc::new(initial));
        let pending: Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<WorkspaceMutateReply>>>> =
            Arc::new(std::sync::Mutex::new(HashMap::new()));
        let reader_pending = pending.clone();
        tokio::spawn(async move {
            loop {
                let (kind, payload) = match read_frame(&mut reader).await {
                    Ok(frame) => frame,
                    Err(_) => break,
                };
                match kind {
                    MessageKind::WorkspaceSnapshot => {
                        if let Ok(snapshot) = serde_json::from_slice::<WorkspaceSnapshot>(&payload)
                        {
                            let _ = snapshot_tx.send(Arc::new(snapshot));
                        }
                    }
                    MessageKind::WorkspaceMutateReply => {
                        if let Ok(reply) = serde_json::from_slice::<WorkspaceMutateReply>(&payload)
                        {
                            let sender = reader_pending
                                .lock()
                                .ok()
                                .and_then(|mut pending| pending.remove(&reply.request_id));
                            if let Some(sender) = sender {
                                let _ = sender.send(reply);
                            }
                        }
                    }
                    MessageKind::Error => {
                        log::warn!("workspace connection error: {}", client_error(&payload));
                    }
                    _ => {}
                }
            }
        });
        Ok(Self {
            writer: Arc::new(Mutex::new(writer)),
            pending,
            snapshots,
            next_id: AtomicU64::new(1),
        })
    }

    pub fn snapshots(&self) -> watch::Receiver<Arc<WorkspaceSnapshot>> {
        self.snapshots.clone()
    }

    pub fn current(&self) -> Arc<WorkspaceSnapshot> {
        self.snapshots.borrow().clone()
    }

    pub async fn mutate(
        &self,
        op: WorkspaceOp,
        expected_revision: Option<u64>,
    ) -> io::Result<WorkspaceMutateReply> {
        let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = WorkspaceMutateRequest {
            request_id,
            expected_revision,
            op,
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        if let Ok(mut pending) = self.pending.lock() {
            pending.insert(request_id, reply_tx);
        }
        let send = async {
            let mut writer = self.writer.lock().await;
            send_frame(&mut writer, MessageKind::WorkspaceMutate, &json(&request)).await
        };
        if let Err(error) = send.await {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&request_id);
            }
            return Err(error);
        }
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, reply_rx).await {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(_)) => Err(io::Error::other("workspace connection closed")),
            Err(_) => {
                if let Ok(mut pending) = self.pending.lock() {
                    pending.remove(&request_id);
                }
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "workspace mutate reply timed out",
                ))
            }
        }
    }

    pub async fn refresh_availability(&self) -> io::Result<WorkspaceMutateReply> {
        self.mutate(WorkspaceOp::RefreshAvailability, None).await
    }
}

/// 会话角色（相对观察者自身 id）：controller / observer / 无。
pub fn role_of(session: &SessionInfo, me: u64) -> &'static str {
    match session.controller {
        Some(id) if id == me => "controller",
        Some(_) => "observer",
        None => "none",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn found_version_is_parsed_from_mismatch_text() {
        assert_eq!(
            parse_found_version(
                "protocol version mismatch (daemon 1, client 2); run `crossh daemon stop` then retry"
            ),
            Some(1)
        );
        assert_eq!(parse_found_version("handshake rejected"), None);
    }

    /// 自动停止只能按对端报告过的版本握手：期间并发客户端可能已拉起
    /// 新 daemon，用当前版本探测成功并不代表它就是旧实例。
    #[tokio::test]
    async fn stop_incompatible_daemon_never_stops_a_compatible_instance() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                let (mut r, mut w) = sock.into_split();
                let Ok((kind, payload)) = read_frame(&mut r).await else {
                    continue;
                };
                assert_eq!(kind, MessageKind::Hello);
                let hello: HandshakeRequest = serde_json::from_slice(&payload).unwrap();
                if hello.version != PROTOCOL_VERSION {
                    let error = ErrorPayload {
                        code: "protocol".to_string(),
                        message: format!(
                            "protocol version mismatch (daemon {PROTOCOL_VERSION}, client {}); run `crossh daemon stop` then retry",
                            hello.version
                        ),
                    };
                    let _ = send_frame(&mut w, MessageKind::Error, &json(&error)).await;
                    continue;
                }
                let ok = HandshakeOk {
                    version: PROTOCOL_VERSION,
                    capabilities: vec![],
                    client_id: 1,
                };
                if send_frame(&mut w, MessageKind::Welcome, &json(&ok))
                    .await
                    .is_err()
                {
                    continue;
                }
                if let Ok((MessageKind::Shutdown, _)) = read_frame(&mut r).await {
                    let _ = shutdown_tx.send(());
                }
            }
        });

        let addr = DaemonAddr {
            port,
            token: "tok".to_string(),
        };
        let error = stop_incompatible_daemon(&addr, Some(PROTOCOL_VERSION.saturating_sub(1)))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(
            shutdown_rx.try_recv().is_err(),
            "a compatible daemon must never receive Shutdown"
        );
    }

    /// 假 daemon：握手后按给定帧应答一次 `Attach`（回放段由调用方编排）。
    async fn fake_attach_once(listener: tokio::net::TcpListener, frames: Vec<Vec<u8>>) {
        use tokio::io::AsyncWriteExt;
        let (conn, _) = listener.accept().await.expect("accept");
        let (mut r, mut w) = conn.into_split();
        let (kind, _) = read_frame(&mut r).await.expect("hello");
        assert_eq!(kind, MessageKind::Hello);
        let ok = HandshakeOk {
            version: PROTOCOL_VERSION,
            capabilities: vec![],
            client_id: 7,
        };
        send_frame(&mut w, MessageKind::Welcome, &json(&ok))
            .await
            .expect("welcome");
        let (kind, _) = read_frame(&mut r).await.expect("attach");
        assert_eq!(kind, MessageKind::Attach);
        for raw in frames {
            w.write_all(&raw).await.expect("frame");
        }
    }

    /// 假 daemon 的 `Attached` 应答帧（`end_seq` 即总流偏移，不含 `Size`）。
    fn attached_frame(end_seq: u64) -> Vec<u8> {
        let session = SessionInfo {
            id: "s1".to_string(),
            cwd: None,
            program: None,
            controller: None,
            alive: true,
            exit_code: None,
            project_id: None,
            tab_id: None,
            pane_id: None,
        };
        let attached = AttachedReply { session, end_seq };
        encode_frame(MessageKind::Attached, &json(&attached)).expect("attached")
    }

    /// 新 daemon → 旧 GUI：`Size` 与未来帧（99）穿插，跳过后仍显示回放。
    #[tokio::test]
    async fn attach_skips_size_and_unknown_still_shows_output() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let size = crate::workspace::TermSize {
            cols: 100,
            rows: 30,
        };
        let out = crate::protocol::encode_output(5, b"hello");
        let end = ReplayEndMarker { end_seq: 5 };
        // 未来帧（旧端未知 kind）：裸字节直写，读帧循环跳过。
        let mut future = (2u32.to_le_bytes()).to_vec();
        future.push(99u8);
        future.push(b'z');
        let frames = vec![
            attached_frame(5),
            encode_frame(MessageKind::Size, &json(&size)).unwrap(),
            future,
            encode_frame(MessageKind::Output, &out).unwrap(),
            encode_frame(MessageKind::ReplayEnd, &json(&end)).unwrap(),
        ];
        let server = tokio::spawn(fake_attach_once(listener, frames));
        let mut client = Client::connect_to(&DaemonAddr {
            port,
            token: "tok".to_string(),
        })
        .await
        .unwrap();
        let attached = client.attach("s1").await.unwrap();
        assert_eq!(attached.replay, b"hello");
        server.await.unwrap();
    }

    /// 新 GUI → 旧 daemon：无 `Size` 的旧单流照常退化显示，无需抖动。
    #[tokio::test]
    async fn attach_without_size_degrades_to_single_stream() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let out = crate::protocol::encode_output(5, b"hello");
        let end = ReplayEndMarker { end_seq: 5 };
        let frames = vec![
            attached_frame(5),
            encode_frame(MessageKind::Output, &out).unwrap(),
            encode_frame(MessageKind::ReplayEnd, &json(&end)).unwrap(),
        ];
        let server = tokio::spawn(fake_attach_once(listener, frames));
        let mut client = Client::connect_to(&DaemonAddr {
            port,
            token: "tok".to_string(),
        })
        .await
        .unwrap();
        let attached = client.attach("s1").await.unwrap();
        assert_eq!(attached.replay, b"hello");
        server.await.unwrap();
    }

    #[test]
    fn client_declares_reattach_nudge_and_detects_agreement() {
        // 能力声明：新 GUI 必带 `reattach-nudge-v1`。
        assert!(client_capabilities().contains(&capabilities::REATTACH_NUDGE_V1.to_string()));
    }
}

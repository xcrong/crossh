//! 第二前端的客户端：连接、握手、会话操作的高层封装。
//!
//! - GUI 与 CLI 共用同一套接入流程（发现 → 建/接会话 → 输入输出 → 结束）。
//! - `ensure_daemon` 按需拉起：探活失败则 detached  spawn（unix `setsid`，
//!   Windows `DETACHED_PROCESS`），环境照常继承（D6）。

use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

use crate::handshake::{
    ErrorPayload, HandshakeOk, HandshakeRequest, PROTOCOL_VERSION, capabilities,
};
use crate::instance::{DaemonAddr, daemon_port_file_path, read_daemon_addr};
use crate::protocol::{
    AttachRequest, AttachedReply, CloseRequest, CreateRequest, CreatedReply, ExitedNotice,
    ListReply, MAX_FRAME, MessageKind, ReplayEndMarker, ResizeRequest, SessionInfo, decode_frame,
    decode_output, encode_frame,
};

/// 握手等待上限（daemon 本地回环，常时毫秒级）。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// 拉起后等端口文件出现的最长等待。
const SPAWN_WAIT: Duration = Duration::from_secs(15);

async fn read_frame(reader: &mut OwnedReadHalf) -> io::Result<(MessageKind, Vec<u8>)> {
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
    decode_frame(&full)
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

async fn probe(addr: &DaemonAddr) -> io::Result<HandshakeOk> {
    let sock = tokio::time::timeout(
        Duration::from_millis(500),
        TcpStream::connect(("127.0.0.1", addr.port)),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "daemon connect timeout"))??;
    let (mut r, mut w) = sock.into_split();
    let hello = HandshakeRequest {
        version: PROTOCOL_VERSION,
        capabilities: vec![
            capabilities::REPLAY_MARK.to_string(),
            capabilities::OBSERVER_LOCAL_LAYOUT.to_string(),
            capabilities::READONLY_BADGE.to_string(),
        ],
        token: addr.token.clone(),
    };
    send_frame(&mut w, MessageKind::Hello, &json(&hello)).await?;
    let (kind, payload) = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(&mut r))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "daemon handshake timeout"))??;
    if kind != MessageKind::Welcome {
        let msg = serde_json::from_slice::<ErrorPayload>(&payload)
            .map(|e| e.message)
            .unwrap_or_else(|_| "handshake failed".to_string());
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, msg));
    }
    serde_json::from_slice(&payload)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad welcome"))
}

/// 按需拉起并返回可用地址：探活成功直接用；陈旧文件删除后 detached 重拉。
pub async fn ensure_daemon() -> io::Result<DaemonAddr> {
    if let Some(addr) = read_daemon_addr()
        && probe(&addr).await.is_ok()
    {
        return Ok(addr);
    }
    let _ = std::fs::remove_file(daemon_port_file_path());
    spawn_detached()?;
    let deadline = Instant::now() + SPAWN_WAIT;
    loop {
        if let Some(addr) = read_daemon_addr()
            && probe(&addr).await.is_ok()
        {
            return Ok(addr);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "crossh-daemon did not come up",
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
}

impl Client {
    pub async fn connect() -> io::Result<Self> {
        let addr = ensure_daemon().await?;
        Self::connect_to(&addr).await
    }

    pub async fn connect_to(addr: &DaemonAddr) -> io::Result<Self> {
        let sock = TcpStream::connect(("127.0.0.1", addr.port)).await?;
        let (mut reader, mut writer) = sock.into_split();
        let hello = HandshakeRequest {
            version: PROTOCOL_VERSION,
            capabilities: vec![],
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
    client
        .create(CreateRequest {
            shell: None,
            cwd,
            cols: 80,
            rows: 24,
            command: Some(argv.to_vec()),
        })
        .await
        .map(|_| ())?;
    // Attached + 回放已由 create() 消费；裸命令无 prompt，回放通常为空。
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

/// `daemon stop`：已认证停服，端口文件由服务端删除；随后探活确认退出。
pub async fn daemon_stop() -> io::Result<()> {
    let addr = ensure_daemon().await?;
    let sock = TcpStream::connect(("127.0.0.1", addr.port)).await?;
    let (mut r, mut w) = sock.into_split();
    let hello = HandshakeRequest {
        version: PROTOCOL_VERSION,
        capabilities: vec![],
        token: addr.token.clone(),
    };
    send_frame(&mut w, MessageKind::Hello, &json(&hello)).await?;
    let (kind, _) = read_frame(&mut r).await?;
    if kind != MessageKind::Welcome {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "handshake failed",
        ));
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

/// 会话角色（相对观察者自身 id）：controller / observer / 无。
pub fn role_of(session: &SessionInfo, me: u64) -> &'static str {
    match session.controller {
        Some(id) if id == me => "controller",
        Some(_) => "observer",
        None => "none",
    }
}

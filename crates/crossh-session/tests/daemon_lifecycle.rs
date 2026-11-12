//! daemon 生命周期集成测试（3.4）：spawn 二进制 → 握手 → 建会话 → 输入输出 → 结束。
//!
//! 同时覆盖 2.4（observer 首输入丢弃+接管）、2.5（observer resize 静默忽略、
//! controller resize 生效由 `stty size` 证明）、2.6（`exit 3` 退出码透出）。
//! 单测独立隔离：`CROSSH_CACHE_DIR` 指向临时目录。

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossh_session::handshake::{HandshakeOk, HandshakeRequest, PROTOCOL_VERSION};
use crossh_session::protocol::{
    AttachRequest, AttachedReply, CloseRequest, CreateRequest, CreatedReply, ExitedNotice,
    ListReply, MessageKind, decode_output, encode_frame,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

struct Conn {
    id: u64,
    reader: OwnedReadHalf,
    writer: OwnedWriteHalf,
    out_buf: Vec<u8>,
}

async fn read_frame(reader: &mut OwnedReadHalf) -> io::Result<(MessageKind, Vec<u8>)> {
    let mut head = [0u8; 5];
    reader.read_exact(&mut head).await?;
    let len = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
    if len < 1 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad len"));
    }
    let mut rest = vec![0u8; len - 1];
    reader.read_exact(&mut rest).await?;
    let kind = MessageKind::from_u8(head[4])
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad kind"))?;
    Ok((kind, rest))
}

async fn send(writer: &mut OwnedWriteHalf, kind: MessageKind, payload: &[u8]) -> io::Result<()> {
    let frame = encode_frame(kind, payload)?;
    writer.write_all(&frame).await
}

fn json<T: serde::Serialize>(v: &T) -> Vec<u8> {
    serde_json::to_vec(v).unwrap()
}

async fn connect(port: u16, token: &str) -> io::Result<Conn> {
    let sock = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    let (mut reader, mut writer) = sock.into_split();
    let hello = HandshakeRequest {
        version: PROTOCOL_VERSION,
        capabilities: vec![],
        token: token.to_string(),
    };
    send(&mut writer, MessageKind::Hello, &json(&hello)).await?;
    let (kind, payload) = read_frame(&mut reader).await?;
    assert_eq!(kind, MessageKind::Welcome);
    let ok: HandshakeOk = serde_json::from_slice(&payload).unwrap();
    assert_eq!(ok.version, PROTOCOL_VERSION);
    Ok(Conn {
        id: ok.client_id,
        reader,
        writer,
        out_buf: Vec::new(),
    })
}

/// 读帧直到 `pred` 对累计输出成立（`Output` 载荷解码累加；跳过其他帧）。
/// 返回新收到的 `Exited`（若有）。
async fn collect_until(
    conn: &mut Conn,
    pred: impl Fn(&[u8]) -> bool,
    timeout: Duration,
) -> io::Result<Option<ExitedNotice>> {
    let deadline = Instant::now() + timeout;
    loop {
        if pred(&conn.out_buf) {
            return Ok(None);
        }
        let remain = deadline.saturating_duration_since(Instant::now());
        if remain.is_zero() {
            panic!(
                "timeout waiting for output; got {:?}",
                String::from_utf8_lossy(&conn.out_buf)
            );
        }

        let (kind, payload) = tokio::time::timeout(remain, read_frame(&mut conn.reader))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "read timeout"))??;
        match kind {
            MessageKind::Output => {
                let (_, bytes) = decode_output(&payload)?;
                conn.out_buf.extend_from_slice(&bytes);
            }
            MessageKind::Exited => {
                let ntf: ExitedNotice = serde_json::from_slice(&payload).unwrap();
                return Ok(Some(ntf));
            }
            _ => {}
        }
    }
}

async fn list(conn: &mut Conn) -> Vec<crossh_session::protocol::SessionInfo> {
    conn.out_buf.clear();
    send(&mut conn.writer, MessageKind::List, &[])
        .await
        .unwrap();
    // 实时 Output 可能抢先到达：跳过它们直到 `Listed`。
    loop {
        let (kind, payload) = read_frame(&mut conn.reader).await.unwrap();
        match kind {
            MessageKind::Listed => {
                return serde_json::from_slice::<ListReply>(&payload)
                    .unwrap()
                    .sessions;
            }
            MessageKind::Output => {
                let (_, bytes) = decode_output(&payload).unwrap();
                conn.out_buf.extend_from_slice(&bytes);
            }
            // 刚结束的会话的退出通知可能抢先：调用方另行断言，这里跳过。
            MessageKind::Exited => {}
            other => panic!("expected Listed, got {other:?}"),
        }
    }
}
async fn drain_replay(conn: &mut Conn) {
    loop {
        let (kind, payload) = read_frame(&mut conn.reader).await.unwrap();
        match kind {
            MessageKind::Output => {
                let (_, bytes) = decode_output(&payload).unwrap();
                conn.out_buf.extend_from_slice(&bytes);
            }
            MessageKind::ReplayEnd => return,
            other => panic!("expected replay/output, got {other:?}"),
        }
    }
}

fn cache_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("crossh-daemon-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn daemon_full_cycle() {
    let dir = cache_dir();
    // SAFETY: 测试进程内最早设置，后续只读。
    unsafe { std::env::set_var("CROSSH_CACHE_DIR", &dir) };
    let daemon_bin = env!("CARGO_BIN_EXE_crossh-daemon");
    let child = std::process::Command::new(daemon_bin)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn daemon");

    struct Guard {
        child: std::process::Child,
        dir: PathBuf,
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    let mut _guard = Guard {
        child,
        dir: dir.clone(),
    };
    let child = &mut _guard.child;

    // 等端口文件出现并读令牌。
    let port_file = dir.join("daemon").join("daemon.port");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !port_file.exists() {
        if Instant::now() > deadline {
            panic!("daemon port file never appeared");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let text = std::fs::read_to_string(&port_file).unwrap();
    let mut lines = text.lines();
    let port: u16 = lines.next().unwrap().parse().unwrap();
    let token = lines.next().unwrap().to_string();

    // 无令牌拿不到会话列表（3.2/规格 R 认证）。
    {
        let sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let (mut r, mut w) = sock.into_split();
        let bad = HandshakeRequest {
            version: PROTOCOL_VERSION,
            capabilities: vec![],
            token: "wrong".to_string(),
        };
        send(&mut w, MessageKind::Hello, &json(&bad)).await.unwrap();
        let (kind, _) = read_frame(&mut r).await.unwrap();
        assert_eq!(kind, MessageKind::Error);
    }

    // 空列表起步。
    let mut c1 = connect(port, &token).await.unwrap();
    assert!(list(&mut c1).await.is_empty());

    // 建会话（创建者自动绑定 + 回放 + 边界帧）。
    let create = CreateRequest {
        shell: Some("/bin/sh".to_string()),
        cwd: None,
        cols: 80,
        rows: 24,
        command: None,
    };
    send(&mut c1.writer, MessageKind::Create, &json(&create))
        .await
        .unwrap();
    let (kind, payload) = read_frame(&mut c1.reader).await.unwrap();
    assert_eq!(kind, MessageKind::Created);
    let created: CreatedReply = serde_json::from_slice(&payload).unwrap();
    let sid = created.session.id.clone();
    let (kind, _) = read_frame(&mut c1.reader).await.unwrap();
    assert_eq!(kind, MessageKind::Attached);
    // 回放（可能非空：prompt 先到）+ 边界。
    drain_replay(&mut c1).await;

    // 基本输入输出。
    send(&mut c1.writer, MessageKind::Input, b"echo hello-daemon\n")
        .await
        .unwrap();
    collect_until(
        &mut c1,
        |b| String::from_utf8_lossy(b).contains("hello-daemon"),
        Duration::from_secs(10),
    )
    .await
    .unwrap();

    // 第二个客户端只观察；首输入被丢弃但完成接管（2.4）。
    let mut c2 = connect(port, &token).await.unwrap();
    send(
        &mut c2.writer,
        MessageKind::Attach,
        &json(&AttachRequest { id: sid.clone() }),
    )
    .await
    .unwrap();
    let (kind, payload) = read_frame(&mut c2.reader).await.unwrap();
    assert_eq!(kind, MessageKind::Attached);
    let attached: AttachedReply = serde_json::from_slice(&payload).unwrap();
    assert_eq!(attached.session.controller, Some(c1.id));
    drain_replay(&mut c2).await;
    // observer 首输入不进 PTY。
    c2.out_buf.clear();
    send(&mut c2.writer, MessageKind::Input, b"echo dropped-marker\n")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    // 排空这 1s 内到达 c2 的帧，确认无 marker。
    while let Ok(Ok((kind, payload))) =
        tokio::time::timeout(Duration::from_millis(100), read_frame(&mut c2.reader)).await
    {
        if kind == MessageKind::Output {
            let (_, bytes) = decode_output(&payload).unwrap();
            c2.out_buf.extend_from_slice(&bytes);
        }
    }
    assert!(
        !String::from_utf8_lossy(&c2.out_buf).contains("dropped-marker"),
        "observer first input must not enter pty"
    );
    // 但接管已完成：列表 controller 易手，从此进入。
    let sessions = list(&mut c1).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].controller, Some(c2.id));
    send(&mut c2.writer, MessageKind::Input, b"echo taken-marker\n")
        .await
        .unwrap();
    collect_until(
        &mut c2,
        |b| String::from_utf8_lossy(b).contains("taken-marker"),
        Duration::from_secs(10),
    )
    .await
    .unwrap();

    // 尺寸：observer（c1，已失权）上报被静默忽略；controller 生效（2.5）。
    send(
        &mut c1.writer,
        MessageKind::Resize,
        &json(&crossh_session::protocol::ResizeRequest { cols: 10, rows: 10 }),
    )
    .await
    .unwrap();
    send(
        &mut c2.writer,
        MessageKind::Resize,
        &json(&crossh_session::protocol::ResizeRequest {
            cols: 100,
            rows: 40,
        }),
    )
    .await
    .unwrap();
    send(&mut c2.writer, MessageKind::Input, b"stty size\n")
        .await
        .unwrap();
    collect_until(
        &mut c2,
        |b| String::from_utf8_lossy(b).contains("40 100"),
        Duration::from_secs(10),
    )
    .await
    .unwrap();

    // 结束会话：在接双方都收到 Exited，随后列表为空（2.6）。
    send(
        &mut c1.writer,
        MessageKind::Close,
        &json(&CloseRequest { id: sid.clone() }),
    )
    .await
    .unwrap();
    let exited = collect_until(&mut c2, |_| false, Duration::from_secs(10))
        .await
        .unwrap();
    let exited = exited.expect("expected Exited");
    assert_eq!(exited.id, sid);
    assert!(list(&mut c1).await.is_empty());

    // 退出码透出：`exit 3`。
    send(
        &mut c1.writer,
        MessageKind::Create,
        &json(&CreateRequest {
            shell: Some("/bin/sh".to_string()),
            cwd: None,
            cols: 80,
            rows: 24,
            command: None,
        }),
    )
    .await
    .unwrap();
    let (kind, payload) = read_frame(&mut c1.reader).await.unwrap();
    assert_eq!(kind, MessageKind::Created);
    let created: CreatedReply = serde_json::from_slice(&payload).unwrap();
    // 消费 Attached + 回放 + 边界。
    let (kind, _) = read_frame(&mut c1.reader).await.unwrap();
    assert_eq!(kind, MessageKind::Attached);
    drain_replay(&mut c1).await;
    send(&mut c1.writer, MessageKind::Input, b"exit 3\n")
        .await
        .unwrap();
    let exited = collect_until(&mut c1, |_| false, Duration::from_secs(10))
        .await
        .unwrap();
    let exited = exited.expect("expected Exited");
    assert_eq!(exited.id, created.session.id);

    // GUI 退出只 detach 不杀会话：建会话后直接断开 TCP（不发 Close，
    // 模拟 view 销毁时 `cmd` 通道关闭），会话必须仍在且可被新连接列出。
    // 若客户端在断开时误发 Close，这里会直接断言失败。
    send(
        &mut c1.writer,
        MessageKind::Create,
        &json(&CreateRequest {
            shell: Some("/bin/sh".to_string()),
            cwd: None,
            cols: 80,
            rows: 24,
            command: None,
        }),
    )
    .await
    .unwrap();
    let (kind, payload) = read_frame(&mut c1.reader).await.unwrap();
    assert_eq!(kind, MessageKind::Created);
    let created: CreatedReply = serde_json::from_slice(&payload).unwrap();
    let orphan = created.session.id.clone();
    let (kind, _) = read_frame(&mut c1.reader).await.unwrap();
    assert_eq!(kind, MessageKind::Attached);
    drain_replay(&mut c1).await;
    drop(c1);
    let mut c3 = connect(port, &token).await.unwrap();
    let sessions = list(&mut c3).await;
    assert!(
        sessions.iter().any(|s| s.id == orphan && s.alive),
        "dropped connection must not end the session: {sessions:?}"
    );
    // 显式 Close 仍结束会话（与 detach 语义区分）。
    send(
        &mut c3.writer,
        MessageKind::Close,
        &json(&CloseRequest { id: orphan.clone() }),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(list(&mut c3).await.iter().all(|s| s.id != orphan));

    let _ = child.kill();
}

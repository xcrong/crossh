//! 连接抽象：一条已认证的 SSH 会话，可向其请求多个 channel（终端/SFTP/转发）。
//!
//! 数据流：
//!  - gpui 侧持 `Entity<Connection>`，通过 `open_terminal` 同步拿回一个
//!    `(Sender<InputCmd>, Receiver<SessionEvent>)` 终端桥接。
//!  - 后台 `run_connection` 任务：connect（含反应式主机密钥确认）→
//!    逐个认证（加密密钥/密码按需向 UI 索要口令）→
//!    `ConnEvent::Connected` → 进入 channel 服务循环，按 `ConnCmd` 开 channel 并派发 relay。
//!  - 生命周期：channel 引用计数；全部关闭或 gpui 释放 Connection 时 disconnect。
//!
//! 反应式凭据/主机密钥：后台在需要时经 `ConnEvent::NeedHostKey` / `NeedCredential`
//! 把 oneshot 回传通道交给 UI；UI 弹模态、用户决定后回传，后台继续。UI 不响应有超时兜底。

use std::sync::Arc;
use std::time::Duration;

use async_channel::{Receiver, Sender};
use gpui::{App, AppContext, Entity, Task};
use russh::client::{self, Handle};
use russh::keys;
use russh::keys::ssh_key::PrivateKey;
use russh::{ChannelMsg, ChannelReadHalf, ChannelWriteHalf, Disconnect};
use tokio::sync::mpsc as tokio_mpsc;
use tokio::sync::oneshot;

use crate::config::HostConfig;
use crate::ui::terminal_view::ConnState;

use super::forward::{
    RemoteForwardRegistry, handle_forwarded_tcpip, new_remote_forward_registry,
    run_dynamic_forward, run_local_forward, start_remote_forward, stop_remote_forward,
};
use super::runtime::runtime;
use super::session::{AuthChoice, InputCmd, SessionEvent, default_user_for};
use super::sftp::{SftpCmd, SftpEvent, run_sftp_worker};

/// UI 主机密钥决定（NeedHostKey 的回传）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyDecision {
    /// 本次接受，不写 known_hosts。
    AcceptOnce,
    /// 接受并写入 known_hosts。
    AcceptAlways,
    /// 拒绝（中止连接）。
    Reject,
}

/// 凭据种类（NeedCredential 的回传）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    /// 私钥口令。
    Passphrase,
    /// 账号密码。
    Password,
}

/// 发往连接任务的命令（gpui → 后台）。
pub enum ConnCmd {
    /// 开一个终端 channel。`input_rx`/`event_tx` 由调用方创建并移交，
    /// 后台在其上驱动 relay；调用方保留对应的 `input_tx`/`event_rx`。
    OpenTerminal {
        cols: u16,
        rows: u16,
        input_rx: Receiver<InputCmd>,
        event_tx: Sender<SessionEvent>,
    },
    /// 开一个 SFTP 工作器。worker 在连接的 sftp 子系统 channel 上运行。
    OpenSftp {
        cmd_rx: Receiver<SftpCmd>,
        event_tx: Sender<SftpEvent>,
    },
    /// 启动一条端口转发（-L/-R/-D）。
    StartForward {
        spec: crate::config::ForwardSpec,
        kind: ForwardKind,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// 停止一条端口转发。
    StopForward {
        spec: crate::config::ForwardSpec,
        kind: ForwardKind,
    },
    /// 主动断开。
    #[allow(dead_code)]
    Shutdown,
}

/// 转发类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ForwardKind {
    Local,
    Remote,
    Dynamic,
}

/// 连接级事件（后台 → gpui）。
pub enum ConnEvent {
    /// TCP+KEX+认证完成，可开 channel。
    Connected,
    /// 连接/认证失败。
    Error(String),
    /// 连接已关闭。
    Closed,
    /// 需要用户确认主机密钥（未知主机或密钥变更）。
    NeedHostKey {
        host: String,
        port: u16,
        key_type: String,
        fingerprint: String,
        /// true = 已知但密钥变更（可能 MITM）。
        changed: bool,
        reply: oneshot::Sender<HostKeyDecision>,
    },
    /// 需要用户输入凭据（私钥口令 / 密码）。回传 None 表示取消。
    NeedCredential {
        kind: CredentialKind,
        prompt: String,
        reply: oneshot::Sender<Option<String>>,
    },
}

/// 一条 SSH 连接（gpui Entity）。
pub struct Connection {
    cmd_tx: Sender<ConnCmd>,
    pub state: ConnState,
    /// 后台请求 UI 决定的弹窗（主机密钥确认 / 凭据输入）。None = 无待处理。
    pub pending_prompt: Option<PendingPrompt>,
    _drain: Option<Task<()>>,
}

/// UI 待处理的请求（由后台经 ConnEvent 触发，主线程 drain 写入）。
pub enum PendingPrompt {
    HostKey {
        host: String,
        port: u16,
        key_type: String,
        fingerprint: String,
        changed: bool,
        reply: Option<oneshot::Sender<HostKeyDecision>>,
    },
    Credential {
        kind: CredentialKind,
        prompt: String,
        reply: Option<oneshot::Sender<Option<String>>>,
    },
}

impl Connection {
    /// 立即在后台发起连接；返回的 Entity 可用于 `open_terminal`。
    pub fn open(
        host: HostConfig,
        methods: Vec<AuthChoice>,
        ssh_config: Arc<crate::config::SshConfig>,
        cx: &mut App,
    ) -> Entity<Self> {
        let (cmd_tx, cmd_rx) = async_channel::bounded::<ConnCmd>(64);
        let (event_tx, event_rx) = async_channel::bounded::<ConnEvent>(64);

        runtime().spawn(async move {
            let result = run_connection(host, methods, ssh_config, cmd_rx, event_tx.clone()).await;
            match result {
                Ok(()) => {
                    let _ = event_tx.send(ConnEvent::Closed).await;
                }
                Err(e) => {
                    let _ = event_tx.send(ConnEvent::Error(e.to_string())).await;
                }
            }
        });

        let entity = cx.new(|_cx| Self {
            cmd_tx,
            state: ConnState::Connecting,
            pending_prompt: None,
            _drain: None,
        });

        // 主线程 drain ConnEvent，更新连接状态 / 弹出待处理请求。
        let weak = entity.downgrade();
        let drain = cx.spawn(async move |cx| {
            while let Ok(ev) = event_rx.recv().await {
                let ok = weak.update(cx, |this, cx| {
                    match ev {
                        ConnEvent::Connected => this.state = ConnState::Connected,
                        ConnEvent::Error(e) => this.state = ConnState::Error(e),
                        ConnEvent::Closed => this.state = ConnState::Closed,
                        ConnEvent::NeedHostKey {
                            host,
                            port,
                            key_type,
                            fingerprint,
                            changed,
                            reply,
                        } => {
                            this.pending_prompt = Some(PendingPrompt::HostKey {
                                host,
                                port,
                                key_type,
                                fingerprint,
                                changed,
                                reply: Some(reply),
                            });
                        }
                        ConnEvent::NeedCredential {
                            kind,
                            prompt,
                            reply,
                        } => {
                            this.pending_prompt = Some(PendingPrompt::Credential {
                                kind,
                                prompt,
                                reply: Some(reply),
                            });
                        }
                    }
                    cx.notify();
                });
                if ok.is_err() {
                    break;
                }
            }
        });
        entity.update(cx, |this, _cx| this._drain = Some(drain));
        entity
    }

    /// 请求开一个终端 channel，返回 per-terminal 桥接。
    /// 同步返回：命令排队等连接 Ready 后处理；调用方 drain `event_rx` 即可。
    pub fn open_terminal(
        &self,
        cols: u16,
        rows: u16,
    ) -> (Sender<InputCmd>, Receiver<SessionEvent>) {
        // 鼠标移动和复杂 TUI 的按键可能在一帧内产生大量小输入；容量太小会让
        // UI 侧的非阻塞发送丢事件。TerminalView 仍会在满载时保留待发队列。
        let (input_tx, input_rx) = async_channel::bounded::<InputCmd>(1024);
        // 高刷新率 TUI 会连续产生许多小块输出；稍大的队列可吸收一帧内的
        // 输出突发，UI drain 会按批次消费，避免远端读循环被短暂反压。
        let (event_tx, event_rx) = async_channel::bounded::<SessionEvent>(256);
        let _ = self.cmd_tx.try_send(ConnCmd::OpenTerminal {
            cols,
            rows,
            input_rx,
            event_tx,
        });
        (input_tx, event_rx)
    }

    /// 主动断开。
    #[allow(dead_code)]
    pub fn shutdown(&self) {
        let _ = self.cmd_tx.try_send(ConnCmd::Shutdown);
    }

    /// 请求开一个 SFTP 工作器，返回 per-pane 桥接。
    pub fn open_sftp(&self) -> (Sender<SftpCmd>, Receiver<SftpEvent>) {
        let (cmd_tx, cmd_rx) = async_channel::bounded::<SftpCmd>(64);
        let (event_tx, event_rx) = async_channel::bounded::<SftpEvent>(64);
        let _ = self.cmd_tx.try_send(ConnCmd::OpenSftp { cmd_rx, event_tx });
        (cmd_tx, event_rx)
    }

    /// 启动一条端口转发。返回后台回执（Ok 或 Err 字符串）。
    pub fn start_forward(
        &self,
        spec: crate::config::ForwardSpec,
        kind: ForwardKind,
    ) -> oneshot::Receiver<Result<(), String>> {
        let (tx, rx) = oneshot::channel();
        let _ = self.cmd_tx.try_send(ConnCmd::StartForward {
            spec,
            kind,
            reply: tx,
        });
        rx
    }

    /// 停止一条端口转发。
    pub fn stop_forward(&self, spec: crate::config::ForwardSpec, kind: ForwardKind) {
        let _ = self.cmd_tx.try_send(ConnCmd::StopForward { spec, kind });
    }

    /// UI 回传主机密钥决定并清空待处理请求。
    pub fn resolve_host_key(&mut self, decision: HostKeyDecision) {
        if let Some(PendingPrompt::HostKey { reply, .. }) = &mut self.pending_prompt {
            if let Some(tx) = reply.take() {
                let _ = tx.send(decision);
            }
        }
        self.pending_prompt = None;
    }

    /// UI 回传凭据（None = 取消）并清空待处理请求。
    pub fn resolve_credential(&mut self, value: Option<String>) {
        if let Some(PendingPrompt::Credential { reply, .. }) = &mut self.pending_prompt {
            if let Some(tx) = reply.take() {
                let _ = tx.send(value);
            }
        }
        self.pending_prompt = None;
    }
}

/// 后台连接任务主体。
async fn run_connection(
    host: HostConfig,
    methods: Vec<AuthChoice>,
    ssh_config: Arc<crate::config::SshConfig>,
    cmd_rx: Receiver<ConnCmd>,
    event_tx: Sender<ConnEvent>,
) -> anyhow::Result<()> {
    let remote_registry = new_remote_forward_registry();
    let (handle, jump_handle) = connect_and_authenticate(
        &host,
        &methods,
        &event_tx,
        remote_registry.clone(),
        ssh_config,
    )
    .await?;
    let handle = Arc::new(handle);
    // 跳板 Handle 需保活；持有到连接结束。
    let _jump_keepalive = jump_handle;

    // 认证完成 → 通知 UI 可开 channel。
    let _ = event_tx.send(ConnEvent::Connected).await;

    // channel 服务循环：终端/SFTP/转发计数；全部关闭即断开。
    let (ended_tx, mut ended_rx) = tokio_mpsc::unbounded_channel::<()>();
    let mut active: usize = 0;
    let mut ever_opened = false;
    // 活动转发的停止信号 / -R 分配端口。
    let mut fw_state: std::collections::HashMap<
        (ForwardKind, crate::config::ForwardSpec),
        ForwardState,
    > = std::collections::HashMap::new();

    let result: anyhow::Result<()> = loop {
        tokio::select! {
            biased;
            _ = ended_rx.recv() => {
                active = active.saturating_sub(1);
                if ever_opened && active == 0 {
                    break Ok(());
                }
            }
            cmd = cmd_rx.recv() => match cmd {
                Ok(ConnCmd::OpenTerminal { cols, rows, input_rx, event_tx: term_tx }) => {
                    let term_tx_err = term_tx.clone();
                    match open_terminal_channel(&handle, cols, rows, input_rx, term_tx, ended_tx.clone()).await {
                        Ok(()) => {
                            active += 1;
                            ever_opened = true;
                        }
                        Err(e) => {
                            log::warn!("open terminal channel failed: {e}");
                            let _ = term_tx_err.send(SessionEvent::Error(e.to_string())).await;
                            let _ = term_tx_err.send(SessionEvent::Closed).await;
                        }
                    }
                }
                Ok(ConnCmd::OpenSftp { cmd_rx, event_tx }) => {
                    match open_sftp_session(&handle).await {
                        Ok(sftp) => {
                            let ended_tx2 = ended_tx.clone();
                            tokio::spawn(async move {
                                run_sftp_worker(sftp, cmd_rx, event_tx).await;
                                let _ = ended_tx2.send(());
                            });
                            active += 1;
                            ever_opened = true;
                        }
                        Err(e) => {
                            log::warn!("open sftp channel failed: {e}");
                            let _ = event_tx.send(SftpEvent::Error(e.to_string())).await;
                            let _ = event_tx.send(SftpEvent::Closed).await;
                        }
                    }
                }
                Ok(ConnCmd::StartForward { spec, kind, reply }) => {
                    let res: Result<(), String> = match kind {
                        ForwardKind::Local | ForwardKind::Dynamic => {
                            let (stop_tx, stop_rx) = oneshot::channel();
                            let h = handle.clone();
                            let spec2 = spec.clone();
                            let ended_tx2 = ended_tx.clone();
                            tokio::spawn(async move {
                                let task = if kind == ForwardKind::Local {
                                    tokio::spawn(async move { let _ = run_local_forward(h, spec2, stop_rx).await; })
                                } else {
                                    tokio::spawn(async move { let _ = run_dynamic_forward(h, spec2, stop_rx).await; })
                                };
                                let _ = task.await;
                                let _ = ended_tx2.send(());
                            });
                            fw_state.insert((kind, spec.clone()), ForwardState { stop: Some(stop_tx), allocated: None });
                            active += 1;
                            ever_opened = true;
                            Ok(())
                        }
                        ForwardKind::Remote => {
                            match start_remote_forward(&handle, remote_registry.clone(), spec.clone()).await {
                                Ok(allocated) => {
                                    fw_state.insert((kind, spec.clone()), ForwardState { stop: None, allocated: Some(allocated) });
                                    active += 1;
                                    ever_opened = true;
                                    Ok(())
                                }
                                Err(e) => Err(e.to_string()),
                            }
                        }
                    };
                    let _ = reply.send(res);
                }
                Ok(ConnCmd::StopForward { spec, kind }) => {
                    if let Some(state) = fw_state.remove(&(kind, spec.clone())) {
                        match kind {
                            ForwardKind::Local | ForwardKind::Dynamic => {
                                if let Some(s) = state.stop { let _ = s.send(()); }
                                // listener 任务结束会发 ended → active--。
                            }
                            ForwardKind::Remote => {
                                if let Some(alloc) = state.allocated {
                                    stop_remote_forward(&handle, remote_registry.clone(), &spec, alloc).await;
                                }
                                // -R 无后台任务，直接 active--。
                                active = active.saturating_sub(1);
                            }
                        }
                    }
                }
                Ok(ConnCmd::Shutdown) => break Ok(()),
                Err(_) => {
                    // gpui 释放了 Connection；等残留 channel 收尾或立即断开。
                    if active == 0 {
                        break Ok(());
                    }
                }
            }
        }
    };

    let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;
    result
}

/// 一条活动转发的本地状态。
struct ForwardState {
    /// -L/-D 的停止信号。
    stop: Option<oneshot::Sender<()>>,
    /// -R 的服务端分配端口。
    allocated: Option<u32>,
}

/// connect（含反应式主机密钥确认）+ 认证（直连；不含 ProxyJump）。
pub(crate) async fn connect_direct_and_auth(
    host: &HostConfig,
    methods: Vec<AuthChoice>,
    event_tx: &Sender<ConnEvent>,
    remote_registry: RemoteForwardRegistry,
) -> anyhow::Result<Handle<ClientHandler>> {
    let addr = host.effective_host().to_string();
    let port = host.effective_port();

    let config = Arc::new(client::Config::default());
    let handler = ClientHandler::new(addr.clone(), port, event_tx.clone(), remote_registry);
    let sock_addr = (addr.as_str(), port);
    let mut handle: Handle<ClientHandler> = client::connect(config, sock_addr, handler).await?;

    let user = default_user_for(host);
    if !authenticate(&mut handle, &methods, &user, event_tx).await? {
        anyhow::bail!("authentication failed: all methods exhausted");
    }

    Ok(handle)
}

/// connect + 认证；支持单层 ProxyJump（经跳板 direct_tcpip + connect_stream）。
/// 返回目标 Handle 与需保活的跳板 Handle（无跳板时为 None）。
async fn connect_and_authenticate(
    host: &HostConfig,
    methods: &[AuthChoice],
    event_tx: &Sender<ConnEvent>,
    remote_registry: RemoteForwardRegistry,
    ssh_config: Arc<crate::config::SshConfig>,
) -> anyhow::Result<(Handle<ClientHandler>, Option<Handle<ClientHandler>>)> {
    match host.proxy_jump.as_deref().filter(|s| !s.is_empty()) {
        Some(jump_alias) => {
            // 跳板在 open_target_via_jump 内部已认证；返回的 target 未认证，需在此 auth。
            let target_host = host.effective_host().to_string();
            let target_port = host.effective_port();
            let (mut handle, jump_handle) = super::proxyjump::open_target_via_jump(
                ssh_config,
                jump_alias.to_string(),
                target_host,
                target_port,
                event_tx,
                remote_registry,
            )
            .await?;
            let user = default_user_for(host);
            if !authenticate(&mut handle, methods, &user, event_tx).await? {
                anyhow::bail!("authentication failed: all methods exhausted");
            }
            Ok((handle, Some(jump_handle)))
        }
        _ => {
            // 直连：connect_direct_and_auth 已完成认证。
            let handle =
                connect_direct_and_auth(host, methods.to_vec(), event_tx, remote_registry).await?;
            Ok((handle, None))
        }
    }
}

/// 认证主循环：先试 agent/无口令密钥；加密密钥→索要口令；全部失败→索要密码。
async fn authenticate(
    handle: &mut Handle<ClientHandler>,
    methods: &[AuthChoice],
    user: &str,
    event_tx: &Sender<ConnEvent>,
) -> anyhow::Result<bool> {
    let default_user = user.to_string();

    for method in methods {
        match method {
            AuthChoice::Key { user, path, .. } => {
                // 先用空口令加载；加密则向 UI 索要口令（最多重试 1 次）。
                match keys::load_secret_key(path, None) {
                    Ok(key) => {
                        if auth_with_key(handle, user, key).await? {
                            return Ok(true);
                        }
                    }
                    Err(keys::Error::KeyIsEncrypted) => {
                        let prompt = format!("Passphrase for {}:", path.display());
                        if let Some(pass) =
                            request_credential(event_tx, CredentialKind::Passphrase, prompt).await
                        {
                            match keys::load_secret_key(path, Some(&pass)) {
                                Ok(key) => {
                                    if auth_with_key(handle, user, key).await? {
                                        return Ok(true);
                                    }
                                }
                                Err(_) => {
                                    // 口令错：再给一次机会。
                                    if let Some(pass2) = request_credential(
                                        event_tx,
                                        CredentialKind::Passphrase,
                                        "Wrong passphrase, retry:".into(),
                                    )
                                    .await
                                    {
                                        if let Ok(key) = keys::load_secret_key(path, Some(&pass2)) {
                                            if auth_with_key(handle, user, key).await? {
                                                return Ok(true);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => log::info!("load key {} failed: {e}", path.display()),
                }
            }
            AuthChoice::Agent { user } => {
                if auth_agent(handle, user).await? {
                    return Ok(true);
                }
            }
            AuthChoice::Password { user, password } => {
                if handle
                    .authenticate_password(user.clone(), password.clone())
                    .await?
                    .success()
                {
                    return Ok(true);
                }
            }
        }
    }

    // 兜底：向 UI 索要密码（用户可取消）。
    let prompt = format!("Password for {default_user}:");
    if let Some(pass) = request_credential(event_tx, CredentialKind::Password, prompt).await {
        if handle
            .authenticate_password(default_user.clone(), pass)
            .await?
            .success()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 用已加载的私钥做 publickey 认证。
async fn auth_with_key(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    key: PrivateKey,
) -> anyhow::Result<bool> {
    let key_with_hash =
        keys::PrivateKeyWithHashAlg::new(Arc::new(key), Some(russh::keys::HashAlg::Sha256));
    Ok(handle
        .authenticate_publickey(user.to_string(), key_with_hash)
        .await?
        .success())
}

/// 向 UI 请求凭据；UI 不响应或取消返回 None（带超时兜底，避免永久挂起）。
async fn request_credential(
    event_tx: &Sender<ConnEvent>,
    kind: CredentialKind,
    prompt: String,
) -> Option<String> {
    let (tx, rx) = oneshot::channel();
    if event_tx
        .send(ConnEvent::NeedCredential {
            kind,
            prompt,
            reply: tx,
        })
        .await
        .is_ok()
    {
        match tokio::time::timeout(Duration::from_secs(300), rx).await {
            Ok(Ok(v)) => v,
            _ => None,
        }
    } else {
        None
    }
}

/// 开一个 SFTP 会话：session channel + sftp 子系统 + 转 stream + 握手。
async fn open_sftp_session(
    handle: &Handle<ClientHandler>,
) -> anyhow::Result<russh_sftp::client::SftpSession> {
    let channel = handle.channel_open_session().await?;
    channel.request_subsystem(true, "sftp").await?;
    let stream = channel.into_stream();
    let sftp = russh_sftp::client::SftpSession::new(stream).await?;
    Ok(sftp)
}

/// 开一个终端 channel：PTY + shell，然后派发独立 relay 任务。
async fn open_terminal_channel(
    handle: &Handle<ClientHandler>,
    cols: u16,
    rows: u16,
    input_rx: Receiver<InputCmd>,
    term_event_tx: Sender<SessionEvent>,
    ended_tx: tokio_mpsc::UnboundedSender<()>,
) -> anyhow::Result<()> {
    let channel = handle.channel_open_session().await?;
    channel
        .request_pty(false, "xterm-256color", cols as u32, rows as u32, 0, 0, &[])
        .await?;
    channel.request_shell(false).await?;

    let _ = term_event_tx.send(SessionEvent::Connected).await;

    let (read_half, write_half) = channel.split();
    tokio::spawn(async move {
        relay_terminal(read_half, write_half, input_rx, term_event_tx).await;
        let _ = ended_tx.send(());
    });
    Ok(())
}

/// 终端 channel relay：读循环转发字节，写循环消费输入。
async fn relay_terminal(
    mut read_half: ChannelReadHalf,
    write_half: ChannelWriteHalf<client::Msg>,
    input_rx: Receiver<InputCmd>,
    term_event_tx: Sender<SessionEvent>,
) {
    let (write_done_tx, mut write_done_rx) = tokio_mpsc::channel::<()>(1);
    let write_task = {
        let term_event_tx = term_event_tx.clone();
        tokio::spawn(async move {
            let result = drive_input(input_rx, write_half).await;
            let _ = write_done_tx.send(()).await;
            if let Err(e) = result {
                let _ = term_event_tx.send(SessionEvent::Error(e.to_string())).await;
            }
        })
    };

    loop {
        tokio::select! {
            biased;
            _ = write_done_rx.recv() => break,
            msg = read_half.wait() => match msg {
                Some(ChannelMsg::Data { data }) => {
                    let _ = term_event_tx.send(SessionEvent::Output(data.to_vec())).await;
                }
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    let _ = term_event_tx.send(SessionEvent::Output(data.to_vec())).await;
                }
                Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                _ => {}
            }
        }
    }

    let _ = write_task.await;
    let _ = term_event_tx.send(SessionEvent::Closed).await;
}

/// 输入驱动：InputCmd → write_half。
async fn drive_input(
    input_rx: Receiver<InputCmd>,
    write_half: ChannelWriteHalf<client::Msg>,
) -> anyhow::Result<()> {
    let wh = write_half;
    while let Ok(cmd) = input_rx.recv().await {
        match cmd {
            InputCmd::Write(bytes) => {
                wh.data_bytes(bytes).await?;
            }
            InputCmd::Resize { cols, rows } => {
                wh.window_change(cols as u32, rows as u32, 0, 0).await?;
            }
            InputCmd::Close => {
                break;
            }
        }
    }
    Ok(())
}

/// 尝试用 ssh-agent 上的每个身份认证，成功即返回。
async fn auth_agent(handle: &mut Handle<ClientHandler>, user: &str) -> anyhow::Result<bool> {
    let mut agent = keys::agent::client::AgentClient::connect_env().await?;
    let identities = agent.request_identities().await?;
    for id in identities {
        let result = handle
            .authenticate_publickey_with(
                user.to_string(),
                id.public_key().into_owned(),
                Some(russh::keys::HashAlg::Sha256),
                &mut agent,
            )
            .await?;
        if result.success() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// russh client Handler：反应式主机密钥确认 + -R 入站转发路由。
pub(crate) struct ClientHandler {
    host: String,
    port: u16,
    event_tx: Sender<ConnEvent>,
    forwards: RemoteForwardRegistry,
}

impl ClientHandler {
    pub(crate) fn new(
        host: String,
        port: u16,
        event_tx: Sender<ConnEvent>,
        forwards: RemoteForwardRegistry,
    ) -> Self {
        Self {
            host,
            port,
            event_tx,
            forwards,
        }
    }

    /// 向 UI 询问主机密钥决定；UI 不响应/超时视为拒绝。
    async fn ask_host_key(
        &self,
        server_public_key: &russh::keys::ssh_key::PublicKey,
        changed: bool,
    ) -> HostKeyDecision {
        let (tx, rx) = oneshot::channel();
        let fp = server_public_key
            .fingerprint(russh::keys::HashAlg::Sha256)
            .to_string();
        let kt = server_public_key.algorithm().as_str().to_string();
        let _ = self
            .event_tx
            .send(ConnEvent::NeedHostKey {
                host: self.host.clone(),
                port: self.port,
                key_type: kt,
                fingerprint: fp,
                changed,
                reply: tx,
            })
            .await;
        match tokio::time::timeout(Duration::from_secs(300), rx).await {
            Ok(Ok(d)) => d,
            _ => HostKeyDecision::Reject,
        }
    }
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        match keys::check_known_hosts(&self.host, self.port, server_public_key) {
            Ok(true) => Ok(true),
            Ok(false) => {
                // 未知主机：弹确认；AcceptOnce/AcceptAlways 接受，Always 写 known_hosts。
                match self.ask_host_key(server_public_key, false).await {
                    HostKeyDecision::AcceptOnce => Ok(true),
                    HostKeyDecision::AcceptAlways => {
                        if let Err(e) = keys::known_hosts::learn_known_hosts(
                            &self.host,
                            self.port,
                            server_public_key,
                        ) {
                            log::warn!("failed to write known_hosts: {e}");
                        }
                        Ok(true)
                    }
                    HostKeyDecision::Reject => Ok(false),
                }
            }
            Err(e) => {
                // 已知但密钥变更（可能 MITM）：告知 UI 后拒绝（见计划）。
                log::error!("host key changed for {}:{}: {}", self.host, self.port, e);
                let _ = self.ask_host_key(server_public_key, true).await;
                Ok(false)
            }
        }
    }

    /// -R 入站转发：服务端转来一条 channel → 查注册表 → relay 到本地目标。
    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<russh::client::Msg>,
        _connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        let registry = self.forwards.clone();
        let registered = self.forwards.lock().await.contains_key(&connected_port);
        if !registered {
            let _ = reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        }
        reply.accept().await;
        tokio::spawn(async move {
            handle_forwarded_tcpip(channel, connected_port, &registry).await;
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SshConfig;
    use crate::ssh::runtime::runtime;
    use crate::ssh::session::default_auth_for;
    use std::time::Duration;

    /// 端到端连接验证（需真实主机）。用 `CROSSH_TEST_HOST` 指定目标，默认 txvps。
    /// 运行：`cargo test -- --ignored --nocapture connect_real_host`
    #[test]
    #[ignore]
    fn connect_real_host() {
        let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
            .try_init();
        let target = std::env::var("CROSSH_TEST_HOST").unwrap_or_else(|_| "txvps".to_string());
        let cfg = Arc::new(SshConfig::from_default_location().unwrap());
        let host = cfg.resolve(&target);
        let methods = default_auth_for(&host);
        eprintln!("[test] target={target}, methods={methods:?}");
        assert!(!methods.is_empty(), "no auth methods discovered");

        let (cmd_tx, cmd_rx) = async_channel::bounded::<ConnCmd>(64);
        let (conn_event_tx, conn_event_rx) = async_channel::bounded::<ConnEvent>(64);
        let cfg_for_task = cfg.clone();
        runtime().spawn(async move {
            let _ = run_connection(host, methods, cfg_for_task, cmd_rx, conn_event_tx).await;
        });

        // 测试充当 UI：自动应答主机密钥(AcceptOnce) 与凭据(无需，因 txvps 用无口令密钥)。
        let (input_tx, event_rx) = async_channel::bounded::<InputCmd>(64);
        let (term_event_tx, term_event_rx) = async_channel::bounded::<SessionEvent>(64);
        cmd_tx
            .try_send(ConnCmd::OpenTerminal {
                cols: 80,
                rows: 24,
                input_rx: event_rx,
                event_tx: term_event_tx,
            })
            .unwrap();

        let rt = runtime();
        let (connected, sample) = rt.block_on(async move {
            let mut connected = false;
            let mut sample = String::new();
            let timer = tokio::time::sleep(Duration::from_secs(12));
            tokio::pin!(timer);
            loop {
                tokio::select! {
                    biased;
                    _ = &mut timer => break,
                    cev = conn_event_rx.recv() => {
                        // 自动应答交互请求。
                        if let Ok(ConnEvent::NeedHostKey { reply, .. }) = cev {
                            let _ = reply.send(HostKeyDecision::AcceptOnce);
                        }
                        // NeedCredential：测试不做应答（依赖无口令密钥）；超时后后台放弃该方式。
                    }
                    tev = term_event_rx.recv() => match tev {
                        Ok(SessionEvent::Connected) => connected = true,
                        Ok(SessionEvent::Output(b)) => {
                            sample.push_str(&String::from_utf8_lossy(&b));
                            if connected && sample.len() > 20 { break; }
                        }
                        Ok(SessionEvent::Cwd(_)) => {}
                        Ok(SessionEvent::Error(e)) => {
                            eprintln!("[test] error: {e}");
                            break;
                        }
                        Ok(SessionEvent::Closed) => break,
                        Err(_) => break,
                    }
                }
            }
            drop(input_tx);
            (connected, sample)
        });
        drop(cmd_tx);

        let preview: String = sample.chars().take(160).collect();
        eprintln!("[test] connected={}, preview={:?}", connected, preview);
        assert!(connected, "failed to connect/authenticate to {target}");
    }

    /// 诊断用：连接真实主机，开终端，等提示符出现后发送 `ls\r`，
    /// 把完整字节流（转义序列可视化）打印出来，用于排查「命令输出丢失」类问题。
    ///
    /// 运行：`cargo test -- --ignored --nocapture connect_and_run_ls`
    #[test]
    #[ignore]
    fn connect_and_run_ls() {
        let _ =
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("debug"))
                .try_init();
        let target = std::env::var("CROSSH_TEST_HOST").unwrap_or_else(|_| "txvps".to_string());
        let cfg = Arc::new(SshConfig::from_default_location().unwrap());
        let host = cfg.resolve(&target);
        let methods = default_auth_for(&host);

        let (cmd_tx, cmd_rx) = async_channel::bounded::<ConnCmd>(64);
        let (conn_event_tx, conn_event_rx) = async_channel::bounded::<ConnEvent>(64);
        let cfg_for_task = cfg.clone();
        runtime().spawn(async move {
            let _ = run_connection(host, methods, cfg_for_task, cmd_rx, conn_event_tx).await;
        });

        let (input_tx, event_rx) = async_channel::bounded::<InputCmd>(64);
        let (term_event_tx, term_event_rx) = async_channel::bounded::<SessionEvent>(64);
        cmd_tx
            .try_send(ConnCmd::OpenTerminal {
                cols: 80,
                rows: 24,
                input_rx: event_rx,
                event_tx: term_event_tx,
            })
            .unwrap();

        let rt = runtime();
        let output = rt.block_on(async move {
            let mut connected = false;
            let mut all: Vec<u8> = Vec::new();
            let mut sent = false;
            let timer = tokio::time::sleep(Duration::from_secs(10));
            tokio::pin!(timer);
            loop {
                tokio::select! {
                    biased;
                    _ = &mut timer => break,
                    cev = conn_event_rx.recv() => {
                        if let Ok(ConnEvent::NeedHostKey { reply, .. }) = cev {
                            let _ = reply.send(HostKeyDecision::AcceptOnce);
                        }
                    }
                    tev = term_event_rx.recv() => match tev {
                        Ok(SessionEvent::Connected) => connected = true,
                        Ok(SessionEvent::Output(b)) => {
                            all.extend_from_slice(&b);
                            // 收到提示符后发送 ls。
                            if connected && !sent {
                                let s = String::from_utf8_lossy(&all);
                                if s.contains("$ ") || s.contains("# ") {
                                    sent = true;
                                    let _ = input_tx
                                        .send(InputCmd::Write(b"ls\r".to_vec()))
                                        .await;
                                }
                            }
                        }
                        Ok(SessionEvent::Cwd(_)) => {}
                        Ok(SessionEvent::Error(e)) => {
                            eprintln!("[test] err: {e}");
                            break;
                        }
                        Ok(SessionEvent::Closed) => break,
                        Err(_) => break,
                    }
                }
            }
            drop(input_tx);
            all
        });
        drop(cmd_tx);

        // 转义序列可视化：ESC 显式标出，方便看清屏/光标类指令。
        let readable: String = output
            .iter()
            .map(|&byte| match byte {
                b'\r' => "\\r".into(),
                b'\n' => "\\n\n".into(),
                0x1b => "<ESC>".into(),
                0x20..=0x7e => (byte as char).to_string(),
                _ => format!("\\x{:02x}", byte),
            })
            .collect();
        eprintln!("[test] full output ({} bytes):\n{}", output.len(), readable);
    }
}

//! Pure SSH connection engine: one authenticated session serving terminal,
//! command, SFTP, and forwarding channels.
//!
//! The engine communicates through async channels. UI adapters may observe
//! `ConnEvent` and answer prompt events, but this module has no UI dependency.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_channel::{Receiver, Sender};
use russh::client::{self, Handle};
use russh::keys;
use russh::keys::ssh_key::PrivateKey;
use russh::{ChannelMsg, Disconnect, Sig};
use tokio::sync::mpsc as tokio_mpsc;
use tokio::sync::oneshot;

use crossh_core::config::HostConfig;

use super::forward::{
    RemoteForwardRegistry, handle_forwarded_tcpip, new_remote_forward_registry,
    run_dynamic_forward, run_local_forward, start_remote_forward, stop_remote_forward,
};
use super::runtime::runtime;
use super::session::{AuthChoice, default_user_for};
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

/// Commands sent to the connection task.
pub enum ConnCmd {
    /// Run a non-interactive command on the same authenticated SSH connection.
    OpenCommand {
        id: u64,
        command: String,
        cwd: String,
        event_tx: Sender<RemoteCommandEvent>,
    },
    /// Ask one remote command channel to terminate.
    StopCommand { id: u64 },
    /// 开一个 SFTP 工作器。worker 在连接的 sftp 子系统 channel 上运行。
    OpenSftp {
        cmd_rx: Receiver<SftpCmd>,
        event_tx: Sender<SftpEvent>,
    },
    /// 启动一条端口转发（-L/-R/-D）。
    StartForward {
        spec: crossh_core::config::ForwardSpec,
        kind: ForwardKind,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// 停止一条端口转发。
    StopForward {
        spec: crossh_core::config::ForwardSpec,
        kind: ForwardKind,
    },
    /// 主动断开。
    Shutdown,
}

/// 转发类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ForwardKind {
    Local,
    Remote,
    Dynamic,
}

/// Connection events emitted by the background task.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteCommandStatus {
    Succeeded,
    Failed,
    Terminated,
}

#[derive(Debug)]
pub struct RemoteCommandEvent {
    pub id: u64,
    pub status: RemoteCommandStatus,
    pub output: String,
    pub exit_code: Option<i32>,
}

/// Handle for issuing commands to a background SSH connection.
pub struct ConnectionHandle {
    cmd_tx: Sender<ConnCmd>,
    next_command_id: u64,
}

impl ConnectionHandle {
    /// Start the background connection task and return its command handle and
    /// event receiver. Prompt replies remain pure channel messages.
    pub fn start(
        host: HostConfig,
        methods: Vec<AuthChoice>,
        ssh_config: Arc<crossh_core::config::SshConfig>,
    ) -> (Self, Receiver<ConnEvent>) {
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

        (
            Self {
                cmd_tx,
                next_command_id: 1,
            },
            event_rx,
        )
    }

    pub fn open_command(
        &mut self,
        command: String,
        cwd: String,
    ) -> (u64, Receiver<RemoteCommandEvent>) {
        let id = self.next_command_id;
        self.next_command_id = self.next_command_id.saturating_add(1);
        let (event_tx, event_rx) = async_channel::bounded(1);
        if self
            .cmd_tx
            .try_send(ConnCmd::OpenCommand {
                id,
                command,
                cwd,
                event_tx: event_tx.clone(),
            })
            .is_err()
        {
            let _ = event_tx.try_send(RemoteCommandEvent {
                id,
                status: RemoteCommandStatus::Failed,
                output: "SSH connection is not available".into(),
                exit_code: None,
            });
        }
        (id, event_rx)
    }

    pub fn stop_command(&self, id: u64) {
        let _ = self.cmd_tx.try_send(ConnCmd::StopCommand { id });
    }

    /// Disconnect the background connection.
    pub fn shutdown(&self) {
        let _ = self.cmd_tx.try_send(ConnCmd::Shutdown);
    }

    /// Request an SFTP worker and return its bridge.
    pub fn open_sftp(&self) -> (Sender<SftpCmd>, Receiver<SftpEvent>) {
        let (cmd_tx, cmd_rx) = async_channel::bounded::<SftpCmd>(64);
        let (event_tx, event_rx) = async_channel::bounded::<SftpEvent>(64);
        let _ = self.cmd_tx.try_send(ConnCmd::OpenSftp { cmd_rx, event_tx });
        (cmd_tx, event_rx)
    }

    /// Start a port forward and return its asynchronous result.
    pub fn start_forward(
        &self,
        spec: crossh_core::config::ForwardSpec,
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

    /// Stop a port forward.
    pub fn stop_forward(&self, spec: crossh_core::config::ForwardSpec, kind: ForwardKind) {
        let _ = self.cmd_tx.try_send(ConnCmd::StopForward { spec, kind });
    }
}

/// 后台连接任务主体。
async fn run_connection(
    host: HostConfig,
    methods: Vec<AuthChoice>,
    ssh_config: Arc<crossh_core::config::SshConfig>,
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
    let (ended_tx, mut ended_rx) = tokio_mpsc::unbounded_channel::<ChannelEnded>();
    let mut active: usize = 0;
    let mut ever_opened = false;
    let mut remote_commands: HashMap<u64, oneshot::Sender<()>> = HashMap::new();
    // 活动转发的停止信号 / -R 分配端口。
    let mut fw_state: std::collections::HashMap<
        (ForwardKind, crossh_core::config::ForwardSpec),
        ForwardState,
    > = std::collections::HashMap::new();

    let result: anyhow::Result<()> = loop {
        tokio::select! {
            biased;
            Some(ended) = ended_rx.recv() => {
                if let ChannelEnded::RemoteCommand(id) = ended {
                    remote_commands.remove(&id);
                }
                active = active.saturating_sub(1);
                if ever_opened && active == 0 {
                    break Ok(());
                }
            }
            cmd = cmd_rx.recv() => match cmd {
                Ok(ConnCmd::OpenCommand { id, command, cwd, event_tx }) => {
                    let (stop_tx, stop_rx) = oneshot::channel();
                    remote_commands.insert(id, stop_tx);
                    let handle = handle.clone();
                    let ended_tx = ended_tx.clone();
                    tokio::spawn(async move {
                        let event = run_remote_command(&handle, id, command, cwd, stop_rx).await;
                        let _ = event_tx.send(event).await;
                        let _ = ended_tx.send(ChannelEnded::RemoteCommand(id));
                    });
                    active += 1;
                    ever_opened = true;
                }
                Ok(ConnCmd::StopCommand { id }) => {
                    if let Some(stop) = remote_commands.remove(&id) {
                        let _ = stop.send(());
                    }
                }
                Ok(ConnCmd::OpenSftp { cmd_rx, event_tx }) => {
                    match open_sftp_session(&handle).await {
                        Ok(sftp) => {
                            let ended_tx2 = ended_tx.clone();
                            tokio::spawn(async move {
                                run_sftp_worker(sftp, cmd_rx, event_tx).await;
                                let _ = ended_tx2.send(ChannelEnded::Regular);
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
                                let _ = ended_tx2.send(ChannelEnded::Regular);
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
                    // gpui 释放了 Connection：cmd 通道已关闭（队列满时
                    // Shutdown 可能根本没送达），任何命令都发不出去了。
                    // 先主动停掉全部残留 channel 再进入纯等待收尾，否则
                    // -R 转发的 active 只走 StopForward 递减（已不可达），
                    // 等待会没有上界。主动停止还顺带消除了 select 因 cmd
                    // 分支恒就绪而空转 CPU 的问题。
                    for (kind, spec) in fw_state.keys().cloned().collect::<Vec<_>>() {
                        if let Some(state) = fw_state.remove(&(kind, spec.clone())) {
                            match kind {
                                ForwardKind::Local | ForwardKind::Dynamic => {
                                    if let Some(stop) = state.stop {
                                        let _ = stop.send(());
                                    }
                                    // listener 任务结束会发 ended → active--。
                                }
                                ForwardKind::Remote => {
                                    if let Some(alloc) = state.allocated {
                                        stop_remote_forward(
                                            &handle,
                                            remote_registry.clone(),
                                            &spec,
                                            alloc,
                                        )
                                        .await;
                                    }
                                    // -R 无后台任务，直接 active--。
                                    active = active.saturating_sub(1);
                                }
                            }
                        }
                    }
                    // 丢弃 stop sender：远程命令任务侧的 stop_rx 立即返回并
                    // 关闭 channel，随即发送 ended（见 OpenCommand 的 wrapper）。
                    for (_, stop) in remote_commands.drain() {
                        drop(stop);
                    }
                    while let Some(ended) = ended_rx.recv().await {
                        if let ChannelEnded::RemoteCommand(id) = ended {
                            remote_commands.remove(&id);
                        }
                        active = active.saturating_sub(1);
                        if active == 0 {
                            break;
                        }
                    }
                    break Ok(());
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

enum ChannelEnded {
    Regular,
    RemoteCommand(u64),
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
    ssh_config: Arc<crossh_core::config::SshConfig>,
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
            AuthChoice::Key { user, path } => {
                // 先用空口令加载；加密则向 UI 索要口令（最多重试 1 次）。
                match keys::load_secret_key(path, None) {
                    Ok(key) => {
                        if auth_with_key(handle, user, key).await? {
                            return Ok(true);
                        }
                    }
                    Err(keys::Error::KeyIsEncrypted) => {
                        let prompt = format!("Passphrase for {}", path.display());
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
                                        "Incorrect passphrase; please try again".to_owned(),
                                    )
                                    .await
                                        && let Ok(key) = keys::load_secret_key(path, Some(&pass2))
                                        && auth_with_key(handle, user, key).await?
                                    {
                                        return Ok(true);
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
        }
    }

    // 兜底：向 UI 索要密码（用户可取消）。
    let prompt = format!("Password for {default_user}");
    if let Some(pass) = request_credential(event_tx, CredentialKind::Password, prompt).await
        && handle
            .authenticate_password(default_user.clone(), pass)
            .await?
            .success()
    {
        return Ok(true);
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

const MAX_REMOTE_COMMAND_OUTPUT: usize = 24 * 1024;

async fn run_remote_command(
    handle: &Handle<ClientHandler>,
    id: u64,
    command: String,
    cwd: String,
    mut stop_rx: oneshot::Receiver<()>,
) -> RemoteCommandEvent {
    let channel = match handle.channel_open_session().await {
        Ok(channel) => channel,
        Err(error) => {
            return RemoteCommandEvent {
                id,
                status: RemoteCommandStatus::Failed,
                output: error.to_string(),
                exit_code: None,
            };
        }
    };
    let remote_command = format!(
        "cd -- {} && exec sh -lc {}",
        shell_quote_remote(&cwd),
        shell_quote_remote(&command),
    );
    if let Err(error) = channel.exec(true, remote_command).await {
        return RemoteCommandEvent {
            id,
            status: RemoteCommandStatus::Failed,
            output: error.to_string(),
            exit_code: None,
        };
    }
    let (mut read_half, write_half) = channel.split();
    let mut output = String::new();
    let mut exit_code = None;
    let mut terminated = false;
    loop {
        tokio::select! {
            biased;
            _ = &mut stop_rx => {
                terminated = true;
                let _ = write_half.signal(Sig::TERM).await;
                let _ = write_half.close().await;
                break;
            }
            message = read_half.wait() => match message {
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                    append_remote_output(&mut output, &data);
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    exit_code = i32::try_from(exit_status).ok();
                }
                Some(ChannelMsg::ExitSignal { .. }) => {}
                Some(ChannelMsg::Close) | Some(ChannelMsg::Eof) | None => break,
                _ => {}
            }
        }
    }
    let status = if terminated {
        RemoteCommandStatus::Terminated
    } else if exit_code == Some(0) {
        RemoteCommandStatus::Succeeded
    } else {
        RemoteCommandStatus::Failed
    };
    RemoteCommandEvent {
        id,
        status,
        output,
        exit_code: if matches!(status, RemoteCommandStatus::Terminated) {
            None
        } else {
            exit_code
        },
    }
}

fn append_remote_output(output: &mut String, bytes: &[u8]) {
    output.push_str(&String::from_utf8_lossy(bytes));
    crossh_core::format::truncate_to_limit(output, MAX_REMOTE_COMMAND_OUTPUT);
}

fn shell_quote_remote(value: &str) -> String {
    shlex::try_quote(value)
        .map(|cow| cow.into_owned())
        .unwrap_or_else(|_| format!("'{}'", value.replace('\'', "'\\''")))
}

/// 尝试用 ssh-agent 上的每个身份认证，成功即返回。
#[cfg(unix)]
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

#[cfg(not(unix))]
async fn auth_agent(_handle: &mut Handle<ClientHandler>, _user: &str) -> anyhow::Result<bool> {
    // russh's environment-agent transport is Unix-only. Windows callers can
    // still select another authentication method, such as a password or key.
    log::debug!("ssh-agent authentication is unavailable on this platform");
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
                // 已知但密钥变更（可能 MITM）：允许「本次接受」以兼容服务器
                // 重装后的合法换钥，但绝不把变更后的密钥写入 known_hosts
                //（OpenSSH 同样要求手动处理变更密钥），因此 AcceptAlways
                // 在此路径等同拒绝。
                log::error!("host key changed for {}:{}: {}", self.host, self.port, e);
                match self.ask_host_key(server_public_key, true).await {
                    HostKeyDecision::AcceptOnce => Ok(true),
                    _ => Ok(false),
                }
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

    #[test]
    fn remote_shell_quote_preserves_command_text() {
        assert_eq!(shell_quote_remote("printf 'hello'"), "\"printf 'hello'\"");
        assert_eq!(shell_quote_remote("/srv/app"), "/srv/app");
        assert_eq!(shell_quote_remote("a b"), "'a b'");
        assert_eq!(shell_quote_remote(""), "''");
    }

    #[test]
    fn remote_command_output_keeps_the_newest_complete_utf8() {
        let mut output = "x".repeat(MAX_REMOTE_COMMAND_OUTPUT - 1);
        append_remote_output(&mut output, "中".as_bytes());
        assert!(output.is_char_boundary(0));
        assert!(output.len() <= MAX_REMOTE_COMMAND_OUTPUT);
        assert!(output.ends_with('中'));

        append_remote_output(&mut output, b"tail");
        assert!(output.len() <= MAX_REMOTE_COMMAND_OUTPUT);
        assert!(output.ends_with("tail"));
    }

    #[tokio::test]
    async fn spec_20260817_remove_auth_choice_password_password_fallback_roundtrip() {
        let (tx, rx) = async_channel::unbounded::<ConnEvent>();
        let task = tokio::spawn(async move {
            request_credential(&tx, CredentialKind::Password, "Password for alice".into()).await
        });

        let event = rx.recv().await.unwrap();
        let ConnEvent::NeedCredential {
            kind,
            prompt,
            reply,
        } = event
        else {
            panic!("expected NeedCredential, got a different ConnEvent");
        };
        assert_eq!(kind, CredentialKind::Password);
        assert_eq!(prompt, "Password for alice");

        reply.send(Some("s3cret".to_string())).unwrap();
        assert_eq!(task.await.unwrap(), Some("s3cret".to_string()));
    }

    #[tokio::test]
    async fn spec_20260817_remove_auth_choice_password_password_fallback_none_when_ui_unreachable()
    {
        let (tx, rx) = async_channel::unbounded::<ConnEvent>();
        drop(rx);
        assert_eq!(
            request_credential(&tx, CredentialKind::Password, "p".into()).await,
            None
        );
    }
}

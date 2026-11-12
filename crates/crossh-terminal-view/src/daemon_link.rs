//! daemon 接入链路：out channel 排空 + 服务端事件泵。
//!
//! - UI → pump：`LinkCmd`（输入/尺寸/结束），经 `DaemonLink::send`。
//! - pump → UI：`UiMsg`（字节/退出/元数据），由视图侧 gpui 任务转发进 view。
//! - `Drop` 只 detach（abort pump），绝不结束会话：关窗口/退出 GUI 不能杀
//!   会话（规格）；结束会话只走显式的 `LinkCmd::Close`。

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use crossh_session::{Client, ServerEvent};
use tokio::sync::mpsc;

/// 建链模式：新建交互会话，或接入既有会话（固定标签恢复）。
pub enum LinkMode {
    Create { cwd: PathBuf, cols: u16, rows: u16 },
    Attach { id: String },
}

/// UI → pump。输入走 Remote 终端的 out channel（按键/粘贴/鼠标统一漏斗），
/// 这里只剩尺寸与结束。
pub enum LinkCmd {
    Resize(u16, u16),
    Close,
}
/// pump → UI（视图侧 gpui 任务消费）。
pub enum UiMsg {
    /// 接入完成：会话 id + 初始输入权 + cwd。
    Attached {
        id: String,
        has_input: bool,
        cwd: Option<String>,
    },
    Bytes(Vec<u8>),
    /// 回放喂完（`ReplayEnd` 之后；空回放也发）：视图据此关闭回放抑制，
    /// 此后的查询应答恢复正常回写。见 `Terminal::replaying_remote`。
    ReplayDone,
    Exited,
    /// 轮询到的输入权/cwd 变化。
    Meta {
        has_input: bool,
        cwd: Option<String>,
    },
    /// 链路死亡（daemon 不可达/会话消失），携带原因。
    Dead(String),
}

pub struct DaemonLink {
    cmd_tx: mpsc::UnboundedSender<LinkCmd>,
    _task: tokio::task::JoinHandle<()>,
}

impl DaemonLink {
    pub fn spawn(
        mode: LinkMode,
        out_rx: async_channel::Receiver<Vec<u8>>,
        ui_tx: mpsc::UnboundedSender<UiMsg>,
    ) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let task = daemon_runtime().spawn(run_link(mode, out_rx, cmd_rx, ui_tx));
        Self {
            cmd_tx,
            _task: task,
        }
    }

    pub fn send(&self, cmd: LinkCmd) {
        let _ = self.cmd_tx.send(cmd);
    }
}

fn daemon_runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("daemon-link")
            .enable_all()
            .build()
            .expect("daemon link runtime")
    })
}

async fn run_link(
    mode: LinkMode,
    out_rx: async_channel::Receiver<Vec<u8>>,
    mut cmd_rx: mpsc::UnboundedReceiver<LinkCmd>,
    ui_tx: mpsc::UnboundedSender<UiMsg>,
) {
    let mut client = match Client::connect().await {
        Ok(client) => client,
        Err(error) => {
            let _ = ui_tx.send(UiMsg::Dead(format!("cannot reach daemon: {error}")));
            return;
        }
    };
    let me = client.client_id;
    let attached = match &mode {
        LinkMode::Create { cwd, cols, rows } => {
            client
                .create_shell(Some(cwd.to_string_lossy().into_owned()), *cols, *rows)
                .await
        }
        LinkMode::Attach { id } => client.attach(id).await,
    };
    let attached = match attached {
        Ok(attached) => attached,
        Err(error) => {
            let _ = ui_tx.send(UiMsg::Dead(format!("cannot attach session: {error}")));
            return;
        }
    };
    let id = attached.session.id.clone();
    let _ = ui_tx.send(UiMsg::Attached {
        id: id.clone(),
        has_input: attached.session.controller == Some(me),
        cwd: attached.session.cwd.clone(),
    });
    if !attached.replay.is_empty() {
        let _ = ui_tx.send(UiMsg::Bytes(attached.replay));
    }
    // 回放边界：空回放也发，视图的回放抑制据此关闭（只影响重放字节，
    // 此后的实时查询应答恢复正常回写）。
    let _ = ui_tx.send(UiMsg::ReplayDone);

    // 单任务 select：out channel 排空（Remote 出向 → daemon）、UI 命令、
    // 服务端下行、2s 元数据轮询。client 可变借用只属于本循环。
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
             incoming = out_rx.recv() => match incoming {
                 Ok(bytes) => {
                     log::debug!("gui-send {}B: {:02x?}", bytes.len(), &bytes[..bytes.len().min(16)]);
                     if client.input(&bytes).await.is_err() {
                         break;
                     }
                     // 发送即接管（乐观更新，2s 轮询会纠正）。
                     let _ = ui_tx.send(UiMsg::Meta { has_input: true, cwd: None });
                 }
                Err(_) => break,
            },
            cmd = cmd_rx.recv() => match cmd {
                Some(LinkCmd::Resize(cols, rows)) => {
                    let _ = client.resize(cols, rows).await;
                }
                 Some(LinkCmd::Close) => {
                     let _ = client.close(&id).await;
                     break;
                 }
                 // `cmd` 通道关闭 = 视图销毁（关窗口/退出 GUI）：只 detach，
                 // 不结束会话。结束会话只走显式的 `LinkCmd::Close`。
                 None => break,
            },
            event = client.next_event() => match event {
                Ok(ServerEvent::Output { bytes, .. }) => {
                    if ui_tx.send(UiMsg::Bytes(bytes)).is_err() {
                        break;
                    }
                }
                Ok(ServerEvent::Exited(_)) => {
                    let _ = ui_tx.send(UiMsg::Exited);
                    break;
                }
                Err(error) => {
                    let _ = ui_tx.send(UiMsg::Dead(format!("daemon link lost: {error}")));
                    break;
                }
            },
            _ = tick.tick() => {
                // 输入权/cwd 轮询：他人接管时点亮只读，cwd 变化时同步。
                if let Ok(sessions) = client.list().await
                    && let Some(session) = sessions.iter().find(|s| s.id == id)
                {
                    let _ = ui_tx.send(UiMsg::Meta {
                        has_input: session.controller == Some(me),
                        cwd: session.cwd.clone(),
                    });
                }
            }
        }
    }
}

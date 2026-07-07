# crossh 剩余功能实现计划（任务 3 补全 + 5/6/7 + ProxyJump + 调优）

本计划覆盖上一版总览计划（`1783348619272-ssh-client-gpui-plan.md`）中所有未完成项，按依赖顺序分阶段。每阶段自包含、可独立验证。硬约束不变：桌面专用、`~/.ssh/config` 只读真源、自研 UI 组件、常驻低内存。

## 当前基线（已实现，本计划在其上演进）
- `src/config/ssh_config.rs`：解析 `Host` 通配/`Include`/`ProxyJump`/`Local·Remote·DynamicForward`/`IdentitiesOnly`；`HostConfig` 含 `local_forwards/remote_forwards/dynamic_forwards: Vec<ForwardSpec>` 与 `proxy_jump: Option<String>`（已解析但未使用）。
- `src/ssh/runtime.rs`：单例 tokio Runtime，`worker_threads(2)`。
- `src/ssh/session.rs`：`AuthChoice{Agent,Key{path,passphrase},Password}`、`default_auth_for`（显式密钥→默认密钥→agent）、`InputCmd{Write,Resize,Close}`、`SessionEvent{Connected,Output,Error,Closed}`、`ClientHandler.check_server_key`（未知主机直接接受，TODO 弹窗）、`spawn_terminal_session`→`run_session`（connect+逐个 auth+PTY+shell+relay，全部耦合在一个函数里）。
- `src/ui/terminal_view.rs`：alacritty `Term`+vte，gpui canvas 渲染，主线程 drain `event_rx`，`SCROLLBACK=1000`。
- `src/ui/app_shell.rs`：`AppShell{ config, entries, active: Option<Entity<TerminalView>>, status }`，单终端，点主机即替换。

## 已敲定的设计决策
1. **共享会话池**：按 `(effective_host, user, port)` 开一条认证会话；终端/SFTP/转发各自从中取 channel；按 channel 引用计数，归 0 才 `disconnect`。这是任务 5/6/7 的骨架，避免重复认证与重复连接占内存。
2. **反应式凭据**：连接先用 agent/无口令密钥试；仅在加密密钥加载失败、或全部密钥/agent 被拒时，向 UI 发 `NeedCredential{Passphrase|Password}`，UI 弹框，结果经 `oneshot` 回传，会话重试。口令最多重试 1 次。不在日志打印凭据，用后 `zeroize`/drop。
3. **主机密钥反应式确认**：把回传通道注入 `ClientHandler`；未知主机发 `NeedHostKey{fingerprint}`，UI 弹「接受一次 / 总是 / 拒绝」，阻塞 `connect()` 直到用户决定。「总是」→ 追加写 `~/.ssh/known_hosts`（known_hosts ≠ config，不受只读约束）。密钥变更 → 弹「已变更/可能 MITM」并拒绝。
4. **工作区多标签**：`Workspace` 持 `Vec<Tab>`；`Tab{ host_alias, pane: Pane }`；`Pane = Terminal(TerminalView) | Sftp(SftpPane) | Forward(ForwardPane)`。sidebar 点击主机 = 新开一个 Terminal 标签（复用既有连接或新建）。
5. **SFTP 核心**：浏览（列表/进入目录/路径栏）、上传/下载单文件、目录递归、进度、覆盖确认。拖拽上传列为 stretch（本计划标注，不阻塞）。
6. **端口转发全量**：`-L`(local listener→`direct_tcpip`)、`-R`(`tcpip_forward` 请求 + Handler 路由入站 `ForwardedTcpIp`→本地目标)、`-D`(本地 SOCKS5 server)。config 驱动，UI 仅启停开关。
7. **ProxyJump 单层、spike-gated**：在跳板会话上 `direct_tcpip` 到 `(target,22)`，把该 channel 当流，在其上跑第二个 russh client。**先做 spike 验证 russh 0.62 能否用任意 `AsyncRead+AsyncWrite` 发起 client**（关键 API 待查：是否有 `connect_stream`/可从 `Channel` 取流）。不通过 → 该阶段降级为「标注后续」，不阻塞其余。
8. **资源调优**：worker 已 2、scrollback 已 1000；新增：测量空闲(无连接)与单连接单终端常驻内存基线并记录目标；验证断开后 runtime 无常驻任务/连接释放。

## 目标模块布局（新增/改动）
```
src/ssh/
  connection.rs   新增：Connection(gpui Entity) + run_connection 任务（connect+反应式 auth+host-key+channel 多路复用+refcount 生命周期）
  pool.rs         新增：ConnectionPool（Key→Shared<Connection>；acquire/open_terminal/open_sftp/open_forward）
  host_key.rs     新增：known_hosts 读(复用 keys::check_known_hosts)+「总是」追加写
  auth.rs         新增：try_auth 拆出 + NeedCredential 分类（加密密钥检测/口令重试）
  sftp.rs         新增：russh-sftp 包装（list/get/put/递归/进度事件）
  forward.rs      新增：-L/-R/-D 任务（listener/direct_tcpip/tcpip_forward/SOCKS5）
  proxyjump.rs    新增(阶段6)：嵌套连接 + spike
  session.rs      改动：终端 channel 桥接保留；connect/auth/host-key 逻辑上移到 connection.rs；保留 InputCmd/SessionEvent
  runtime.rs      不变
src/ui/
  workspace.rs    新增：Tab 容器 + 标签条(自研) + 连接状态徽标
  app_shell.rs    改动：active→Workspace；sidebar 点击改走 Workspace::open_terminal
  terminal_view.rs 改动：new 不再自连，改为从 Connection 取 terminal channel pair
  sftp_pane.rs    新增：浏览 + 传输列表/进度 + 覆盖弹窗
  forward_pane.rs 新增：ForwardSpec 开关列表
  prompt.rs       新增：密码/口令/主机密钥模态组件(自研)
```

## 阶段任务（按依赖顺序）

### 阶段 0 — 终端路径重构到 Connection（零行为变化，先去风险）
目的：把 `run_session` 里「connect+auth+host-key+PTY+shell+relay」拆开，让终端 channel 变成「向一条已认证会话申请 channel」，为复用打底。
- [0.1] 新增 `ssh/connection.rs`：`Connection`(gpui Entity) 持 `cmd_tx: Sender<ConnCmd>`、`event_rx: Receiver<ConnEvent>`、状态机 `Connecting|AuthNeeded|HostKeyNeeded|Ready|Failed|Closed`、attached panes 列表与 refcount。
- [0.2] 新增 `run_connection` 任务（runtime 上）：connect →（暂沿用现有逐个 auth 与 auto-accept host-key，先不接 UI）→ Ready 后循环处理 `ConnCmd::OpenTerminal{cols,rows} -> (InputCmd tx, SessionEvent rx)`；其余 `OpenSftp/OpenForward` 占位返回未实现。
- [0.3] `terminal_view.rs::new` 改为接收一个已 Ready 的 terminal channel pair（`Sender<InputCmd>, Receiver<SessionEvent>`），不再自己 `spawn_terminal_session`。drain 逻辑不变。
- [0.4] `app_shell.rs` 临时改为：点主机 → 建 `Connection` → Ready 后 `OpenTerminal` → 包成 `TerminalView`。行为与现版一致。
- 验证：`cargo build` 干净；`connect_real_host` 集成测试仍通过（txvps 出 live shell）；GUI 单终端行为不变。

### 阶段 1 — 反应式认证 + 主机密钥弹窗（完成任务 3）
- [1.1] `ConnEvent` 增 `NeedHostKey{ key_type, fingerprint_sha256 }`；`ConnCmd` 增 `HostKeyDecision{ AcceptOnce|AcceptAlways|Reject }`。`ClientHandler` 持一对回传 channel：未知/变更时发事件并 `await` 决定。
- [1.2] `ssh/host_key.rs`：`is_known(host,port,pubkey)->Result<bool>`（复用 `keys::check_known_hosts`）；`append_known(host,port,pubkey)`（构造 `[host]:port <type> <base64 ssh wire>` 行，原子追加到 `~/.ssh/known_hosts`，必要时先建文件 0600）。查准 `russh::keys::ssh_key::PublicKey` 的 SSH 序列化 API。
- [1.3] `ssh/auth.rs`：`try_auth` 上移；`Key{passphrase:None}` 加载失败时按错误分类「加密需口令」→ 发 `NeedCredential{Passphrase,path}`；口令错可重试 1 次。全部密钥/agent 被拒且服务器允许 password → 发 `NeedCredential{Password}`。
- [1.4] `ConnEvent` 增 `NeedCredential{kind}`；`ConnCmd` 增 `Credential{value}`（oneshot 回填）。`run_connection` 的 auth 循环改为可暂停-续跑。
- [1.5] `ui/prompt.rs`：自研三个模态（密码框、口令框、主机密钥指纹确认）。焦点接管、Esc=取消/拒绝、回车=提交。凭据用后清零。
- [1.6] Workspace/Connection 在主线程 drain `ConnEvent`：`NeedHostKey`/`NeedCredential` → 弹模态 → 结果回 `ConnCmd`。
- 验证：连加密私钥主机（弹口令框后成功）；连密码-only 主机（弹密码框后成功）；连未知主机（弹指纹框，选「总是」后 `~/.ssh/known_hosts` 出现新行，再连无弹窗）；密钥变更主机被拒。新增 `#[ignore]` 集成测试覆盖加密密钥路径。

### 阶段 2 — 多标签工作区 + 会话池生命周期（完成任务 5）
- [2.1] `ssh/pool.rs`：`ConnectionPool{ map: HashMap<Key, Entity<Connection>> }`；`acquire(host)->Entity<Connection>`（命中复用，否则新建并触发 connect）；`release`/refcount。Pool 作为 gpui Entity 或 App-level 全局。
- [2.2] `ui/workspace.rs`：`Vec<Tab>`、标签条（自研，可关闭/切换/拖序留 stretch）、活动标签索引；Tab 持 `Pane` 枚举。
- [2.3] sidebar 点击 → `pool.acquire(host)` → 在 Workspace 新开 Terminal Tab（同主机第二标签复用同连接，开新 channel）。
- [2.4] 关闭标签 → 该 pane 释放 channel → Connection refcount 减；归 0 → `disconnect` + 从 pool 移除。sidebar 显示连接状态徽标（idle/connecting/ready/error）。
- 验证：同时连两台主机各一标签；同主机开两标签共享单连接（日志只见一次 auth）；关闭所有标签后该主机断开、内存回落。

### 阶段 3 — SFTP 核心（完成任务 6）
- [3.1] `ssh/sftp.rs`：从 Connection 取 sftp channel（`russh-sftp::client::SftpSession`）；`list(path)`、`stat`、`mkdir -p`、流式 `get`/`put`（分块，按 chunk 发进度事件）、递归上传/下载目录。
- [3.2] `ConnCmd` 增 `OpenSftp -> (sftp 句柄/事件流)`；SftpPane 在主线程 drain 进度。
- [3.3] `ui/sftp_pane.rs`：本地/远程双栏或单远程栏（先单远程+本机文件选择器上传）；列表、进入目录、路径栏、上传按钮、下载按钮、传输队列+进度条、覆盖确认模态（复用 prompt 组件）。
- [3.4] stretch（标注不阻塞）：gpui drop 事件拖拽本地文件→上传。
- 验证：上传/下载单文件含进度；目录递归传输；覆盖时弹确认；大文件流式不整缓存（观察内存不爆涨）。

### 阶段 4 — 端口转发 -L/-R/-D（完成任务 7）
- [4.1] `ssh/forward.rs`：
  - `-L`：本地 `TcpListener`，每 accept → Connection 上 `channel_open_direct_tcpip(host:hport)` → 双向流式 relay 任务。
  - `-R`：Connection 发 `tcpip_forward(remote_bind, remote_port)`；`ClientHandler` 持「已注册远端转发表 (port→local_target)」，入站 `ForwardedTcpIp` channel → relay 到本地目标。
  - `-D`：本地 SOCKS5 server（自实现最小 SOCKS5 握手），每连接解析目标 → `direct_tcpip`。
- [4.2] `ConnCmd` 增 `StartForward{spec}/StopForward{spec}`；转发状态进 `ConnEvent`（已启动/失败/端口被占）。
- [4.3] `ui/forward_pane.rs`：列出该主机 config 的 `local/remote/dynamic_forwards`，每条一个开关；失败/端口占用明确提示，不自动换端口。
- 验证：`-L` 转发到远端 HTTP 服务能 `curl localhost:lport`；`-R` 远端能反连本地；`-D` 用 `curl --socks5` 走通；端口被占时报错。

### 阶段 5 — ProxyJump（spike-gated，可降级）
- [5.0] spike（先行）：验证 russh 0.62 能否用任意 `AsyncRead+AsyncWrite` 流发起 client（找 `connect_stream` 或把 `Channel` 的读写半转为流喂给 `client::connect`）。写一个 `#[ignore]` 测试：经跳板 `direct_tcpip` 到 target:22 并在其上完成第二次 SSH 握手+认证。
- [5.1] spike 通过 → `ssh/proxyjump.rs`：解析 `host.proxy_jump`（可能 `user@jumphost:port`）；先 `pool.acquire(jumphost)` 取跳板连接，在其上 `direct_tcpip(target,22)`，把 channel 当流跑第二条 russh client；仅支持单层；多层/解析失败降级提示。
- [5.2] spike 不通过 → 在本计划末尾「待办」段记录，ProxyJump 移出本计划，其余阶段不受影响。
- 验证：经跳板连到 target 出 live shell；多层/无效 ProxyJump 给明确报错。

### 阶段 6 — 资源调优与内存基线（完成任务 9）
- [6.1] 确认 `runtime()` 线程惰性（首次用才起）、断开后无遗留 tokio 任务（日志/任务计数核对）。
- [6.2] 测量：空闲（无连接）与「单连接单终端」常驻内存（`/usr/bin/time -l` RSS 或 mach `task_info`）；记录到本文件「基线」节。
- [6.3] 大输出 profile（`yes`/`cat 大文件`/`htop`）确认渲染无明显掉帧；scrollback 上限生效。
- 目标（写入基线节）：空闲 < 待测；单连接单终端相对空闲增量 < 待测（测得后填）。

## 风险与缓解
- **russh ProxyJump 嵌套**（最高）：阶段 5.0 spike 先行，不通过即降级，不阻塞。
- **反应式 auth 的错误分类**：russh-keys 加载错误需精确区分「加密需口令」与「格式不支持/文件不存在」；若 API 不易区分，回退为「任何 Key 加载失败都尝试一次口令」。
- **`ClientHandler` 阻塞等 UI**：`connect()` 期间持回传 channel 等用户决定，需保证 UI 取消/超时路径（Esc=拒绝）不会让任务永挂；加超时兜底。
- **会话池并发**：refcount/channel 注册在 runtime 与 gpui 两侧访问，统一经 channel 命令驱动，避免裸共享可变状态。
- **known_hosts 写格式**：需查准 `ssh_key::PublicKey` 的 wire/base64 编码 API；以 OpenSSH `ssh-keygen -F` 能查到为验收标准。
- **gpui 模态焦点/事件**：自研模态需正确接管键盘与点击透传；先做最简覆盖层，后续再打磨。

## 明确不在本计划范围
- `Match exec` / `ProxyCommand` / 多层 ProxyJump / GSSAPI / 智能卡。
- config 编辑 UI、自有主机库、连接/转发状态持久化与自动重连。
- 本地终端（local shell）、终端图像协议(Sixel/iTerm)、连字。
- SFTP 拖拽/批量/断点续传（stretch，标注不阻塞）。
- 多窗口。

## 基线（阶段 6 测得后填）
- 空闲 RSS：**~71.5 MB**（release，无连接；gpui + 代码基线，tokio runtime 惰性未起线程）。
- 单连接单终端 RSS：_未单独测（需 GUI 交互触发连接）_；增量预估 = russh 会话缓冲 + 终端滚动(上限 1000 行) + relay 任务，受 scrollback 上限约束。
- 调优项已落实：`worker_threads(2)`、`SCROLLBACK=1000`、tokio runtime 惰性初始化(`OnceLock`)、传输流式(32KB chunk)、关闭全部标签后连接 disconnect 并在下次 acquire 时替换池条目。


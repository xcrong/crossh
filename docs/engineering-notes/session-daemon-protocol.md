# 会话属主进程：协议帧、重连回放、出向通道

daemon（`crossh-session`）持 PTY、不持 VT 网格；前端各自解析字节流。
对照实现为 tty7（`docs/getting-started/concepts.mdx`："The GUI does not have
to be running at all"），帧格式照其切分：`u32` 小端长度 + 1 字节 kind，
一次 `write_vectored` 写全，socket 设 `TCP_NODELAY`。

## 帧与重放

- 帧：`u32` 小端长度 + 1 字节 kind + 载荷，`MAX_FRAME = 8 MiB`。
  超长/截断返回错误不 panic（`decode_frame` 单测覆盖），单帧只产生一次
  `write_vectored`（自定义 `Write` 计数断言）。
- 结构化载荷 `serde_json`，无 `deny_unknown_fields`：旧端忽略新字段。
- 输出环形缓冲 + 序列号；重连只发 `AttachRequest { id }`，服务端回放最近
  512 KiB 的环（`feed.rs`），`AttachedReply.end_seq` 标出回放末端，
  转发任务只继续发序号更大的实时块（`serve.rs`）。
- 能力标志表：每个标志文档注明"不支持它的对端怎么办"，未知标志不影响
  已解析公共字段（单测断言）。
- 握手：版本不一致给明确错误类型，文案含停止旧实例的具体命令；
  令牌缺失/错误拒绝连接（回环 TCP 从"开目录"升级为"可打字"后的安全面）。

## 出向通道划分

`write_to_pty` 是唯一漏斗：按键/鼠标/滚轮/焦点/`PtyWrite`/特性探测/
颜色应答全部经此。`TerminalType::Remote { tx }` 分支 `try_send` 进 channel
不阻塞按键；`TextAreaSizeRequest` 在 Remote 下禁回（尺寸权威在服务端，
本地格子回答远端查询会污染布局）；入向 `write_remote_output` 直达 VT，
不经 `convert_lf_to_crlf`（raw 模式程序语义）。

## 输入权与尺寸

- 单会话单 controller：第二个接入者是 observer，只读 badge；
  observer 发输入即接管（服务端换主 + 广播，新 512 KiB 回放上限内）。
- 尺寸：controller 上报（视图 paint 去重后发 Resize），observer 上报被
  服务端静默忽略。alt-screen 程序在 observer 窗口重排错位是已知 artefact，
  非回归。

## 重放抑制：旧查询不得重放回答

- 接入时重放环里的旧设备查询（OSC 11 查底色、`CSI ? mode $ p` 查模式、DA、特性探测）会被 VTE 重解析；
  不抑制的话应答经 `write_to_pty` 灌回 shell 当输入执行（`zsh: command not found`、认领旧 TUI 会话复现过）。
 - `Terminal::replaying_remote` 在 `Attached` 置 true、`ReplayDone` 置 false，
   同步翻转共享的 `reply_gate`（`Arc<AtomicBool>`）；
   抑制 `PtyWrite`/颜色应答/特性探测应答/剪贴板读写（标题/响铃不抑制：前者恢复标签后者瞬态）。
   实时查询不受影响（TUI 主题适配照常用）。
 - 门控必须在**发射时**（`ZedListener::send_event` 丢弃不进队列），不能只在消费侧
   （`process_event`）：`ReplayDone` 清旗跑在异步 drain 前面是竞态，旧查询应答照样漏出去。
   消费侧门控保留作纵深。
 - 回归：`remote_replay_suppresses_stale_query_replies`（消费侧）+
   `zed_listener_drops_replies_at_emit_time_while_replaying`（发射侧，清旗后消费也拿不回）。

## 工作区（协议 v2）

- daemon 是工作区的唯一属主：项目、tab、递归分屏树、比例与焦点都持久化在
  daemon，GUI 只消费 `WorkspaceSnapshot`，断线期间布局仍由 daemon 持有。
- 身份分层：`ProjectId`/`TabId`/`PaneId`/`SplitId` 稳定且落盘；`SessionId`
  只活在 daemon 进程内，首个客户端接入时物化活跃 tab 的 shell，其余 tab
  在激活时物化，daemon 重启后按 pane 重建会话。
- 消息：`WorkspaceGet`(17) 读快照；`WorkspaceAttach`(18) 先回当前快照再
  持续推送 `WorkspaceSnapshot`(20)；`WorkspaceMutate`(19) 的应答是
  `WorkspaceMutateReply`(21)。能力标志 `workspace-v1`，协议版本 v2，v1
  客户端或 daemon 在握手阶段被拒绝。
- 修改回执：请求可带 `expected_revision`，与当前 `revision` 不等直接回
  `Conflict`（`error` 文案含期望值与当前值）；其余结果分 `Applied`、
  `Unchanged`、`Rejected`。`durable` 标明本次结果是否已同步落盘；
  结构修改写入失败时回执 `durable=false` 并带 `storage_error`，异步写
  失败只反映在后续快照。
- 串行 actor：`WorkspaceActor` 独占 `Workspace` 与 `Runtime`，命令走单条
  队列；PTY 的 spawn/close/resize 作为 `Effect` 交给运行时任务，会话归属
  镜像只在 bind/unbind 时由 actor 写。结构修改在回执前完成原子写入与
  fsync，比例/焦点/cwd/尺寸 300 ms 合并后写。
- 持久化：`<config-root>/daemon/workspace.json`，写入顺序为 tmp →
  `sync_all` → 主文件转 `.bak` → rename → 目录同步；读取先主文件、再
  `.bak`，损坏文件改名为 `.corrupt-<ts>` / `.corrupt-bak-<ts>` 隔离；
  `format_version` 高于当前值时拒绝读写、不改写文件，并把原因尽力写入
  `<config-root>/daemon/daemon.error`（成功启动后删除）。

## 搜索关键词

`Remote`, `write_remote_output`, `write_vectored`, `TCP_NODELAY`,
`512 KiB`, `AttachedReply`, `end_seq`, `controller`, `observer`,
`TextAreaSizeRequest`, `convert_lf_to_crlf`, detached,
`replaying_remote`, `ReplayDone`, 重放抑制, 旧查询重放, `setsid`,
`DETACHED_PROCESS`, 重连回放, 输入权, 只读,
`WorkspaceSnapshot`, `WorkspaceActor`, `revision`, `expected_revision`,
`workspace-v1`, 协议 v2, 递归分屏, `durable`, `storage_error`,
`workspace.json`, `CROSSH_CONFIG_DIR`, `daemon.error`

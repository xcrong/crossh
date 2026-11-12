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
- 输出环形缓冲 + 序列号；重连带 `since_seq` 回放，最多 512 KiB，
  旧序列号回空快照不报错（`feed.rs`）。
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

## 搜索关键词

`Remote`, `write_remote_output`, `write_vectored`, `TCP_NODELAY`,
`since_seq`, `512 KiB`, `SessionAttached`, `controller`, `observer`,
`TextAreaSizeRequest`, `convert_lf_to_crlf`, detached,
`replaying_remote`, `ReplayDone`, 重放抑制, 旧查询重放, `setsid`,
`DETACHED_PROCESS`, 重连回放, 输入权, 只读

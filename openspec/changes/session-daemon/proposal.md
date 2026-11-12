# Proposal

## Why

crossh 现在只有一种展现，而且这种展现就是状态的属主。这不影响单窗口使用，但它把"一个核心配多种展现"（web、CLI、将来的手机端）变成了结构上做不到的事：

- **终端状态活在一个 gpui Entity 里。** `crates/terminal/src/terminal.rs` 5974 行，`Term` + 选择 + vi + find + 鼠标全部住在这一个实体中，绝大多数方法签名带 `&mut Context<Self>`。换一个渲染前端就要重写它，而不是复用它。
- **会话表键在 UI 实体上。** `src/features/workspace/state.rs:29` 的 `LocalSession.terminal` 是 `Entity<TerminalView>`，会话的身份就是 GPUI 实体身份。
- **跨进程没有任何承载会话的东西。** `crates/crossh-core/src/single_instance.rs` 的全部协议是"一行路径 + `ok`/`err` 应答"。它能转发一次"打开目录"，转发不了字节流、尺寸和退出码。
- **关闭窗口即杀会话。** `src/features/workspace/shell/quit.rs:135` 的 `begin_shutdown` 在 `:151` 对每个会话调 `terminal.request_close`，正在跑的构建随之消失。

同时这件事已经有可对照的先行实现：`l0ng-ai/tty7`（同为 gpui + `alacritty_terminal` 技术栈）用"常驻进程持 PTY、不持 VT 网格、前端各自解析字节流"支撑了三个前端（GUI、CLI、手机 gateway），其 `docs/getting-started/concepts.mdx` 明确写着 *"The GUI does not have to be running at all."* 本变更照它的核心切分走，并按本仓库的分层纪律（crate 图强制、`scripts/check-architecture.sh`）落地。

## What Changes

- **新增会话属主进程。** 新 crate `crossh-session`（零 `gpui`、零 `gpui_platform`）与同 crate 的 `[[bin]] crossh-daemon`：拥有 PTY、每会话输出环形缓冲、输入转发、尺寸仲裁、会话元数据。它**不**做 VT 网格解析，也不包含任何视图；不放根包 `src/bin/`，否则会继承根包的 gpui 依赖。
- **daemon 自己负责 shell。** 决定"跑哪个 shell、装不装 shell 集成脚本"的类型目前住在依赖 gpui 的 `crates/terminal` 里，必须下沉到零 gpui 的 `crossh-core` 才能被 daemon 使用。不装集成脚本就没有 `crossh-command=` 标记，会话 cwd 与 agent 程序状态都会失效。
- **新增跨进程会话协议。** 帧格式为 `u32` 小端长度 + 一字节 kind + 载荷（serde），带 `PROTOCOL_VERSION` 与字符串能力标志；接入必须出示令牌，令牌与端口写在新的 `daemon.port` 里。今天回环端口"同机任意进程可请求打开目录"的风险，在协议权限升级为"可以打字进你的 shell"之后不再可接受。
- **GUI 变成客户端。** 服务端持 PTY 与回放环，客户端持 VT：复用仓库里已有的 display-only 构造路径，并把出向写入统一收在一个分派点，覆盖面包含按键、鼠标、焦点与终端查询应答。仅把"按键"接出去是不够的。
- **CLI 成为第二个前端。** `crossh` 新增会话子命令：列会话（含每个会话的输入权归属）、在会话里跑命令并转发输出与退出码、取某个会话的输出、启停 daemon。它证明"核心与展现分离"不是纸面结论 —— 验收要求 GUI 全程不参与。
- **行为迁移：关闭窗口不再杀 shell。** 关窗口退出 GUI，daemon 与其中的进程继续运行；重新启动 `crossh` 自动重连并回放每个会话的最近输出。
- **BREAKING**：关闭窗口与退出 GUI 不再结束会话（原本会结束）。daemon 自身崩溃或被杀时，会话随之丢失（本变更不做 PTY 交接），这一点在规格里写成明确要求而不是待补的洞。
- **明确不改**：`crossh <path>` 的语义与主窗口 `instance.port` 投递、`git.port` / `note.port` 一律保持现状。会话属主换代与 CLI 路由是两件事，混在一起会让本变更同时背两个 BREAKING。

## Capabilities

### New Capabilities

- `session-host`: 会话归属与接入契约——会话由常驻进程持有、生命周期与任何窗口无关；多个客户端可以接入同一会话，其中只有一个拥有输入权；协议版本与能力不匹配时的拒绝方式；接入必须完成认证；任何前端都能在不依赖 GUI 的前提下完成"发现会话—驱动会话—读取结果"的闭环。

### Modified Capabilities

无（`openspec/specs/` 目前只有 `note-search`，与本变更无关）。

## Impact

- 新 crate `crates/crossh-session`（协议编解码、会话属主、环形缓冲、单实例与握手）+ 同 crate 的 `[[bin]] crossh-daemon`；`Cargo.toml` workspace 成员与 `scripts/check-architecture.sh` 的分层检查各加一处。
- `crates/crossh-core`：接收从 `crates/terminal` 下沉的 shell 选择与终端环境装配类型（零 gpui 侧）。
- `crates/terminal`：新增远程会话类型与四条通道（入向原始字节、出向统一分派、cwd 上报、尺寸），删除已无调用点的 `clone_builder`。该文件在 `scripts/check-architecture.sh` 的体积白名单里，改动必须保持最小，不顺手拆分。
- `crates/crossh-terminal-view`：会话建立从 spawn PTY 改为向 daemon 申请会话并接入；失去输入权时显示只读状态。
- `src/features/workspace`：`LocalSession` 的身份增加 daemon 会话 id，固定标签按 id 恢复；关窗口不再调用 `request_close`，退出意图与"停止 daemon"必须分开；GUI 侧"显式结束会话"的调用点需要穷举。
- `src/app/cli.rs`：新增 `daemon`/`ls`/`run`/`capture` 子命令，并同步更新帮助文本、遮蔽名单与对应测试（新名字会遮蔽同名目录）。
- 依赖：新增 `portable-pty`（跨平台 PTY，阻塞读写，不绑事件循环）；unix 下需要 `libc`（已在依赖图中）用于 `setsid`。
- 安全面：回环 TCP 从"开目录"升级为"可以打字"，必须伴随令牌认证与端口文件权限收敛；`crates/crossh-core/src/single_instance.rs:20-21` 现有的"接受该风险"结论对本协议不再成立。
- 文档：`docs/architecture.md`（新增 crate 与边界规则、进程角色）、`docs/engineering-notes/`（协议帧、重连回放、出向通道划分的经验条目）、`README.md`（会话常驻、CLI 入口与停止 daemon 的后果）。
- 明确不在本变更：托盘、web UI、SSH 远程主机、`execve` 式的 daemon 热升级、daemon 崩溃后的会话恢复、多客户端尺寸租约、`crossh <path>` 语义变更。

# Design

## Context

动机见 `proposal.md - Why`。这里只列决定方案所必需的现状。

终端现在由 `crates/terminal` 一个进程内闭环完成：`alacritty.rs:194` 建 `Term`、`:211` 建 `EventLoop`、`pty_tap.rs` 的 `TeePty` 夹在 PTY 与 VT 之间把 OSC 7501 抄一份出来。字节流边界**已经存在**，只是两端都在同一个进程里。这条边界是本变更唯一要切开的地方。

工作区把会话身份绑在 UI 上：`state.rs:29` 的 `LocalSession.terminal` 是 `Entity<TerminalView>`；`registry.rs` 直接持有 `Entity` 与 gpui 的 `Task`/`Subscription`。跨进程投递只有 `single_instance.rs` 的一行路径（`:139-148`）。

`crates/terminal` 是上游派生代码（5974 行，`scripts/check-architecture.sh` 的体积白名单成员），任何改动都要按"最小、不顺手拆分"处理。它的终端类型只有两个分支：`TerminalType::{Pty, DisplayOnly}`（`terminal.rs:1505-1511`），这是客户端接入远程会话最自然的落点。

对照实现 `l0ng-ai/tty7` 的关键事实（本设计的取法来源）：daemon 只依赖 `portable_pty`，全仓 `Term::new(` 只出现在客户端；结构化载荷用 `serde_json`，帧为 `u32` 小端长度 + 1 字节 kind，一次 `write_vectored` 写全，socket 设 `TCP_NODELAY`；daemon 以 detached 方式启动，生命周期不随发起它的进程。

## Goals / Non-Goals

**Goals**

- 会话归属从窗口移到常驻进程，且这条边界可被 crate 图强制。
- 一个会话同时被多个前端接入；其中只有一个能写。
- 不依赖 GUI 的完整闭合路径（发现 → 创建 → 输入 → 输出与退出码 → 结束）。

**Non-Goals**

- 不引入 VT 网格协议（不做服务端 cell diff）。
- 不做远程主机（SSH）、不做 web 前端、不做托盘。
- 不做 daemon 热升级与会话交接；daemon 结束即会话结束。
- 不做 `tty7` 的尺寸租约（observer 以自己尺寸观察）；本变更尺寸只由输入所有者决定。
- **不改 `crossh <path>` 语义，也不引入"daemon 唤起 GUI"**：主窗口的 `instance.port` 投递本变更不动，`<path>` 仍然投给运行中的 GUI。会话属主换代与 CLI 路由是两件事，混在一起会让本变更同时背两个 BREAKING。

## Decisions

### D1 常驻进程持 PTY 与回放环，不持 VT 网格

切分点选在**字节流**，而不是"把 alacritty `Term` 挪进 daemon 再下发差异"：

- 服务端持：PTY、每会话输出环、输入转发、尺寸、会话元数据。
- 客户端持：VT 解析与画面（它本来就持）。

否决"服务端持网格 + cell diff 协议"：`crossh-terminal-view` 的渲染路径直接读 `Term`（`terminal.rs:2452` 的 `with_renderable_cells`、整个 `crates/crossh-terminal-view`），服务端持有网格意味着这套渲染要么搬去 RPC 之后要么作废；同时要自己发明一套 diff 协议，而字节流的解析实现是现成的（GUI 用 fork 的 `alacritty_terminal`，将来的 web 用 xterm.js，CLI 直接用上游 `alacritty_terminal`）。

否决"用 tmux/dtach 当会话属主"：Windows 不可用，且会话元数据（cwd、程序状态）要再解析一遍 `tmux` 输出，比自带 PTY 更绕。

### D2 客户端复用 `TerminalType`，开四条通道

`TerminalBuilder::new_display_only_with_bounds`（`terminal.rs:1003`）已经能建一个"有 `Term`、没有 PTY"的终端；`tests/terminal_replay.rs:36` 正是用它把字节重放进 VT 并断言画面与模式。`TerminalType` 只有 `Pty` 与 `DisplayOnly` 两个分支（`:1505-1511`），新增第三个 `Remote` 分支是改动量最小的接法：所有出向写入都在 `write_to_pty` 里按 `terminal_type` 分派，**不需要在 19 个调用点各加一个 if**。

四条通道：

1. **入（daemon → VT）**：新增 raw 入口，**不经过 `write_output`**。原因是 `write_output`（`:1990-2003`）会先跑 `convert_lf_to_crlf`（`:3271-3287`），而那个函数的注释自己写明它是给"从管道而非 PTY 抓到的输出"补 `ONLCR` 用的，且每次调用都重置 `previous_byte_was_cr`。daemon 送来的就是 PTY 原始字节，再补一次 `\r` 会改变 raw 模式程序的语义（视觉无害，所以 `tests/terminal_replay.rs` 现在是绿的，掩盖了差异）。做法：新入口只做 `output_processor.advance` + 标记检测 + `Event::Wakeup`，不动 `write_output` 的现有语义（display-only 的本地用例继续用它）。
2. **出（VT → daemon）**：真正的唯一漏斗是 `write_to_pty`（`:2139`），它有 18 个调用点，大部分**绕过** `input()`（`:2159`）。按权限归属分成两类，**这是本设计最容易被写错的地方**：
   - **必须送到 daemon**：按键（`input()`）、鼠标上报（`:2683`、`:2781`）、alternate scroll 转方向键（`:2832`、`:2841`）、焦点进出（`:2470`、`:2476`）、`TerminalBackendEvent::PtyWrite`（`:1672`）、终端特性探测应答（`:1696`）、颜色查询应答（`:1723`）。
   - **必须由服务端回答**：`TerminalBackendEvent::TextAreaSizeRequest`（`:1672-1675`）——尺寸权威在服务端，客户端不得用它自己的格子尺寸去回答远端程序的查询。
   启动脚本（`:1387`/`:1391`/`:1401`/`:1403`）改由 daemon 侧执行，客户端不发送。
3. **cwd（daemon → 会话表）**：`is_remote_terminal` 目前只**关闭**客户端嗅探——`working_directory()` 直接返回 `None`（`:2889-2894`），`record_cwd_change` 立即 `return`（`:2929-2932`），全文件没有 setter。因此必须新增 `set_remote_working_directory(...)`，并让 `working_directory()` 在 remote 时返回它；否则 `view.rs:375-410` 的上报链路会退化成 `None`，侧栏、状态栏路径与按 cwd 的链接解析全部冻结在创建目录。
4. **尺寸（客户端 → daemon）**：`set_size`（`:2114`）完成后仅由持有输入权的客户端上报。

`clone_builder`（`:3210-3229`）会 `TerminalBuilder::new` 起一个本地 PTY，当前无调用点。daemon 化后它是"绕过会话属主再开一个 shell"的死路径，本变更一并删除。

否决"把 `Terminal` 拆成模型 + 视图两半"：那是 5974 行上游派生代码的结构性重写，每次 Zed rev 升级的 diff 会失控；`TerminalType::Remote` 用第三个枚举分支拿到同样的效果。

### D3 服务端只嗅探它必须知道的字节

daemon 不持网格，但必须知道两件与会话表有关的事实：

- **cwd**：会话表需要它（`LocalSession.cwd`、侧栏、固定标签恢复）。
- **程序状态**（OSC 7501）：侧栏与状态栏需要，且会话一旦脱离具体窗口，客户端各自嗅探会得到互相竞争的结果。

复用 `Osc7501Scanner` 的扫描逻辑（`crates/crossh-core/src/terminal/program_status.rs:193`，零 gpui，`crates/crossh-terminal/src/lib.rs:8-11` 只是重导出），直接挂在 daemon 的阻塞读循环上。**不需要** `pty_tap.rs` 的 `TeePty`：那个类型是为 alacritty 的 `EventedReadWrite` 适配而生的，daemon 不用那条路径（见 D11）。

标题、提示符边界、链接、图像仍由客户端从字节流自己算 —— 那是纯展示语义，放服务端只会变成第二份要维护的实现。

### D4 传输复用回环 TCP，令牌走新文件与新解析函数

现有 `single_instance.rs` 已经是"回环 TCP + 每用户缓存目录下的端口文件"，三平台一套实现，不引 Unix socket / 命名管道分支。本变更沿用这个载体，只把"一行路径"换成帧协议。

不引入 `tty7` 的 `Duplex` 抽象：tty7 需要它是为了同一套协议同时跑本地 socket 与远程 TCP，本变更没有远程需求。

**权限前提变了，必须补认证**：今天协议能做的最坏事是"开一个目录"，`single_instance.rs:20-21` 明确接受"同机任意进程可请求"这个风险。新协议能做的最坏事是"往你的 shell 里打字"，同一条结论不再成立。处置：

- 令牌与端口写在**新文件 `daemon.port`** 里，配**新的解析函数**；`instance.port` / `git.port` / `note.port` 及其解析路径（`read_port` `:256-258`、`try_forward_to` `:102-137`）一律不动，避免三份文件互相污染。
- daemon 用**自己新建的子目录**（不是 `port_dir()` 的共享目录，`:63-71`）：`#[cfg(unix)]` 下把目录设 `0700`、文件设 `0600`；Windows 依赖每用户目录的默认 ACL，不假装有 `PermissionsExt`。
- 握手第一帧必须带令牌；拒绝应答是固定文案，不含会话数量、标题、cwd、内容；令牌不得进入日志。

否决"改用 Unix domain socket 靠文件权限认证"：Windows 没有对应语义，命名管道要第二套实现；令牌一处实现三平台一致。

否决"不加认证，仅回环"：浏览器页面同样走回环，任何本地进程都能拿到完整 shell。

### D5 帧格式、版本与能力协商

照 `tty7` 的形态：`u32` 小端长度 + 1 字节 kind + 载荷；结构化载荷用 `serde_json`，纯字节载荷（输入、输出）不带编码开销；帧头与载荷用一次 `write_vectored` 写出；socket 设 `TCP_NODELAY`（否则每次 PTY 读会拆成三个小包，刷屏时放大成三倍唤醒）。

`MAX_FRAME = 8 MiB`（PTY 单次读远小于此），超出直接拒绝而不是尝试分配。

`PROTOCOL_VERSION` 单调递增，另带一组字符串能力标志。每个标志必须写清"对不支持它的对端该怎么办"，因为标志存在的意义就是让新旧两端各自决定行为，而不是让旧端崩掉。

版本不匹配时的拒绝文案必须直接给出可执行的恢复步骤（`crossh daemon stop`）：否则二进制升级后旧 daemon 常驻，用户面对的只是一句"版本不符"，GUI 起不了任何终端。

### D6 部署形态与启动方式

否决 `tty7` 的单二进制 `--daemon` 形态：tty7 的 daemon 映像里照样链着 gpui（它只是不初始化），而本仓库的分层靠 crate 图与 `scripts/check-architecture.sh` 强制（`:45-51`、`:53-59`）。单二进制会让"daemon 不含 UI"从"链接期可验证"退化成"约定"。

落地：新 crate `crates/crossh-session`，同时提供库（协议、会话属主、回放环、单实例、握手）与 `[[bin]] crossh-daemon`（只做装配：日志、locale、tokio 运行时、`run()`）。

**二进制不能放在根包的 `src/bin/` 下**：那里的每个 target 都继承根包的依赖（根 `Cargo.toml:22` 直接依赖 gpui），daemon 会照样链进 gpui。放在 `crossh-session` 自己的 `src/main.rs`，依赖闭包才真正不含 `gpui`。

**daemon 必须以 detached 方式启动**，否则本变更的主承诺会静默失效：`cargo run` 是本仓库的日常路径，从终端启动时 daemon 落在同一进程组/控制终端里，用户关掉那个终端窗口就会 SIGHUP 掉 daemon 与所有会话。

- unix：`CommandExt::pre_exec` 里调 `libc::setsid()`（`libc` 已在依赖图中）。
- Windows：`creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)`，`std::os::windows::process::CommandExt` 自带，不引新 crate。
- 环境变量照常继承发起客户端（daemon 里的 shell 仍然需要 `PATH`/`LANG` 等）；detached 只影响会话与进程组，不清理环境。

### D7 单实例：原子占位，不复用现有端口文件

现状 `serve_to`（`single_instance.rs:174-186`）是"绑定随机端口 → `create_dir_all` → `std::fs::write` 覆盖"，没有任何排他。**"写完后回连自校验"是竞态假解**：A 写→A 读到自己→认为胜出；B 写→B 读到自己→也认为胜出，对称。

改成原子占位：`OpenOptions::create_new` 独占创建端口文件，成功者即属主；失败者读文件、连上去、投递请求后退出。属主正常退出时删除文件；启动时若连接失败则视为陈旧文件，删除后重试一次。

主窗口的 `instance.port` 与 `git.port` / `note.port` 本变更一律不动（见 Non-Goals 与 D4），因此 `crossh <path>` 的现有转发路径不受影响。

### D8 输入所有权：controller / observer

v1 规则：第一个接入者持有输入权（controller），其余为只读观察者；**在接客户端发送输入即取得输入权**（不需要额外命令）；controller 主动断开时输入权释放。尺寸只接受 controller 上报。

易手必须可观察：`crossh ls` 的每一行显示该会话当前的角色，易手后随之改变；GUI 在失去输入权时显示只读标记。

否决"多客户端共享输入"：两个前端同时打字进一个 shell 是数据损坏而不是并发。

否决"照搬 tty7 的 `LeaseRequest::{Take,Release,Watch,TakeBack}`"：那套机制是为"手机以自己尺寸临时观察"准备的，本变更没有第二个尺寸来源。

**已知 artefact（接受并记录）**：observer 以自己宽度渲染时，`ALT_SCREEN` 程序（vim、htop）的画面是按 controller 的格子排版的，在观察者窗口里会重排错位。本变更不解决，也不把它当 bug。

### D9 回放环：512 KiB 上限，写进规格

每会话保留最近 512 KiB 原始输出（约 5000 行 × 100 列），超出按"保留最近、先丢最旧"截断。接入时先投递回放，再投递一条带标记的帧表示回放结束，之后才是实时输出。

上限必须写进规格而不是只写在设计里，否则"回放超出上限"这个场景无法验证。不含配置项：需求方提不出第二种取值。取 512 KiB 而不是 tty7 的 8 MiB，是因为本仓库的空闲内存基线是 ~70MB 量级（`README.md` 特性行），而回放环按会话数线性叠加。

回放不重复投递，也不重复触发已发生过的程序状态与通知。

### D10 关闭窗口与退出的语义分开

- 关窗口：只关窗口。不再进入 `quit.rs:135` 的 `begin_shutdown`，不再 `request_close`（`:151`）。
- 退出 GUI：同样不结束会话；会话属主是 daemon。
- 结束会话：显式关闭该会话，或停止 daemon。
- 关闭确认提示（`quit.rs:75-82` 的 `request_exit`）只在"确实会破坏东西"的路径上出现；窗口关闭不再属于这类路径。

反直觉但必要：如果"退出应用"顺手停掉 daemon，那么 daemon 化只换来了"关窗口不杀进程"，用户一点退出仍然全部丢失，与规格里"生命周期不依附窗口"的要求不一致。

### D11 daemon 的 shell 启动契约

会话由 daemon 创建，那么"跑哪个 shell、带什么 env、装不装集成脚本"就必须在 daemon 侧定，而决定它的代码现在分布如下：

- `Shell` / `ShellKind` / `ShellBuilder` / `get_system_shell`：`crates/terminal/src/shell.rs:26-160`，而那个 crate 依赖 gpui（`crates/terminal/Cargo.toml`），`crossh-session` 拿不到。
- `LocalShellEnvironment`（写 ZDOTDIR / `--rcfile` 的 shell 集成脚本）：`crates/crossh-core/src/terminal/shell.rs:38`，已经是 gpui-free。
- `insert_zed_terminal_env`：`crates/terminal/src/terminal.rs:660`；子进程信号掩码：`crates/terminal/src/alacritty.rs:157`。

**决定**：把 `Shell`/`ShellKind`/`get_system_shell`/`ShellBuilder` 与 `insert_zed_terminal_env` 下沉到 `crossh-core`（零 gpui），`crates/terminal` 保留 re-export 以免那 5974 行里的调用点扩散改动。理由：`crossh-core` 已经是 gpui-free 的共享逻辑层，`LocalShellEnvironment` 就住在那里。

**不装 shell 集成等于什么都没做**：没有 `crossh-command=` / `crossh-command-status=` 标记（`crates/crossh-core/src/terminal/shell.rs:19-20`），D3 的 cwd 与 OSC 7501 程序状态都会连带失效。daemon 必须显式装配 `LocalShellEnvironment`。

**PTY 来源**：用 `portable_pty`（跨平台、阻塞读写、无事件循环耦合），在 daemon 的读线程里直接 `read()`。

否决"复用 `alacritty_terminal::tty::new`"：它返回的 `Pty` 与 `EventLoop` + poller 绑定——`tty/unix.rs:339` 明确把 master fd 设成非阻塞，用它就得自己重写一套 poller 的注册/注销与子进程事件循环，比一个依赖更贵。否决"用 `EventLoop` 但喂给一个假 Term"：与 D1 直接冲突。

`portable_pty` 是本变更唯一的实质新依赖（`Cargo.lock` 里目前没有它）。

## Risks / Trade-offs

- **回环端口 + 令牌仍是本机信任模型** → 令牌不落日志、权限按平台收敛、拒绝应答固定文案；测试覆盖"无令牌拿不到会话列表"。跨机访问控制不在范围（没有远程能力就没有暴露面）。
- **`crates/terminal` 出现回归** → 改动限定为"`TerminalType::Remote` 分支 + raw 入向入口 + cwd setter + 删 `clone_builder`"，不碰渲染/选择；用 `tests/terminal_replay.rs` 扩展成对拍用例。
- **入向字节保真**：若 raw 入口与 `write_output` 混用，会出现"某些程序多一个 `\r`"这种只有 TUI 才看得见的偏差 → 两条入口分开，对拍夹具包含 raw-mode 裸 `\n`。
- **刷屏时的帧风暴**（每个 PTY 读一帧 → 每帧一次唤醒） → 按读缓冲批量成帧、`TCP_NODELAY` + 一次 `write_vectored`；仍不够时的退路是在服务端做短窗口聚合（Open Question，不预先实现）。
- **daemon 崩溃等于会话全丢** → 规格已写成要求而不是隐含行为；缓解（环落盘、会话交接）留给后续变更。
- **`crossh-session` 的依赖面**：`crossh-core` 本身不算轻（git 解析、i18n、sysinfo）。可接受：daemon 本来就要用它的 `program_status`、`logging`、`LocalShellEnvironment`，切出一个更小的 crate 是另一笔重构。
- **Windows 侧 PTY 与进程取消**：`portable_pty` 的 ConPTY 行为、`DETACHED_PROCESS` 的效果无法在本机验证 → 由 CI 编译 + 实机冒烟承接，明确写进 tasks 的未验证项。
- **注意力分散风险**：本变更同时动协议、终端 crate、工作区状态三处。处置是把"daemon + 协议"、"GUI 接入"、"CLI"分成三个可独立验证的块。

## Migration Plan

- 无数据迁移：没有持久化的会话格式；`instance.port` 保持原用途，`daemon.port` 是新文件。
- 用户可见迁移：首次启动自动拉起 daemon；关窗口与退出 GUI 不再杀 shell；`crossh <path>` 语义**不变**。
- 回滚：整体 revert 提交即可，daemon 不写任何跨版本状态。
- 发布说明必须写清"关窗口不再结束会话"与"停止 daemon 会结束所有会话"，避免用户把它当成 bug。

## Open Questions

- web 前端接入时的尺寸策略：仍然 controller 独占，还是引入 tty7 式租约？等确有第二个尺寸来源再定。
- 刷屏性能若需要短窗口聚合，聚合窗口由服务端固定还是客户端声明？（不改变本变更的任何规格）
- 托盘与"daemon 的常驻入口"是否合成一个后续变更。
- 自动检测并重启版本不匹配的旧 daemon（本变更只要求文案给出 `crossh daemon stop` 指引）。

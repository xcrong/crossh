# Review: session-daemon

- 评审日期：2026-11-10
- 评审对象：`proposal.md` / `specs/session-host/spec.md` / `design.md` / `tasks.md`
- 结论：**带条件通过**
- 说明：本次评审按 `AGENTS.md — OpenSpec Change Review Gate` 执行。评审前该目录下不存在 `review.md`（`openspec/changes/session-daemon/` 只有 `.openspec.yaml`、`proposal.md`、`design.md`、`tasks.md`、`specs/`），本文件落盘后 apply 才解除阻塞。

## 核对方式

不采信提案自述。行为主张逐条用 `rg` / 读源码核对（`crates/terminal`、`crates/crossh-terminal-view`、`crates/crossh-core`、`crates/crossh-terminal`、`src/features/workspace/*`、`src/app/*`、`src/main.rs`、`scripts/check-architecture.sh`）。

| 主张 | 证据 | 结论 |
| --- | --- | --- |
| `terminal.rs` 5974 行、`Term`/选择/vi/find/鼠标同住一实体 | `crates/terminal/src/terminal.rs` 末行 `5974`；`with_renderable_cells`、`vi_motion`、`mouse_down`、`find_matches` 均在该文件 | 成立 |
| `new_display_only_with_bounds` 在 `:1003` | `terminal.rs:1003`（`:992` 的 `new_display_only` 委托给它） | 成立 |
| `is_remote_terminal` 构造参数 `:1101`、分支 `:2889`/`:2929`/`:2959` | `:1101` 形参、`:1339` 赋值、`:1542` 字段；`:2889` `working_directory`、`:2929` `record_cwd_change`、`:2959` `cwd_at_line` | 行号成立，**语义被误读**，见 F3 |
| `write_output` `:1990`、`input()` `:2159`、`set_size` `:2114`、`with_renderable_cells` `:2452` | 四处行号逐一命中 | 行号成立；`input()` 作为唯一出向出口**不成立**，见 F1 |
| display-only 终端不会向不存在的 PTY 写入/panic | `write_to_pty`（`terminal.rs:2139-2161`）只匹配 `TerminalType::Pty`，display-only 是显式 no-op；`InternalEvent::Resize`（`:1743-1770`）只对 Pty 调 `pty_tx.resize`，本地 `resize(term, …)` 照做；`Drop`（`:3445-3450`）`subprocess` 为 `None` 时空转 | **成立**（D2 的这条前提是干净的：鼠标/焦点/查询应答是"静默丢弃"，不是崩溃） |
| `tests/terminal_replay.rs` 已用 display-only 路径重放字节 | `/tests/terminal_replay.rs:36` 用 `TerminalBuilder::new_display_only`＋`write_output` 断言画面与 `Modes` | 成立 |
| `alacritty.rs:194` 建 `Term`、`:211` 建 `EventLoop`、`TeePty` 夹在中间 | `alacritty.rs:194`/`:211`；`crates/terminal/src/pty_tap.rs:23` `struct TeePty` | 成立 |
| `LocalSession.terminal` 是 `Entity<TerminalView>`（`state.rs:29`） | `src/features/workspace/state.rs:29` 精确命中 | 成立 |
| `quit.rs:135` `begin_shutdown`、`:151` `request_close` | 精确命中；`request_exit` 在 `:75-82`，`quit_risks` 在 `:124-131` | 成立 |
| `single_instance.rs` 协议是"一行路径 + `ok`/`err`"（`:140-150`、`:208-210`） | `:139-148` `write_request` 写一行路径、`:149` `read_reply`；`:204-211` `ok\n` / `err …` | 成立 |
| 无认证风险结论在 `single_instance.rs:20-27` | 实际是 `:20-21`；`:22-23` 是非 UTF-8 路径取舍，`:25-27` 是 `use` | **范围溢出**，见 F12 |
| `Osc7501Scanner`/`parse_program_status` 在 `crossh-core`，`crossh-terminal/src/lib.rs:8-13` 只是重导出 | `crates/crossh-core/src/terminal/program_status.rs:193`；`crates/crossh-terminal/src/lib.rs:8-11` 为重导出（`:13` 是无关的 `use serde`） | 成立（`:8-13` 同样溢出一行） |
| `check-architecture.sh:44-60` 检查逻辑 crate 不得出现 `gpui` | `:45-51` gpui 检查、`:53-59` `crossh_ui\|crossh::\|crate::features` 检查 | 成立 |
| 二进制不能放根包 `src/bin/`（会继承 gpui） | 根 `Cargo.toml:22` 直接依赖 gpui；`src/bin/` 现有 `crossh-git.rs`/`crossh-note.rs`/`crossh-updater.rs` | 成立 |
| `view.rs:242` 是 display-only builder、`:261` 是 spawn PTY | 精确命中；全仓生产代码里 `TerminalBuilder::new(` 只有 `view.rs:261` 一处，会话唯一入口是 `from_local_zed`（`shell/mod.rs:450`） | 成立（spawn 面很窄，好事） |
| `src/app/single_instance.rs`、`src/app/cli.rs`、`src/main.rs` 的投递点 | 转发在 `main.rs:113`，`serve` 在 `main.rs:166`；`app/single_instance.rs:8` 重导出 | 成立 |
| `openspec/specs/` 只有 `note-search`，无重复能力/命名冲突 | `openspec/specs/` = `.gitkeep` + `note-search/`；其 4 条 requirement 与 `session-host` 无交集 | 成立，`Modified Capabilities: 无` 正确 |
| `crossh_core::logging` 可直接复用 | `crates/crossh-core/src/lib.rs:24` `pub mod logging;` | 成立 |

## 发现

> 本次评审无写权限，下列全部为**待改**（未修），请在 apply 前按条目修改产物文本。

### F1（待改，P1）出向通道选错层：`input()` 不是 PTY 写入的唯一出口

`design.md` D2 第 2 条把"出"定在 `input()`（`terminal.rs:2159`），`tasks.md` 4.1 也写作"输入字节出口"。但真正的唯一漏斗是 `write_to_pty`（`terminal.rs:2139`），它有 18 个调用点，其中大量**绕过 `input()`**：

- 鼠标上报：`:2683`、`:2781`；alternate scroll（滚轮→方向键）：`:2832`、`:2841`；
- 焦点进出：`:2470`、`:2476`；
- VT 对 shell 的查询应答：`:1663`（`TerminalBackendEvent::PtyWrite`）、`:1696`（`FEATURE_DETECTION_REPLY`）、`:1723`（颜色查询应答）、`:1672`/`:1674`（尺寸查询应答）。

后果：照字面实现，鼠标报告、焦点事件、DA/颜色查询应答、OSC 7501 特性探测应答全部在客户端静默丢弃（display-only 下 `write_to_pty` 是 no-op，不会 panic），TUI 鼠标与 nvim/vim 的查询会"半死"。这是"方案缺了一条必要通道"。

**最小修**：D2 把"出"改指 `write_to_pty`（`terminal.rs:2139`），并明确区分两类写入——必须送 daemon 的（按键、鼠标、焦点、查询应答）与只在本地有意义的（`:1674` 的尺寸应答应改由 daemon 侧回答）；`tasks.md` 4.1 增加断言："注入每类 VT 事件后，socket 侧收到对应字节"。

### F2（待改，P1）入向通道不是"原始字节"：`write_output` 会改写线上字节

`write_output`（`terminal.rs:1990-2003`）先跑 `convert_lf_to_crlf`（`:3271-3287`），而该函数的注释自己写明它是给"**从管道而非 PTY** 抓到的输出"补 `ONLCR` 的；并且每次调用都重置 `previous_byte_was_cr`。

把 daemon 的 PTY 原始字节塞进这条通道，等于在客户端做一次字节改写：raw 模式程序（自行关闭 `ONLCR`）的裸 `\n` 会被插入 `\r`，与"本地 PTY 直连"语义不同；分块边界还会产生多余 `\r`（视觉无害，所以 `tests/terminal_replay.rs` 现在是绿的，掩盖了差异）。这与 D9"回放环保存的是原始输出"、任务 4.2"两条路径对拍画面一致"的假设相冲突。

**最小修**：D2 明确"daemon 路径不复用 `convert_lf_to_crlf`"（新增 raw 变体，或在远程构造时跳过该转换），任务 4.2 的对拍夹具必须包含 raw-mode 裸 `\n` 片段。

### F3（待改，P1）cwd 通道不存在，且 D2 对 `is_remote_terminal` 的描述是错的

D2 称"`is_remote_terminal` 已经为'cwd 由外部上报'让好了路"。核实：它只**关闭**客户端嗅探——

- `working_directory()`（`terminal.rs:2885-2896`）在 remote 时直接返回 `None`（原注释："SSH 主机无法可靠探测 cwd，所以不显示也不持久化"）；
- `record_cwd_change`（`:2929-2932`）在 remote 时立即 `return`；
- `cwd_at_line`（`:2959`）短路到 `working_directory()`。

也就是说它不携带任何外部值，也没有任何 setter。后果：`crates/crossh-terminal-view/src/view.rs:375-410` 的上报链路（`working_directory()` → `TerminalEvent` → `shell/mod.rs:847 session.cwd = cwd`）退化为 `None`，侧栏/状态栏路径/按 cwd 的链接解析全部冻结在创建目录；`view.rs:400` 的 shell 集成标记路径虽仍可用，但它依赖 daemon 侧装同一套集成（见 F4）。

**最小修**：D2 改为"需要新增 remote cwd 通道：`set_remote_working_directory(...)` + `working_directory()` 返回它"；`tasks.md` 4.1 的三条通道改成四条并补单元测试。

### F4（待改，P1）daemon 的 shell 启动契约未定义，所需代码在禁止依赖的 gpui crate 里

`tasks.md` 2.1 只说"每会话一个 PTY 子进程"。但决定"跑哪个 shell、带什么 env、装不装集成脚本"的代码分布是：

- `Shell` / `ShellKind` / `ShellBuilder` / `get_system_shell`：`crates/terminal/src/shell.rs:26-160`，而 `crates/terminal/Cargo.toml:19,22` 依赖 gpui；
- `insert_zed_terminal_env`：`crates/terminal/src/terminal.rs:660`；信号掩码 `current_child_signal_mask`：`crates/terminal/src/alacritty.rs:157`；
- 只有 `LocalShellEnvironment`（写 ZDOTDIR/`--rcfile` 的 shell 集成脚本）在 gpui-free 的 `crates/crossh-core/src/terminal/shell.rs:38`。

`crossh-session` 被命令"零 gpui"，因此拿不到前半部分。更关键：不装 shell 集成就没有 `crossh-command=` / `crossh-command-status=` 标记（`crates/crossh-core/src/terminal/shell.rs:19-20`），F3 的 cwd 与 OSC 7501 程序状态都会连带失效。另外 daemon 的 PTY 来源也没写：现有实现是 `alacritty_terminal` 的 `tty::new`＋`EventLoop`（`alacritty.rs:194/211`），而 `EventLoop` 与 VT `Term` 绑死，与 D1"服务端不持网格"冲突。

**最小修**：D6/tasks 2 补一份"shell 启动契约"决定：把 `Shell`/`get_system_shell`/`ShellBuilder` 下沉到 gpui-free crate（`crossh-core` 或 `crossh-session`，`crates/terminal` 保留 re-export），daemon 侧显式装配 `LocalShellEnvironment`；并写清 PTY 来源（alacritty `tty::new` 的裸 `Pty`，还是新增 `portable_pty`）。

### F5（待改，P1）`tasks.md` 1.1 的依赖白名单自相矛盾且不足，照此实施任务 2 编不过

1.1 限定"依赖只取 `serde`/`serde_json`/`dirs`/`log`/`tokio`"，但 2.4 要求"`Osc7501Scanner` 从现有 `crossh-core` 复用"、3.3 要求"日志（复用 `crossh_core::logging`，见 `crates/crossh-core/src/lib.rs:24`）"，且白名单里没有任何 PTY crate。

**最小修**：1.1 白名单加上 `crossh-core` 与选定的 PTY crate，保留"不得出现 `gpui`/`gpui_platform`/`crossh-terminal`"这条真正的约束（`crates/terminal` 因依赖 gpui 必须继续排除）。

### F6（待改，P1）D7 的单实例校验是竞态假解，规格的"至多一个属主"因此不成立

现状 `serve_to`（`crates/crossh-core/src/single_instance.rs:174-186`）是"绑定随机端口 → `create_dir_all` → `std::fs::write` 覆盖"，无任何排他。D7 只加"写完后回连自校验"，在真正并发下是对称的：A 写 → A 立刻自读到自己 → A 认为胜出；随后 B 写 → B 读到自己 → B 也认为胜出。它只能抓"对手先写"，抓不到规格 scenario 所描述的"几乎同时"。

**最小修**：改成原子占位——端口文件的 `OpenOptions::create_new` 锁、`flock` 独占、或固定端口 `bind` 失败即认输；`tasks.md` 3.1 的用例改成可确定复现（并发 spawn N 次属主，断言恰好一次胜出、其余以"已在运行"退出）。

### F7（待改，P1）规格 R2 有不可观察的 MUST 与不可执行的场景

`specs/session-host/spec.md`「一个会话可被多个前端接入，输入所有权唯一」：

- 要求句"输入权易手时，涉及的两个客户端都 MUST 能观察到这件事，而不是静默失效"——**没有任何 scenario 说明"怎么观察"**（GUI 显示什么？CLI 打印什么？），属于典型的空洞断言；
- Scenario「输入权持有者断开」里"仍在接入的任一客户端**可以取得**它"没有定义取得方式（自动？发一次输入？显式命令？），无法写成测试。

**最小修**：把前者落成可观察断言（例：`crossh ls` 显示 controller/observer 角色且易手后改变；GUI 在失权时显示只读标记）；把后者改成"任一在接客户端发送输入即取得输入权"，并给 CLI 侧的人工/自动步骤。

### F8（待改，P1）规格 R3 的两句不可测

「接入时回放最近输出」：

- "客户端 MUST 能辨别自己收到的是一次回放而不是新产生的输出"是协议内部义务，没有可观察后果；
- "回放有明确上限"没给上限，于是 Scenario「回放超出上限」无法验证——上限 512 KiB 只存在于 D9。

**最小修**：前者改成可观察后果（例如"回放不重复投递、不重复触发通知与程序状态"）；后者在 spec 写死上限，或写明"固定且文档化的常量"并让 `tasks.md` 2.1 的测试对准该值。

### F9（待改，P1）"唤起 GUI"没有机制决策，任务 5.3 的验证无法达成；且该行为在规格里没有 requirement

D7、proposal「What Changes」、`tasks.md` 5.3 都要求"`crossh <path>` 在 GUI 未运行时唤起 GUI 并打开该目录"，但 design 对此**零决策**：daemon 是被 GUI 拉起的，它没有用户会话的 `PATH`/`DISPLAY`/`WAYLAND_DISPLAY`，macOS 上还不知道该开 `.app` 还是裸二进制，Windows 上还要避免黑窗。同时 `session-host` 规格里没有任何一条 requirement 覆盖这条 BREAKING 行为变化。

**最小修**：二者择一——(i) D7 补平台机制并加一条 ADDED requirement＋scenario；或 (ii) 把"唤起 GUI"移出本变更（`crossh <path>` 无 GUI 时只投递 daemon，GUI 由用户自行启动），并同步 proposal 的 BREAKING 措辞。

### F10（待改，P1）GUI 侧"显式结束会话"的调用点没被穷举

`tasks.md` 4.5 只改了 `quit.rs:135/151`。但 `TerminalView::request_close`（`crates/crossh-terminal-view/src/view.rs:789-799`）现在是"置 `Closed` + `input(vec![0x04])`"：在 daemon 模型下 Ctrl-D 只有 shell 空闲时才结束会话（有前台命令时它只是 stdin 数据），也不会释放客户端连接或通知 daemon。其余调用点：`shell/tabs.rs:208/217`、`shell/split.rs:127`、`tab_strip.rs:301/373`、`shell/mod.rs:365`。（`is_command_running` 用 `ALT_SCREEN` 判定（`view.rs:870-877`），client 侧仍然可用，这条不是问题。）

**最小修**：4.5 增加"显式关闭 = 向 daemon 发 close 请求 + 断开连接"，并把这些调用点列进任务；顺手说明关闭标签仍保留"有命令在跑"的确认（因为它仍然会结束会话）。

### F11（待改，P1）daemon 必须脱离调用者进程组，否则"关窗口不结束会话"这条主承诺会静默失效

D6 / D10 / `tasks.md` 3.3 都没有要求 daemon 以独立会话启动（posix `setsid` / Windows `DETACHED_PROCESS`）。GUI 从终端启动（`cargo run` 是仓库的日常路径）时，daemon 落在同一进程组/控制终端里；用户关掉发起它的终端窗口 → SIGHUP → daemon 与会话一起消失。

**最小修**：D6 写明 detached 启动与最小环境继承，`tasks.md` 3.3 加断言，`6.3` 冒烟加一步"从终端启动 → 关窗口 → 关闭该终端 → 会话仍在"。

### F12（待改，P2）三处 file:line 引用范围溢出

- `design.md` D4 引 `single_instance.rs:20-27`：真正的"接受该风险"是 `:20-21`，`:22-23` 是非 UTF-8 路径取舍、`:25-27` 是 `use`；
- `design.md` D3 引 `crates/crossh-terminal/src/lib.rs:8-13`：重导出是 `:8-11`，`:13` 是无关的 `use serde`。

**最小修**：分别改为 `:20-21`、`:8-11`。（其余行号——`state.rs:29`、`quit.rs:135/151`、`terminal.rs` 五处、`alacritty.rs:194/211`、`view.rs:242`、`check-architecture.sh:44-60`——逐条命中，保留。）

### F13（待改，P2）proposal 的 Impact 与 design D2、tasks 4.1 三处复述同一组行号

proposal「Impact」写了 `terminal.rs:1003`、`:2889`、`:2929`、`:2959` 与"复用 display-only 路径"，design D2 与 tasks 4.1 又各写一遍。门禁明确要求 proposal / specs / design / tasks 不得互相复述。

**最小修**：proposal 只保留受影响面（crate / 文件），行号与 seam 归 design。

### F14（待改，P2）端口文件权限与目录归属没有落地路径

`port_dir()`（`single_instance.rs:63-71`）是按 debug/release 加 `-dev` 后缀的**共享**目录，`git.port`/`note.port` 也在其中；`serve_to`（`:177-182`）用 `create_dir_all` + `std::fs::write`，全程没有任何权限设置。因此 D4/任务 3.1 的"文件 `0600`、目录 `0700`"需要新代码，且若 `crossh-git` 先建了目录，只 chmod 文件不会把已存在的目录收到 `0700`；Windows 没有 `PermissionsExt`，任务 3.1/3.6 的写法会编不过或断言不到。

**最小修**：D4 写明"daemon 专用子目录 + 目录与文件各自显式设权 + `#[cfg(unix)]`，Windows 依赖每用户目录"；spec 的"令牌载体 MUST 仅当前用户可读"按平台分述。

### F15（待改，P2）新子命令与"被遮蔽的名字"策略冲突未处理

`src/app/cli.rs:70-83` 的帮助文本与 `:140` 的测试 `shadowed_names_need_dot_slash_or_separator` 已把 `git`/`note`/`help` 列为需 `./` 前缀的名字。新增 `daemon`/`ls`/`run`/`capture` 会再遮蔽四个目录名（`crossh run` 将不能再打开 cwd 下的 `run/`），任务 5.1 只说"保持现有 `git`/`note`/`help`/`version` 行为不变"。

**最小修**：5.1 补"同步更新 `help_text`、遮蔽名单与对应测试"。

### F16（待改，P2）退役主窗口单实例的删除面没写，daemon 端口文件名也没定

任务 5.3 说"`serve`/`try_forward` 仅保留 git/note 两个用途"，但没说要删掉 `crossh_core::single_instance::{serve, try_forward, port_file_path}`（`:49-51`、`:88-91`、`:156-159`）、`src/app/single_instance.rs:8` 的重导出，以及断言 `instance.port` 的测试 `viewer_port_files_do_not_clash_with_main_instance`（`:436-450`）。这与"无兼容层"规则冲突。另一方面 D7/Migration 说"新版本不读 `instance.port`"，却没给 daemon 的端口文件名——照现状实施者会复用同名文件，把旧版遗留的裸端口误当成"port + 缺 token"。

**最小修**：5.3 写明删除面，并给 daemon 端口文件一个明确名字（如 `daemon.port`）与残留文件处置。

### F17（待改，P2）规范围里混入载体词汇

R4 scenario 2 的"与**端口信息**中一致的令牌"、R5 的"**握手阶段**/能力标志"。前者应改成"与常驻进程发布的令牌一致"；后者保留语义没问题，但主句应写成客户端可观察的措辞（"较旧的客户端仍能使用双方共同支持的功能"）。规格里没有出现"帧/socket/环"这类词，这点是好的。

### F18（待改，P2）R7 scenario 2 的可观察断言在回放上限下不总成立

「两个前端的观察一致」写"GUI 之后接入该会话时看到同一段输出"，但按 D9 只回放最近 512 KiB，长输出会话看不到"同一段"。

**最小修**：加限定"在回放上限之内"。

### F19（待改，P2）R1 scenario 2 的"同一个会话"缺少可观察判据

现在只有"不是一片空白"，无法区分"同一个会话"与"新建 shell 恰好有 banner"。

**最小修**：加"该会话中此前启动的命令仍在运行（同一个进程仍在推进）"。

### F20（待改，P2）`clone_builder` 会成为绕过 daemon 的死路径

`terminal.rs:3210-3229` 的 `clone_builder` 会 `TerminalBuilder::new` 起一个本地 PTY；`rg clone_builder` 只命中定义，当前无调用点。daemon 化后它是"本地再开一个 shell"的隐患。

**最小修**：任务 4.1 附带删除（符合"无兼容层"规则）。

### F21（待改，P2）observer 尺寸策略只写策略、没写可见后果

Non-goals 说"observer 以自己尺寸观察"，但对 `ALT_SCREEN` 程序来说，observer 看到的是按 controller 格子排版的画面，在自己宽度下会重排错位。设计应把这条已知 artefact 写清（并在 6.3 冒烟里标注为可接受），否则后续会被当成 bug。

### F22（待改，P2）版本升级后的恢复路径缺失

旧 daemon 常驻、二进制升级后，新客户端会按 R5 被"明确拒绝"，于是 GUI 起不了终端。规格只保证"拒绝而不是半可用"，没有任何恢复叙述（`crossh daemon stop` 只在 README）。最小修：`tasks.md` 5.1/5.5 要求拒绝文案与帮助文本直接给出 `crossh daemon stop` 指引；自动重启旧 daemon 留给后续变更（见遗留项）。

### F23（待改，P2）"端口 + 令牌"会污染 git/note 的解析路径

`read_port`（`single_instance.rs:256-258`）与 `try_forward_to`（`:102-137`）是同模块、三个端口文件（`instance.port`/`git.port`/`note.port`）共用的解析路径。D4 只说"沿用这个载体"，没说要为令牌格式新增函数/新文件。最小修：D4 注明"令牌格式走新解析函数与新文件名，不改动 git/note 的既有读取路径"。

### F24（待改，P2）两处任务验证命令是空过或不可用

- `tasks.md` 4.1 的 `cargo test --bin crossh terminal_replay`：`/tests/terminal_replay.rs` 是根包的集成测试，不在 bin target 里，过滤器匹配不到用例即"绿"。应为 `cargo test --test terminal_replay`（6.2 已写对，4.1 与之不一致）；
- `tasks.md` 5.4 的 `tests/cli_sessions.rs` 在根包，拿不到 `CARGO_BIN_EXE_crossh-daemon`（该 bin 属于 `crossh-session`），`cargo test --test cli_sessions` 不会构建它。建议把该 e2e 放进 `crates/crossh-session/tests/`（3.4 已在那里），或在 5.4 写明显式构建＋路径解析。

## 必须关闭的条件

apply 前必须逐条落实并回写本文件标记（`带条件通过` 只放行下列各项）：

- **C1（对应 F4、F5）`crossh-session` 的依赖白名单与 shell 启动契约补全。** 白名单加 `crossh-core` 与选定的 PTY crate；design 增加一节写明 daemon 如何选 shell、装配 `LocalShellEnvironment`、构造 PTY（并说明为何不用与 `Term` 绑死的 alacritty `EventLoop`）；`Shell`/`get_system_shell` 的下沉方案写明属于哪个 crate。未做完这条，任务 2 无法通过编译或无法让 cwd/程序状态工作。
- **C2（F1）出向通道改到 `write_to_pty`。** D2 与任务 4.1 同步改写，并列出"必须送 daemon"与"本地语义"两类写入的划分（含 `:1674` 尺寸应答的归属）；4.1 增加逐类 VT 事件的出口断言。
- **C3（F2）入向通道的字节保真决定。** 明确 daemon 路径是否绕过 `convert_lf_to_crlf`，4.2 的对拍夹具包含 raw-mode 裸 `\n`。
- **C4（F3）remote cwd 通道。** design 增加 setter 与 `working_directory()` 新语义，任务与单元测试落地；同时修订 D2 中"`is_remote_terminal` 已让好路"的错误表述。
- **C5（F6）单实例改为原子占位。** design D7 与任务 3.1 同步改写，3.1 用例改为可确定复现的并发断言。
- **C6（F7、F8、F17、F18、F19）规格可观察化。** R2 的"输入权易手可观察"与"如何取得输入权"、R3 的"辨别回放"与"回放上限"、R7 的"同一段输出"、R1 的"同一个会话"、R4 的"端口信息"全部改成可写成测试或人工步骤的断言。
- **C7（F9）唤起 GUI 的归宿。** 要么补机制决定＋一条 ADDED requirement＋scenario，要么把它移出本变更并同步 proposal 的 BREAKING 说明与任务 5.3。两种选择都必须落文本，不能留空。
- **C8（F10、F11）生命周期两端的调用点。** 任务 4.5 穷举 GUI 侧"显式结束会话"的调用点（`view.rs:789`、`tabs.rs:208/217`、`split.rs:127`、`tab_strip.rs:301/373`、`shell/mod.rs:365`、`quit.rs:135/151`）；任务 3.3/6.3 加上 daemon detached 启动与对应冒烟步骤。

## 遗留项（本次不做，需另立变更）

- **升级后自动重启旧 daemon**：C6/F22 只要求给出可执行指引，客户端自动检测版本不匹配并重启 daemon 另立变更。
- **旧版 `instance.port` 残留清理**：D7 声明新版本不读它即可；清理机制不做。
- **尺寸租约（tty7 式 `Take/Release/Watch`）**：等出现第二个尺寸来源（web 标签尺寸）再设计；observer 重排错位（F21）在本变更内接受并记录。
- **daemon 崩溃后的会话恢复**（环落盘、PTY 交接、`execve` 热升级）：本变更明确不做，规格已把它写成"会话随之消失"的要求。
- **托盘 / web 前端 / SSH 远程主机**：与本变更正交。
- **Windows ACL 与 ConPTY 实机结论**：本机不验证，由 PR CI（`cargo check` + `cargo test -p crossh-session`）与实机冒烟承接；在冒烟结论回写前，发布说明不得写成"Windows 已支持"。

## 条件状态

评审为只读评审，未修改任何产物。下列关闭记录由修订方在评审后逐条落实并回填（2026-11-10）。

- **C1（F4、F5）已关闭。** `design.md` 新增 **D11 daemon 的 shell 启动契约**：`Shell`/`ShellKind`/`ShellBuilder`/`get_system_shell` 与 `insert_zed_terminal_env` 下沉到 `crossh-core`（`crates/terminal` 保留 re-export），明确 daemon 必须装配 `LocalShellEnvironment`，并写清 PTY 来源为 `portable_pty` 及其理由（否决 `alacritty_terminal::tty::new`：`tty/unix.rs:339` 把 master fd 设成非阻塞，脱离 `EventLoop` 使用要自己重写 poller 循环）。`tasks.md` 1.1 的依赖白名单改为 `serde`/`serde_json`/`dirs`/`log`/`tokio`/`crossh-core`/`portable-pty`+unix `libc`，并把真正的约束改成"不得出现 `gpui`/`gpui_platform`/`crossh-terminal`"；新增任务 2.1（类型下沉）与 2.3（集成脚本装配 + 真 shell 断言标记载入）。
- **C2（F1）已关闭。** `design.md` D2 把出向通道改指 `write_to_pty`（`:2139`），给出两条实现要求：新增 `TerminalType::Remote { tx }` 分支让分派集中在一处（不在 18 个调用点各加判断），以及两类写入的划分（必须送 daemon：按键/鼠标 `:2683`・`:2781`/alternate scroll `:2832`・`:2841`/焦点 `:2470`・`:2476`/`PtyWrite` `:1672`/特性探测 `:1696`/颜色 `:1723`；服务端回答：`TextAreaSizeRequest` `:1672-1675`）。`tasks.md` 4.1 列出同样的覆盖面并要求"逐类 VT 事件注入后断言远程出口收到对应字节，缺任何一类都算未完成"，4.3 加只读标记。
- **C3（F2）已关闭。** `design.md` D2 第 1 条明确入向走**新 raw 入口**、绕开 `write_output`（`:1990-2003`）里的 `convert_lf_to_crlf`（`:3271-3287`），并说明 `write_output` 的现有语义保留给 display-only 本地用例。`tasks.md` 4.2 要求对拍夹具必须包含 raw 模式裸 `\n` 片段。
- **C4（F3）已关闭。** `design.md` D2 第 3 条改为事实陈述：`is_remote_terminal` 只**关闭**客户端嗅探（`working_directory()` 返回 `None`、`record_cwd_change` 直接 `return`），全文件无 setter；因此必须新增 `set_remote_working_directory(...)` 并让 `working_directory()` 在 remote 时返回它，否则 cwd 上报链路退化。`tasks.md` 4.1 第 3 条与 4.4 落地。
- **C5（F6）已关闭。** `design.md` D7 把"写完后回连自校验"判定为竞态假解并改为原子占位（`OpenOptions::create_new`、失败即接入、退出删除、陈旧文件重试一次）。`tasks.md` 3.1 的用例改为"并发 spawn N 次，断言恰好一次胜出、其余以已在运行退出"，删除依赖时序运气的写法。
- **C6（F7、F8、F17、F18、F19）已关闭。** `specs/session-host/spec.md`：R2 把输入权易手落成可观察断言（会话列表显示每会话的持有者、GUI 失去输入权时显示只读状态），并把"可以取得"改为"发送一次输入即取得"，场景重写为"断开后可被接管"；R3 删除不可观察的"能辨别回放"，把上限写死为 512 KiB，补"不重复投递、不重复触发已发生的程序状态与通知"，相关断言落进两个场景；R1 场景二补"长命令仍在同一会话里运行并继续产生输出"作为判据；R4 去掉"端口信息"改"常驻进程发布的令牌"并按平台分述可读性；R7 场景补"在回放上限之内"；R5 增加"拒绝时必须给出停止旧实例的具体命令"。
- **C7（F9）已关闭：选择「移出本变更」。** 按用户决定，`crossh <path>` 与主窗口 `instance.port` 投递**本变更一律不动**：`proposal.md` 删掉语义变更与对应 BREAKING 条目、新增"明确不改"条目；`design.md` 的 Non-Goals、D4、D7 三处同步；`tasks.md` 5.3 从"退役单实例"改写为"`daemon.port` 与现有三份端口文件互不影响，`crossh <path>` 行为逐字不变"，并加入"与本变更前一致"的 `rg` 校验。无需新增 requirement（规格没有声称这项行为）。
- **C8（F10、F11）已关闭。** `tasks.md` 4.5 穷举 GUI 侧"显式结束会话"的六个调用点（`view.rs:789`、`tabs.rs:208/217`、`split.rs:127`、`tab_strip.rs:301/373`、`shell/mod.rs:365`、`quit.rs:135/151`），并要求"结束会话 = 向 daemon 发 close 请求 + 断开连接"，同时保留关标签的确认；`design.md` D6 增加 detached 启动平台机制（unix `pre_exec`+`setsid`，Windows `DETACHED_PROCESS|CREATE_NEW_PROCESS_GROUP`，环境照常继承），`tasks.md` 3.3 加 `sid` 断言、6.3 冒烟加"从终端启动 → 关窗口 → 关闭该终端 → 会话仍在"。

### P2 发现的处理

F12–F24 均已按各条的"最小修"落进产物：行号溢出（F12）、proposal 与 design 复述（F13）、权限与目录落地（F14）、遮蔽名单与帮助文本（F15）、
`daemon.port` 命名与删除面（F16，因 C7 退出而缩小为"命名 + 互不影响"）、规格词汇（F17）、回放上限限定（F18）、同一会话判据（F19）、`clone_builder` 删除（F20）、observer 重排 artefact（F21）、版本不匹配的恢复指引（F22）、git/note 解析路径隔离（F23）、两处空过验证命令（F24）。

### 待验证（不由评审关闭）

- `tasks.md` 6.3 的 macOS 实机冒烟、6.5 的 Windows/Linux CI 结论：需实施期回填。
- 归档前需确认：本文件全部条件已标记关闭；6.5 的 CI 结论已回填；`openspec validate session-daemon --strict` 通过。

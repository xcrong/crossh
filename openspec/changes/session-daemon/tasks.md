# Tasks

> 顺序按依赖排列。每个任务自带验证方式；跨平台验证本机做不了的部分标 `[ ]` 并写明由哪条 CI 承接（`AGENTS.md`：本机只负责 macOS arm64 与平台无关逻辑）。

## 1. `crossh-session` 骨架与协议编解码

- [x] 1.1 新建 `crates/crossh-session`（`lib.rs` + `main.rs` + `Cargo.toml`），加入根 `Cargo.toml` 的 `workspace.members`。依赖白名单：`serde`、`serde_json`、`dirs`、`log`、`tokio`、`crossh-core`、`portable-pty`，unix 另加 `libc`。**不得出现** `gpui`、`gpui_platform`、`crossh-terminal`、`crates/terminal`（后者依赖 gpui，必须继续排除）。验证：`cargo check -p crossh-session` 通过；`cargo tree -p crossh-session | rg 'gpui|crossh-terminal'` 无输出。
- [x] 1.2 `scripts/check-architecture.sh` 的两处 `check_absent`（`gpui`、`crossh_ui|crossh::|crate::features`）加入 `crates/crossh-session`。验证：故意在 crate 内写一行 `use gpui::App;`，脚本报 `architecture violation` 并非零退出；删掉后恢复为 0。
- [x] 1.3 帧编解码：`u32` 小端长度 + 1 字节 kind + 载荷，`MAX_FRAME = 8 MiB`，一次 `write_vectored` 写出；结构化载荷 `serde_json`，输入/输出用原始字节。验证：`cargo test -p crossh-session` 覆盖——往返一致、超长载荷被拒、截断输入返回错误而不是 panic、单帧只产生一次 `write_vectored`（自定义 `Write` 计数）。
- [x] 1.4 消息类型、`PROTOCOL_VERSION` 与字符串能力标志表；每个标志的文档注释必须写明"对不支持它的对端该怎么办"。验证：一条断言"未知标志不影响已解析的公共字段"的测试。
- [x] 1.5 握手纯逻辑：版本不一致 → 明确错误类型且错误文案含停止旧实例的具体命令；令牌缺失/错误 → 固定文案拒绝，错误值里不含任何会话字段。验证：`cargo test -p crossh-session` 直接断言错误类型与文案（含断言文案不含令牌子串）。

## 2. 会话属主：shell 契约、PTY、回放环、输入所有权

- [x] 2.1 把 `Shell`/`ShellKind`/`ShellBuilder`/`get_system_shell`（`crates/terminal/src/shell.rs:26-160`）与 `insert_zed_terminal_env`（`crates/terminal/src/terminal.rs:660`）下沉到 `crossh-core`（零 gpui），`crates/terminal` 保留 re-export 以免 5974 行文件里的调用点扩散。验证：`cargo test -p crossh-core shell` + `cargo test -p crossh-session`；`scripts/check-architecture.sh` 通过；`git diff --stat crates/terminal/src/terminal.rs` 只含 re-export 与既有调用点未变。
- [x] 2.2 会话属主结构：每会话一个 `portable_pty` 子进程 + 阻塞读线程 + 512 KiB 环形缓冲（保留最近、先丢最旧）+ 广播给已接入客户端。验证：`cargo test -p crossh-session` 用真实 PTY 起 `sh -c 'printf ...'`，断言输出进入环、截断后只保留尾部。
- [x] 2.3 装配 `LocalShellEnvironment`（`crates/crossh-core/src/terminal/shell.rs:38`），在阻塞读循环里直接调 `Osc7501Scanner` 取程序状态、并从字节流取 cwd。**不使用** `pty_tap.rs` 的 `TeePty`（那是 alacritty `EventedReadWrite` 的适配层）。验证：重放一段含 OSC 7 与 OSC 7501 的固定字节，断言 cwd 与程序状态被更新；另跑一次真实 shell，断言 `crossh-command=` 标记出现（证明集成脚本装上了）。
- [x] 2.4 输入所有权状态机：首个接入者为 controller其余 observer 的按键被丢弃；在接客户端发送一次输入即取得输入权；controller 断开后角色变为"无"。验证：单元测试覆盖四种迁移；集成断言"observer 输入不进 PTY"与"发送一次输入后按键生效"。
- [x] 2.5 尺寸只接受 controller 上报；observer 接入不改变 PTY 尺寸。验证：单元测试断言非 controller 的 resize 被拒且尺寸未变。
- [x] 2.6 会话元数据（id、cwd、标题占位、程序状态、角色、是否存活）与"会话结束"判定：子进程退出 → 会话标记结束并广播随后从列表移除。验证：起 `sh -c 'exit 3'` 断言收到结束事件且退出码为 3。

## 3. 单实例、令牌与 daemon 二进制

- [x] 3.1 单实例用原子占位：`OpenOptions::create_new` 独占创建 `daemon.port`，成功者即属主；失败者读文件、连上去、投递后退出；属主正常退出删除文件；启动时连接失败视为陈旧文件，删除后重试一次。**不得**复用 `instance.port`/`git.port`/`note.port` 及其解析函数（`single_instance.rs:102-137/256-258`）。验证：并发 spawn N 次属主，断言恰好一次胜出、其余以"已在运行"退出（可确定复现不依赖时序运气）。
- [x] 3.2 令牌与端口同文件；Unix 下 daemon 专用子目录设 `0700`、文件设 `0600`，Windows 依赖每用户目录默认 ACL（`#[cfg(unix)]` 分支不假装有 `PermissionsExt`）。令牌只从文件读取不落日志、不出现在错误文案。验证：测试断言拒绝文案不含令牌子串；`rg -n 'token' crates/crossh-session/src` 人工确认没有 `log::` 打印点；Unix 下断言模式位为 `0600`。
- [x] 3.3 `crates/crossh-session/src/main.rs` 提供 `[[bin]] crossh-daemon`：日志（复用 `crossh_core::logging`）、locale、tokio 运行时、`run()`；**以 detached 方式启动**——unix 在 `pre_exec` 里 `libc::setsid()`，Windows 用 `creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)`，环境照常继承。验证：`cargo build -p crossh-session` 产出二进制；`./target/debug/crossh-daemon --version` 输出一行版本号后退出；从 shell 启动 daemon 后 `ps -o sid= -p <pid>` 与该 shell 的 sid 不同。
- [x] 3.4 daemon 集成测试（`crates/crossh-session/tests/`）：spawn 二进制 → 读 `daemon.port` → 令牌握手 → 建会话 → 写输入 → 读输出 → 结束会话；另断言无令牌握手拿不到会话列表。验证：`cargo test -p crossh-session --test daemon_lifecycle`。
- [x] 3.5 `docs/architecture.md` 加入 `crossh-session` 的所有权、边界规则与 `crossh-daemon` 的进程角色（含 detached 启动），crate 图段落同步。验证：段落与 `cargo tree` 实际依赖一致（人工对照一次），`scripts/check-architecture.sh` 通过。
- [ ] 3.6 Windows / Linux 的 daemon 行为（ConPTY、`DETACHED_PROCESS`、令牌文件权限、单实例竞态）本机不验证，由 PR CI 的 `cargo check` + `cargo test -p crossh-session` 承接。验证：CI 绿灯后回填此处。

## 4. GUI 成为客户端

- [x] 4.1 `crates/terminal` 增加 `TerminalType::Remote { tx }`（`terminal.rs:1505-1511` 的第三个分支）与四条通道：
  1. 入：新 raw 入口，**绕开** `write_output`（`:1990-2003`）里的 `convert_lf_to_crlf`（`:3271-3287`），只做 `output_processor.advance` + 标记检测 + `Event::Wakeup`；
  2. 出：在 `write_to_pty`（`:2139`）里按 `terminal_type` 分派，覆盖面必须包含按键、鼠标（`:2683`/`:2781`）、alternate scroll（`:2832`/`:2841`）、焦点（`:2470`/`:2476`）、`PtyWrite`（`:1672`）、特性探测应答（`:1696`）、颜色应答（`:1723`）；`TextAreaSizeRequest`（`:1672-1675`）**不由客户端回答**，改由服务端；
  3. cwd：新增 `set_remote_working_directory`，`working_directory()`（`:2889`）在 remote 时返回它；
  4. 尺寸：`set_size`（`:2114`）后仅 controller 上报。
  同时删除 `clone_builder`（`:3210-3229`，当前无调用点，daemon 化后是绕过属主的死路径）。验证：`cargo test --test terminal_replay` 与新增单元测试；逐类 VT 事件注入后断言"远程出口收到对应字节"（缺任何一类都算未完成）；`git diff --stat crates/terminal/src/terminal.rs` 不含渲染/选择/vi 相关改动。
- [x] 4.2 扩展 `tests/terminal_replay.rs`：同一段固定字节分别经"本地 PTY 直连"与"远程回放路径"进入 VT，断言画面与 `Modes` 一致；夹具必须包含 raw 模式裸 `\n` 片段，用来证明两条路径的字节保真一致。验证：`cargo test --test terminal_replay`。
- [ ] 4.3 `crates/crossh-terminal-view/src/view.rs`：会话建立从 spawn PTY 改为向 daemon 申请会话并接入；按键走新出口，resize 只在持有输入权时上报；失去输入权时显示只读标记。验证：本机跑 GUI，确认键入、粘回显、`Ctrl-C`、窗口缩放后 `stty size` 与窗口一致；`crossh ls` 从 CLI 侧抢走输入权后 GUI 出现只读标记。
- [ ] 4.4 `src/features/workspace/state.rs`、`registry.rs`：`LocalSession` 增加 daemon 会话 id 并与视图绑定；固定标签（`pinned.rs`）按 id 恢复而不是按目录重开。验证：固定两个标签 → 关闭 GUI → 重新打开，两个标签回到各自会话；`cargo test --bin crossh` 补一条按 id 恢复的纯逻辑测试。
- [ ] 4.5 退出与结束语义拆分（`src/features/workspace/shell/quit.rs`）：关窗口与退出 GUI 都不再 `begin_shutdown`/`request_close`；有命令在跑时不再弹关闭确认；"结束会话"改为向 daemon 发 close 请求并断开连接，且必须穷举 GUI 侧调用点——`crates/crossh-terminal-view/src/view.rs:789` 的 `request_close`、`shell/tabs.rs:208/217`、`shell/split.rs:127`、`tab_strip.rs:301/373`、`shell/mod.rs:365`、`quit.rs:135/151`。关闭标签仍保留"有命令在跑"的确认（它确实会结束会话）。验证：起长命令 → 关窗口 → GUI 退出、命令仍在该会话中运行（重新打开可见）；关标签有确认；`cargo test --bin crossh quit` 断言"关窗口不需要确认"。
- [x] 4.6 记录工程笔记 `docs/engineering-notes/session-daemon-protocol.md`：帧格式、`TCP_NODELAY` 与小帧风暴、回放入口与边界帧、`write_to_pty` 两类写入的划分、令牌与权限、detached 启动并更新 `docs/engineering-notes/README.md` 索引行。验证：按索引关键词能在不读代码的情况下解释一次重连失败的排查路径。
- [x] 4.7 孤儿认领（新增）：`activate_local_dir` 后认领 daemon 里活着、无输入者、未被任何标签/pin 认领的会话，按原 cwd 接回本项目（只收本项目子树）。验证：杀 GUI 前不 pin → 重启后原会话自动出现；`crossh ls` 角色变为 controller。

- [x] 4.8 重放抑制：接入重放旧查询（OSC 11/`CSI?$p`/DA/特性探测）不回写，`Attached→ReplayDone` 窗口由 `Terminal::replaying_remote` + 共享 `reply_gate` 在发射侧（`ZedListener::send_event`）门控（消费侧有竞态，见工程笔记），实时查询不受影响；`ReplayDone` 后抖一下尺寸（+1 再恢复）逼 shell 重画，认领首帧不错位、无需回车。验证：`remote_replay_suppresses_stale_query_replies` + 认领旧 TUI 会话不再向 shell 灌垃圾。
## 5. CLI 第二前端

- [x] 5.1 `src/app/cli.rs`：新增 `daemon status|start|stop` 与 `ls|run|capture` 子命令解析，同步更新 `help_text`（`:70-83`）、**遮蔽名单**与测试 `shadowed_names_need_dot_slash_or_separator`（`:140`）——新增的四个名字会再遮蔽同名目录；版本不匹配的拒绝文案必须直接给出 `crossh daemon stop`。验证：`cargo test --bin crossh cli` 覆盖每个子命令的解析、错误分支与新的遮蔽名单。
- [x] 5.2 CLI 客户端实现：按需拉起 daemon、令牌握手、`ls` 输出会话表并显示每个会话当前的角色（controller/observer/无）、`run -- <cmd>` 转发输出并原样返回退出码、`capture` 输出指定会话的回放字节。验证：`crossh-daemon` 未运行时直接 `crossh run -- echo hi` 成功且退出码 0；`crossh run -- sh -c 'exit 7'` 的进程退出码为 7；两个客户端同时接入时 `crossh ls` 的角色列正确。
- [x] 5.3 `daemon.port` 与主窗口投递**互不影响**：`instance.port`、`git.port`、`note.port` 及其解析路径一律不动，`crossh <path>` 语义保持不变（本变更不改它）。验证：`rg -n "instance.port|port_file_path\\(|try_forward\\(" src crates` 与本变更前一致（除新增 daemon 相关文件）；`crossh <path>` 行为与合并前逐字相同（GUI 已运行与未运行两种情况各测一次）。
- [x] 5.4 CLI e2e 放进 `crates/crossh-session/tests/`（与 3.4 同目录），以便拿到 `CARGO_BIN_EXE_crossh-daemon`；断言"列空 → 建会话 → 跑命令拿输出与退出码 → 结束会话后列表为空"的闭环以及两个前端看到同一段输出与退出状态（在回放上限之内）。验证：`cargo test -p crossh-session --test cli_sessions`。
- [x] 5.5 `README.md` 更新：会话常驻、CLI 入口、`crossh daemon stop` 的后果；`docs/architecture.md` 的进程角色段落同步。验证：按 README 步骤在干净目录下走一遍（daemon 未运行起步）。

## 6. 集成验证

- [x] 6.1 `cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`scripts/check-architecture.sh` 全绿。
- [x] 6.2 定向测试：`cargo test -p crossh-session`、`cargo test -p crossh-core`、`cargo test --bin crossh`、`cargo test --test terminal_replay`、`cargo test -p crossh-session --test cli_sessions`（按 `AGENTS.md` 不跑 `--workspace` 全量）。
- [ ] 6.3 macOS 实机冒烟：关窗口后长命令继续；重开回到原会话；GUI + CLI 同时接入时 CLI 只观察、发送一次输入即接管；`ALT_SCREEN` 程序在 observer 窗口重排错位（D8 已知 artefact，判定为可接受而非回归）；`crossh daemon stop` 后会话消失且下次调用重新拉起；**从终端启动 crossh → 关窗口 → 关闭该终端 → 会话仍在**（验证 detached）。
- [x] 6.4 `openspec validate session-daemon --strict` 通过。
- [ ] 6.5 本机不跑 Windows/Linux 目标构建；两者的 daemon 与协议行为由 PR CI（Windows 与 Linux 各跑 `cargo test -p crossh-session`）承接，绿灯后回填。

## Workflow follow-up

- `openspec/changes/session-daemon/review.md` 的 C1–C8 必须在进入 apply 前全部关闭并在该文件逐条标记。
- 归档前确认 6.5 的 CI 结论已回填。
- 后续变更（不在本变更内）：托盘、web 前端、daemon 热升级与会话交接、远程主机、自动重启版本不匹配的 daemon。

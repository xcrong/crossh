# Proposal

## Why

crossh 的 daemon 目前只有会话，没有项目与布局。会话表是一张扁平映射（`crates/crossh-session/src/serve.rs:61` 的 `Sessions = Arc<Mutex<HashMap<String, Handle>>>`）；项目、tab、分屏、比例、焦点全部由 GUI 在内存里维护（`src/features/workspace/state.rs:24` 的 `LocalSession`、`:66` 的 `LocalDir`，`src/features/workspace/registry.rs:190` 的 `terminal_splits`）。GUI 一退出，这棵树就消失，重启后只能按 live cwd 重新分组（`src/features/workspace/shell/tabs.rs:863` 的 `adopt_orphan_daemon_sessions`）。同一个项目里 `cd` 到不同目录的 pane 会被摊成多个项目，分屏会被摊成多个 tab。

旧设置里能留下的只有两类线索：`recent_local_dirs`（`src/features/workspace/settings.rs:34`）与 `pinned_local_tabs`（`:38`）。后者是 `PinnedLocalTab`（`settings.rs:15`，字段 `:16-26`），字段为 `pin_id`（`:16`）、`project_dir`（`:17`）、`cwd`（`:18`）、`custom_name`（`:20`）、`default_command`（`:22`）、`daemon_session_id`（`:26`）——它记录的是会话 id 而不是布局身份，daemon 重启后全部失效；`custom_name` 与 `default_command` 也因此没有可靠的承载体。

布局是用户工作状态的一部分，它的生命周期却比 GUI 短。继续在 GUI 侧增加恢复启发式只能缩小误差，无法让“项目 ↔ tab ↔ pane”在重启后保持原样。把 Workspace 树变成 daemon 的持久状态，可以从根上消除这类推断。

## 前置条件

- `openspec/changes/session-daemon/` MUST 先 apply；本变更的代码实施可在其归档前开始，但本变更归档前 `session-daemon` MUST 已归档并合入 `session-host` 主规格。`session-daemon` 的主规格仍要求“固定标签重新接入各自的同一个会话”；本变更删除该机制，因此本变更的 `specs/session-host/spec.md` 是一条 MODIFIED delta，必须在该归档之后合入主规格。
- 本变更取代 `session-daemon` 的两项遗留声明：`tasks.md:41`（4.4“固定标签按会话 id 恢复”）在本变更删除 `pinned.rs`、`pinned_local_tabs` 与 `daemon_session_id` 后无法完成，`session-daemon` 归档前应标记取消，由本变更的布局持久化与迁移任务承接；`review.md:231`（C7）“`crossh <path>` 语义逐字不变”被本变更改写为 `crossh open <path>` 先在 daemon 中打开项目、再唤起 GUI（见 design D13）。
- 本变更不依赖“版本不匹配时自动重启旧 daemon”。`session-daemon` 把这条留作后续变更；本变更改为显式的兼容性错误与可执行的 `crossh daemon stop`（见 design D13）。

## What Changes

- **daemon 持有工作区。** 项目、tab、递归分屏树、比例、焦点、活跃位置、pane 的 cwd 与尺寸成为 daemon 的持久状态；GUI 只消费快照。
- **布局身份与运行时会话分离。** daemon 生成并持久化稳定 ID，session 身份只活在 daemon 进程内；daemon 重启后布局不变、会话重建。
- **修改有回执。** 每个工作区修改返回本次结果、revision 与新建 ID，并标明是否已落盘；结构变更在回执前完成原子写入与 fsync（写入失败时回执标记未落盘），比例/焦点/cwd/尺寸延迟合并；格式版本高于当前版本时拒绝启动、保护文件并留下可被 `crossh daemon status` 读取的原因。
- **物化按需发生。** daemon 启动不创建 shell；首个客户端 attach 后物化活跃 tab，非活跃 tab 在激活时物化。
- **旧设置由 daemon 一次导入。** 迁移在 daemon 启动、开始服务前完成，不依赖 GUI、不依赖工作区为空；导入后旧字段不再被读取。
- **GUI 变成工作区投影。** 布局所有权、按 cwd 猜项目、按会话 id 恢复固定标签、孤儿会话认领全部删除；`ActiveView`、`LocalDir`、`TerminalSplitState` 退出布局语义，活动键改为 `PaneId`。
- **CLI 跟随工作区。** `crossh open` 先写工作区并等待回执，再确保 GUI；`crossh ls` 保留输入权角色列并追加层级列；`crossh daemon status` 追加工作区计数。
- **单实例通道只唤起窗口。** 转发载荷不再携带目录；项目激活只发生在 daemon 中（由 `crossh open` 发起），`src/infrastructure/app_menu.rs` 只负责显示窗口。
- **BREAKING**：协议升到 v2，旧 daemon 被拒绝；固定标签与最近目录由工作区取代；`crossh daemon stop` 结束会话但保留布局。
- **明确不改**：daemon 仍只持 PTY 与回放环，不做 VT 网格；`crossh git` / `crossh note` 的端口与路由不变；多窗口独立活跃视图、远程主机、daemon 热升级与 PTY 交接不在本变更内。

## Capabilities

### New Capabilities

- `workspace-state`: 定义 daemon 持有的工作区状态契约——项目、tab、pane 与分屏树的稳定身份，持久化与恢复语义，修改回执与并发仲裁，pane 与 session 的生命周期关系，运行状态与错误的可观察字段，以及旧设置的一次性迁移。

### Modified Capabilities

- `session-host`：MODIFIED delta，改写“重新打开后回到原会话”这一固定标签场景，改为“布局恢复、仍在运行的会话被重新接入、已结束的会话在 pane 上重建”。该 delta 以 `session-daemon` 归档为前提。

## Impact

- `crates/crossh-session/src/`：新增 `workspace.rs`、`workspace_store.rs`、`workspace_actor.rs`（或等价拆分）与 `serve.rs` 的 actor 接入；`protocol.rs`、`handshake.rs`、`client.rs`、`instance.rs` 增加 v2 消息、回执、归属字段、配置根与旧 daemon 停止路径。
- `crates/crossh-core/src/`：`config/mod.rs` 新增共享 `config_root()`/`settings_path()`/`settings_read_path()`/`workspace_path()`（`settings_read_path()` 承担 debug 旧路径的只读回退）；`locale.rs:41-43` 的 `settings_path()` 改调同一解析器；`single_instance.rs` 的主窗口转发改为无载荷（git/note 的目录语义不变）。
- `src/features/workspace/`：`state.rs`、`registry.rs`、`shell/mod.rs`、`shell/tabs.rs`、`shell/split.rs`、`shell/quit.rs`、`tab_strip.rs`、`sidebar.rs`、`empty_state.rs`、`view.rs`、`command_palette.rs`、`local_paths.rs` 改为快照投影与 daemon 操作。
- `src/features/workspace/settings.rs`、`src/features/settings/persistence.rs`：删除 `recent_dirs` / `recent_dirs_max` / `pinned_local_tabs`；settings 路径改走 `crates/crossh-core` 的共享解析器。
- `src/app/cli.rs`、`src/app/single_instance.rs`（re-export）、`src/infrastructure/app_menu.rs`、`src/main.rs`：`crossh open` 改走 daemon 并等待回执，单实例转发只唤起窗口，`crossh ls` / `daemon status` 输出工作区信息与层级列，帮助文本与遮蔽名单同步。
- `crates/crossh-terminal-view/`：`daemon_link.rs` 的接入与结束路径跟随 pane 归属；`view.rs` 的 `request_close` 在布局 pane 上改发工作区操作。
- 文档：`docs/architecture.md`、`docs/engineering-notes/session-daemon-protocol.md`、`README.md`、发布说明。
- 本机验证受磁盘限制约束：实现阶段只做源码与测试编写，编译、测试与多平台行为由 CI 承接。

## 发布与回滚

- 升级前必须备份实际存在的旧 settings：release 与旧 debug 都写在 `~/.config/crossh/settings.toml`（`CROSSH_CONFIG_DIR` 设置时以它为准）。新版 release 读同一路径；新版 debug 读 `~/.config/crossh-dev/settings.toml`，该文件不存在时**只读回退**读取 `~/.config/crossh/settings.toml`，因此旧 debug 设置也会被导入 `crossh-dev` 工作区。迁移会读取源文件的 `recent_local_dirs` 与 `pinned_local_tabs`，并在实际读取的源文件旁留下只读快照 `settings.toml.pre-workspace`；旧 debug 文件不会被新版写入（release 的写入落在同一路径，首次保存前已生成快照）。
- 回滚到旧版本后，旧 GUI 不读 `workspace.json`，且旧 settings 中的 `recent_local_dirs` / `pinned_local_tabs` 会在新版首次保存设置时消失：最近目录与固定标签都无法自动恢复，只能从备份还原。发布说明必须写清这一点。
- `crossh daemon stop` 在升级后第一次运行时会停止旧 daemon（含 v1），不会顺手拉起第二个 daemon；被停止的 daemon 中的会话结束，布局保留。

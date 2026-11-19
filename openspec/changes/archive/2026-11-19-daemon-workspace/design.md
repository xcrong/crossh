# Design

## Context

动机见 `proposal.md - Why`，行为契约见 `specs/workspace-state/spec.md`。这里只列决定方案所必需的现状。

- daemon 的会话状态是一张扁平映射：`crates/crossh-session/src/serve.rs:61` 的 `Sessions = Arc<Mutex<HashMap<String, Handle>>>`，没有项目、tab、分屏字段；`SessionInfo` 由 `serve.rs:81-90` 的 `info_of` 在持有会话锁时构造。
- GUI 才是布局属主：`src/features/workspace/state.rs:24` 的 `LocalSession` 持有 `project_dir`（`:26`）、`cwd`（`:28`）、`Entity<TerminalView>`（`:29`）、`git_status`（`:30`）、`pin_id`（`:33`）、`custom_name`（`:35`）、`default_command`（`:37`）与 `daemon_session_id`（`:39`）；`:66` 的 `LocalDir` 按 `project_dir` 分组；`state.rs:20` 的 `ActiveView` 是工作区渲染与焦点的唯一键。`src/features/workspace/registry.rs:190-195` 的 `WorkspaceState` 以 `ActiveView` 为键持有 `terminal_splits`、`compose` 与三组尺寸。
- 重启恢复靠推断：`src/features/workspace/shell/tabs.rs:863` 的 `adopt_orphan_daemon_sessions` 列出 daemon 中活着且无 controller 的会话，按 live cwd 决定 `project_dir`；`src/features/workspace/shell/mod.rs:893` 的 `sync_local_dirs` 在每次 cwd 变化后重新分组。
- 持久化只有两份 GUI 侧线索：`src/features/workspace/settings.rs:34` 的 `recent_dirs`（serde rename 为 `recent_local_dirs`）与 `:38` 的 `pinned_local_tabs`；`PinnedLocalTab`（`settings.rs:15`）保存 `pin_id`（`:16`）、`project_dir`（`:17`）、`cwd`（`:18`）、`custom_name`（`:20`）、`default_command`（`:22`）与 `daemon_session_id`（`:26`），其中 `daemon_session_id` 在 daemon 重启后失效。`src/features/workspace/shell/mod.rs:226-241` 在 `AppShell::new` 启动时就归一化 `recent_dirs`、修剪 `pinned_local_tabs` 并立即写回 settings。
- daemon 已经能拿到 cwd：`crates/crossh-session/src/feed.rs:33` 的 `osc7: Osc7Scanner` 字段（扫描器定义在 `:116`）从 PTY 输出解析 OSC 7，`SessionMeta.cwd`（`session.rs:139` 结构体、`cwd` 字段在 `:141`）在 `List`/`Attach` 前由 `serve.rs:93` 的 `refresh_meta` 刷新。布局层只需要消费这条已有事实。
- 协议版本为 1（`handshake.rs:10`），`MessageKind` 枚举起于 `protocol.rs:17`，`Shutdown = 16` 在 `protocol.rs:49`，结构化载荷用 `serde_json`，帧上限 8 MiB。
- 升级路径目前是错的：`client.rs:162` 的 `ensure_daemon` 用当前版本 `probe`（`client.rs:130`）判活，版本不匹配会被当作探活失败，随后删除 `daemon.port` 并 `spawn_detached`（`client.rs:99`）；`client.rs:456` 的 `daemon_stop` 先调 `ensure_daemon`，再用当前版本握手，旧 daemon 在 `handshake.rs:93-98` 拒绝后只得到 `PermissionDenied("handshake failed")`。`serve.rs:329-333` 的 `Shutdown` 在删除端口文件后立即 `process::exit(0)`。
- 配置根不一致：settings 走 `dirs::home_dir().join(".config/crossh/settings.toml")`（`src/features/settings/persistence.rs:97-102`），daemon 缓存走 `dirs::cache_dir()` 加 debug 后缀 `-dev` 并支持 `CROSSH_CACHE_DIR`（`instance.rs:24-31`）；`CROSSH_CONFIG_DIR` 目前不存在。
- 单实例通道仍传目录：`src/main.rs:129` 的 `try_forward(initial_dir)` 与 `src/infrastructure/app_menu.rs:223-231` 的 `activate_project` → `AppShell::activate_local_dir`（`shell/mod.rs:579`）会绕过 daemon 直接开本地会话。

## Goals / Non-Goals

**Goals**

- 让项目、tab、分屏树、比例、焦点与活跃位置在 GUI 重启和 daemon 重启后都保持稳定。
- 把布局身份与运行时 session 身份彻底分开，删除按 cwd 猜项目、按会话 id 恢复标签这两类启发式。
- 让 GUI、CLI 与任何未来前端消费同一份工作区快照，并由 daemon 串行仲裁修改、逐条给出可判定结果。
- 在 daemon 崩溃或停止后保留布局，按 pane 的 cwd 与尺寸重建会话。

**Non-Goals**

- 不做 daemon 崩溃后的进程恢复、PTY 交接或 `execve` 热升级；布局活着，进程重建。
- 不做 VT 网格协议；daemon 仍然只持 PTY、回放环与工作区元数据。
- 不做多窗口的独立活跃视图；活跃项目/tab/pane 是工作区级状态。
- 不做远程主机、web 前端、托盘改造。
- 不改 `crossh git` / `crossh note` 的端口与路由；继续使用现有 `instance.port` / `git.port` / `note.port`。
- 不保留旧协议的兼容分支；协议 v2 与旧 daemon 明确拒绝。

## Decisions

### D1 Workspace 模型：持久树 + 运行时视图分离

新增 `crates/crossh-session/src/workspace.rs`，零 `gpui`：

```rust
pub struct Workspace {
    pub format_version: u32,   // 当前 1
    pub revision: u64,         // 持久化，跨重启单调
    pub next_id: u64,
    pub active_project: Option<ProjectId>,
    pub projects: Vec<Project>, // 顺序即侧栏顺序
    pub legacy_imported: Option<LegacyImport>,
}

pub struct Project {
    pub id: ProjectId,
    pub root: PathBuf,
    pub name: Option<String>,
    pub last_opened_at: u64,   // 仅展示/迁移参考，不参与排序
    pub active_tab: Option<TabId>,
    pub tabs: Vec<Tab>,
}

pub struct Tab {
    pub id: TabId,
    pub title: Option<String>,
    pub default_command: Option<String>,
    pub active_pane: Option<PaneId>,
    pub layout: PaneNode,
}

pub enum PaneNode {
    Leaf(Pane),
    Split { id: SplitId, axis: Axis, ratio: f32, first: Box<PaneNode>, second: Box<PaneNode> },
}

pub struct Pane {
    pub id: PaneId,
    pub cwd: PathBuf,
    pub size: TermSize,
}
```

`available`、`PaneSnapshot.session`、`cwd_available`、pane 运行状态与 `storage_error` 是运行时字段，只出现在快照中，不落盘。项目顺序只由 `Vec<Project>` 决定：打开已有项目移到最前，新项目插到最前（与 `spec` R「布局操作的目标与默认值明确」一致）；`last_opened_at` 只用于展示与迁移参考，不参与排序，避免出现第二套排序依据。

否决“在 GUI 侧把 `TerminalSplitState` 序列化到 settings”：那样能恢复分屏形状，但项目归属、tab 顺序、活跃位置与 session 绑定仍然由 GUI 推断；多前端也无法共享同一份布局。

### D2 布局身份持久、会话身份运行时

持久身份：`ProjectId`、`TabId`、`PaneId`、`SplitId`，由 `next_id` 单调分配并写入工作区文件。运行时身份：`SessionId`（现有字符串 id），由 daemon 在物化 pane 时生成，不写文件。

```rust
pub struct Runtime {
    pub pane_session: BTreeMap<PaneId, SessionId>,
    pub session_pane: BTreeMap<SessionId, PaneId>,
    pub starting: BTreeSet<PaneId>,  // 物化进行中，防重复 spawn
    pub attached_clients: usize,     // 已 attach 的客户端数；0 表示无人观看
}
```

`Pane` 的 `cwd` 与 `size` 落盘；session 不落在 `Pane` 上。daemon 重启加载出的是一棵没有 session 的树，GUI 看到的也是同一棵。会话退出只清运行时映射；只有在“该 pane 应随 shell 退出而关闭”时才改树；daemon shutdown 不改树。

否决“把 session id 写进 workspace 文件”：session id 在 daemon 重启后没有意义，写进去只会复现当前固定标签的失效路径。

### D3 递归分屏树，不保留 2×2

当前 GUI 固定 2×2（`registry.rs:185-195` 的 `left/right/bottom_left/bottom_right` 与三组尺寸）。daemon 使用递归 `PaneNode`：每次分屏把目标叶子替换为 `Split`，关闭时把父 `Split` 替换为兄弟子树。

理由：比例、焦点、关闭折叠只需要一套规则；2×2 是 UI 限制而不是产品语义，写进持久化格式会让未来任意分屏再做一次迁移；GUI 递归渲染的增量可控（`Split` 用 `flex_row`/`flex_col` + 分隔条）。

旧 2×2 布局从未持久化，升级时不会迁移任何 GUI 内存布局；但模型必须能表达它，任务 5.1 用纯函数把 `TerminalSplitState` 的四个槽位映射为 `Split(Row, Split(Column, left, bottom_left), Split(Column, right, bottom_right))` 并断言遍历顺序与焦点顺序一致。这个测试验证的是模型表达能力，不是迁移路径。

否决“持久化固定 2×2”：实现更小，但 daemon 契约会被 UI 限制污染。

### D4 Workspace actor：命令集、效果与锁顺序

新增 `WorkspaceActor`，独占 `Workspace` 与 `Runtime`，通过 `mpsc` 串行处理命令：

```text
// 用户操作入口（WorkspaceOp 解码后分派到同一组 handler）
Mutate { request_id, expected_revision, op, reply }
// 前端读取
Get { reply }                  // 只读；重新计算可用性
Attach { reply, subscribe }    // 返回当前快照 + watch 订阅；物化活跃 tab 中可物化的 pane（D10）
// 运行时命令（daemon 内部，不是用户操作）
AttachEnded                    // 订阅连接结束：attached_clients 递减
PersistenceResult { revision, result }  // writer 报告写入成功或失败（D7）
BindSession { pane_id, session_id }
UnbindSession { pane_id }
SessionSpawnFailed { pane_id, reason }
SessionExited { session_id }
PaneCwdChanged { session_id, cwd }
ResizePane { session_id, size }
ClosePane { pane_id }
CloseTab { tab_id }
SetPaneCwd { pane_id, cwd }
RefreshAvailability { reply }
Shutdown { reply }
```

- `WorkspaceOp` 的用户操作与运行时命令共用同一组纯函数 handler；`ClosePane`/`CloseTab`/`SetPaneCwd`/`RefreshAvailability` 既能由 `Mutate` 分派，也能由 daemon 内部直接投递（连接任务的 `Close`、窗口重新聚焦）。
- actor 不直接操作 PTY。它把副作用投递给运行时任务：

  ```text
  Spawn { pane_id, cwd, size }
  CloseSession { session_id }
  ResizeSession { session_id, size }
  ```

  物化在 actor 外执行：`Spawn` 成功后投 `BindSession`；失败投 `SessionSpawnFailed`。actor 在发出 `Spawn` 时把 pane 标为 `Starting` 并加入 `Runtime.starting`，重复请求直接返回当前状态，避免两次 spawn。

**`Close` 路由。** 连接任务收到 `Close` 帧时，先在归属镜像中查 `session_id → (project_id, tab_id, pane_id)`：

- 命中 → 投 `ClosePane { pane_id }` 给 actor；actor 先解绑（清 `Runtime` 映射、写归属镜像、把 pane 状态改为休眠），更新布局（折叠 split、必要时删 tab），完成结构持久化，再把 `CloseSession { session_id }` 作为效果投回运行时任务，最后回执（顺序见 D5/D6）。
- 未命中 → 该会话不属于布局（`crossh run` 的临时会话），走现有 kill 路径。

自然退出走 `SessionExited`：先查 `session_pane`；显式关闭已解绑的会话找不到映射，直接忽略，重复事件不会二次删 pane。`shutting_down` 置位后忽略 `SessionExited`。

**`Resize` 与 `SetSplitRatio` 的职责差异。** `Resize` 只由 controller 发起（controller 判定仍在会话所有权里）：连接任务校验后投 `ResizePane { session_id, size }`，actor 写 `pane.size` 并 debounce 持久化，再通过 `ResizeSession` 效果异步转发 PTY resize。`SetSplitRatio` 只改 `Split.ratio`，不触碰 PTY，也不写 `pane.size`。

**锁顺序（MUST）。**

1. `Sessions`（`tokio::sync::Mutex`）只在连接任务/运行时任务内短持；持锁期间 MUST NOT 向 actor 发命令或等待回复。
2. `List`/`Attach` 在会话锁内只构造 `Vec<SessionInfo>` 快照（克隆 `SessionMeta` 与 controller），释放锁后再从归属镜像合并 `project_id`/`tab_id`/`pane_id`。`info_of`（`serve.rs:81`）不再在锁内拼接归属字段。
3. 归属镜像 `Arc<std::sync::RwLock<BTreeMap<SessionId, PaneRef>>>` 只由 actor 在 `BindSession`/`UnbindSession` 中写入；读者先克隆条目再释放读锁，持读锁期间 MUST NOT 调用 actor（否则 actor 的写锁会与等待回复的读者互等）。
4. actor MUST NOT 获取 `Sessions` 锁；关闭与 resize 一律通过效果投递。

由此锁的获取方向固定为 `Sessions → OwnershipMirror`，且任何锁都不跨 actor 调用，不存在反向等待。

**可用性刷新触发。** actor 在 `Get`、`Attach`、`ActivateProject` 与显式 `RefreshAvailability` 时重新计算每个项目的 `available`（`root.is_dir()`）与 dormant pane 的可物化性；布局未变时 revision 不变，但仍重新发布快照（见 D6 的快照应用规则），因此目录恢复后不需要任何布局操作就能反映。

否决“在 `serve.rs` 的连接处理里直接改 `Workspace`”：那会把布局变更、持久化、广播与 PTY spawn 分散在多个连接任务里，容易出现锁顺序与部分应用。

### D5 操作 schema

`WorkspaceOp` 是唯一的工作区修改入口，serde 载荷逐项定义如下。目标不存在一律返回 `Rejected("not_found")`；`project_id` 缺省时取活跃项目。

| 操作 | 字段 | 目标与前置条件 | 默认值 / 副作用 |
| --- | --- | --- | --- |
| `OpenProject` | `root: PathBuf` | 无 | 根目录做词法规范化（绝对化、去 `.`/`..`，存在时 canonicalize，缺失时保留词法结果）后按规范化根去重。已存在 → 激活并保证至少一个 tab（没有则建一个 tab + 单 pane，cwd 为项目根）；新项目且根存在 → 插到项目顺序最前并建一个 tab + 单 pane（cwd 为项目根）；根缺失 → 建 `available=false` 的项目 + 一个 tab + 单 pane（状态按判定表为 `Unavailable(project_root_missing)`）；有 attach 客户端时物化新建或新激活的可物化 pane（D10） |
| `StopProject` | `project_id` | 项目存在 | 解绑并结束该项目全部会话、移除全部 tab、保留项目条目（副作用顺序见下文） |
| `ForgetProject` | `project_id` | 项目存在 | 解绑并结束会话、移除项目与布局；若它是活跃项目，活跃项目改为顺序最前的另一个项目（副作用顺序见下文） |
| `ActivateProject` | `project_id` | 项目存在 | 只切换活跃项目与 `active_tab`（空项目为 `None`）；MUST NOT 创建 tab |
| `CreateTab` | `project_id: Option<ProjectId>`、`title: Option<String>`、`cwd: Option<PathBuf>`、`default_command: Option<String>` | 目标项目存在；无活跃项目且未给 `project_id` → `Rejected("no_active_project")` | cwd 默认取该项目活跃 pane 的 cwd，否则项目根；创建 tab + 单 pane；有 attach 客户端时物化（D10） |
| `CloseTab` | `tab_id` | tab 存在 | 解绑并关闭其全部 pane；激活后一个 tab，没有则前一个；项目保留（副作用顺序见下文） |
| `ActivateTab` | `tab_id` | tab 存在 | 同时设 `active_project` 与该 tab 的 `active_pane` |
| `ReorderTabs` | `project_id`、`tab_id`、`before_tab_id: Option<TabId>` | tab 属于该项目；`before_tab_id` 为 `None` 表示移到末尾 | 位置变化才算 `Applied` |
| `SplitPane` | `pane_id`、`axis: Axis`、`cwd: Option<PathBuf>` | pane 存在 | cwd 默认继承目标 pane 的 cwd；`ratio = 0.5`；建 `SplitId` + `PaneId`；有 attach 客户端时物化（D10） |
| `ClosePane` | `pane_id` | pane 存在 | 折叠父 split；焦点移到兄弟子树第一个 leaf；tab 空则删 tab |
| `ActivatePane` | `pane_id` | pane 存在 | 同时设 `tab.active_pane`、`project.active_tab` 与 `workspace.active_project` |
| `SetSplitRatio` | `split_id`、`ratio: f32` | split 存在 | clamp 到 `0.05..=0.95`；只改布局 |
| `SetPaneCwd` | `pane_id`、`cwd: PathBuf` | pane 必须是 `Dormant` 或 `Unavailable`；`Starting` → `Rejected("pane_starting")`，`Live` → `Rejected("pane_live")`（即使项目根或 cwd 已缺失，live 仍拒绝） | cwd 缺失也接受（保留原值并保持 `Unavailable(cwd_missing)`） |
| `EnsurePaneSession` | `pane_id` | pane 存在 | `Live`/`Starting` → `Unchanged`；`Dormant` 且 `attached_clients > 0` → `Applied`（runtime-only，revision 不变，见 D6），pane 转 `Starting` 并投 `Spawn`；`Dormant` 且计数为 0 → `Unchanged`（保持 `Dormant`）；`Unavailable` → `Unchanged`（不尝试 spawn） |
| `RefreshAvailability` | 空 | 无 | 只重算运行时字段，不移动布局：运行时字段有变化 → `Applied`（revision 不变，见 D6）并重发快照；无变化 → `Unchanged`，但仍重发同 revision 快照 |
| `RenameTab` | `tab_id`、`title: Option<String>` | tab 存在 | `None` 或空白清除自定义名，回退到终端标题 |
| `SetTabCommand` | `tab_id`、`command: Option<String>` | tab 存在 | `None` 或空白清除 |

`RenameProject` 不新增：现有 UI 没有项目重命名入口（`rg 'rename_project|context_menu\.rename' src/features/workspace` 只命中 `context_menu.rename_tab` 与固定标签重命名弹窗），项目显示名继续取根目录 basename；新增一个没有入口的操作违反 YAGNI。

错误码固定为 `not_found`、`pane_starting`、`pane_live`、`no_active_project`、`invalid_target`、`invalid_payload`、`io_error`、`session_spawn_failed`。`Rejected` 回执携带 `code` 与人类可读的 `message`；协议层解析失败仍用 `MessageKind::Error`，两者不混用。持久化写失败不是操作错误：修改照常在内存生效，MUST NOT 变成 `OpError`；回执用 `durable`/`storage_error`（D6）与快照的 `storage_error` 表达“已应用但未落盘”。

**pane 运行状态判定。** `PaneStatus` 是快照里的运行时投影，不落盘。会话绑定是独立字段 `PaneSnapshot.session`，状态只描述运行阶段，按优先级从高到低判定，先命中者生效：

| 状态 | 判定条件 | 含义 |
| --- | --- | --- |
| `Live` | `Runtime.pane_session` 命中（此时 `PaneSnapshot.session = Some(id)`） | 会话已绑定；与项目根、pane cwd 是否可用无关 |
| `Starting` | 无会话，且已投递 `Spawn`、尚未 `BindSession` | 物化进行中，重复请求不重复 spawn |
| `Dormant { reason }` | 无会话，项目 `available = true`，pane cwd 存在 | 可物化（daemon 重启后、显式 `EnsurePaneSession` 前、上次物化失败） |
| `Unavailable { reason }` | 无会话，且项目 `available = false`（`project_root_missing`）或 pane cwd 不存在（`cwd_missing`） | 不可物化；cwd 原值保留 |

判定顺序 MUST 先看会话绑定：命中即为 `Live` 并给出 `session`，即使所属项目根或 pane cwd 已缺失——项目 `available` 与 `PaneSnapshot.cwd_available` 作为并列的警告字段暴露，MUST NOT 把有会话的 pane 降级为 `Unavailable`，否则客户端拿不到会话标识、无法重新接入仍在运行的会话。无会话时才依次看 `Starting`、可物化性。`Dormant.reason` 取 `not_started` 或 `spawn_failed`，记录最近一次未物化或物化失败的原因，不参与判定；`Unavailable.reason` 取 `project_root_missing` 或 `cwd_missing`。无会话且不可用时 MUST NOT 尝试 spawn。

**`StopProject` / `ForgetProject` / `CloseTab` 的副作用顺序。** 三者都按同一顺序执行，避免“会话已结束但布局仍指向它”的中间态：actor 先删除目标 pane 的运行时映射与归属镜像条目、更新布局并完成结构持久化（D7），再为每个仍 live 的 session 投递 `CloseSession` 效果，最后回执。`CloseSession` 只是投给运行时任务的异步效果，回执 MUST NOT 等待会话结束。迟到的 `SessionExited` 因映射已删除而被忽略（D4 的幂等规则）。因此回执携带的快照一定不含目标 pane，而会话结束 MAY 稍后完成。

**路径归一化必须与 GUI 的 `normalize_local_cwd` 分开。** `src/features/workspace/local_paths.rs:17-23` 只接受存在的目录，`normalize_recent_dirs`（`:26`）会丢弃缺失目录；工作区要求“缺失项目根保留、缺失 pane cwd 保留原值并标 dormant”，因此 `workspace.rs` 自带 `normalize_project_root` 与 `normalize_pane_cwd`：只做绝对化与词法清理，存在时再 canonicalize，不因缺失而丢弃。`local_paths.rs` 保留给 CLI 的输入解析与 `current_local_cwd`。

### D6 回执与 revision：可判定的并发语义

本变更把协议升到 v2，工作区消息都在这一版内；追加 `MessageKind::WorkspaceMutateReply = 21`：

```rust
pub struct WorkspaceMutateRequest {
    pub request_id: u64,
    pub expected_revision: Option<u64>,
    pub op: WorkspaceOp,
}

pub struct WorkspaceMutateReply {
    pub request_id: u64,
    pub outcome: MutateOutcome,     // Applied | Unchanged | Conflict | Rejected
    pub revision: u64,
    pub created: CreatedIds,        // project/tab/pane/split 各 Option
    pub error: Option<OpError>,     // Rejected/Conflict 时给出
    pub durable: bool,              // 本次 Applied 是否已落盘；其他 outcome 恒为 false
    pub storage_error: Option<StorageErrorInfo>,  // 写盘失败原因；仅结构变更写盘失败时出现
}
```

`durable` 与 `storage_error` 的语义固定为四种组合（`StorageErrorInfo` 见 D9）：

| 情形 | `outcome` | `durable` | `storage_error` |
| --- | --- | --- | --- |
| 结构变更，writer 已完成原子写入与 fsync | `Applied` | `true` | `None` |
| 结构变更已在内存应用，写盘失败 | `Applied` | `false` | `Some(StorageErrorInfo)` |
| 元数据变更（比例、焦点、cwd、尺寸），已入队 | `Applied` | `false` | `None` |
| `Unchanged` / `Conflict` / `Rejected` | 对应结果 | `false` | `None` |

`Applied` + `durable = false` + `storage_error = Some` 是结构变更已在内存应用但写盘失败的组合；客户端 MUST 对它显示“未落盘”，并由 writer 的重试补齐。`Applied` + `durable = false` + `storage_error = None` 表示元数据或仅运行时字段的修改已入队，不是失败，允许崩溃丢失最近 500 ms 的元数据。store 失败期间，客户端 MUST 结合快照的 `storage_error` 判断元数据修改当前未落盘，而不是只看回执。`Unchanged`/`Conflict`/`Rejected` MUST NOT 改变既有持久性，也 MUST NOT 携带 `storage_error`；它们的 `durable = false` 不表示未落盘，MUST NOT 展示为未落盘。`OpError` 仍只用于 `Rejected`/`Conflict`。

**revision 规则。** `revision` 只在**持久布局状态**（项目、tab、pane、分屏、顺序、名称、cwd、尺寸、焦点与活跃位置）实际变化时 `+1`。仅运行时字段（`available`、`PaneStatus`、`session`、`cwd_available`、`storage_error`）变化的操作 MUST 返回 `Applied`、revision 不变，并重发同一 revision 的快照：`EnsurePaneSession` 在休眠且可物化的 pane 上物化（仅 `attached_clients > 0`）、`RefreshAvailability` 发现变化、`BindSession`/`UnbindSession` 与 `PersistenceResult` 都属于这一类。`EnsurePaneSession` 对 `Live`/`Starting`、`Dormant` 且 0 attach、`Unavailable` 一律返回 `Unchanged` 且不 spawn；无变化的 `RefreshAvailability` 也返回 `Unchanged`。`RefreshAvailability` 与 `PersistenceResult` 是“revision 不变仍重发快照”的显式例外：无变化的前者也重发同 revision 快照。`OpenProject`/`CreateTab`/`SplitPane`/`ClosePane` 等创建或删除布局对象的操作按布局变化 `+1`。有效但无任何变化 → `Unchanged`，revision 不变；`Conflict`/`Rejected` 不变。actor 串行应用，未带 `expected_revision` 时最后写入者生效；带了就比较后应用，不匹配返回 `Conflict` 与当前 revision，客户端可以拉取新快照后重试。

时序保证：actor 应用 → 序列化 → 投 writer。结构变更必须等 writer 完成包含该 revision 的原子写入与 `fsync`（或明确报告写入失败）后才回执，再广播快照；元数据变更（比例、焦点、cwd、尺寸）入队后即可回执，写入由 writer 在 300 ms 内合并完成。回执与随后的快照携带同一 revision；两者由不同任务与不同 socket 写出，客户端 MUST NOT 依赖到达先后。请求方要确认“我的修改已生效”，判据是收到自己的回执且其 `outcome != Rejected/Conflict`，不是“看到 revision 前进”。

**writer → actor 的结果回路。** writer 每次写入尝试完成后向 actor 投 `PersistenceResult { revision, result }`（`Ok` 或 `Err(StorageErrorInfo)`，daemon 内部命令，不是用户操作）。actor 据此更新运行时的 `storage_error` 并重发同 revision 快照：失败时记录并重发，成功后清除并重发——清除同样 MUST 重发，否则客户端会一直显示旧错误。`PersistenceResult` 只改运行时字段，MUST NOT 推进 revision，也不改变布局，并遵循 D6 的同 revision 合并规则（只允许 `available`、`session`、`cwd_available`、`PaneStatus`、`storage_error` 变化）。一次写入覆盖多个待 ack 请求时，writer 仍逐个 ack，`PersistenceResult` 按 revision 报一次结果。

快照应用规则：`revision` 高于已知值时整份替换；等于已知值时只合并运行时字段（`available`、`PaneSnapshot.session`、`cwd_available`、`PaneStatus`、`storage_error`），布局字段必须与已知快照一致（违反视为服务端错误并丢弃该快照）；低于已知值时丢弃。同 revision 不变量由 actor 保证：`RefreshAvailability` 只重算运行时字段，不移动布局，因此不推进 revision。迟到的旧快照被忽略，可用性刷新也不会把客户端拉回旧焦点。

`crossh open` 依赖这条：发出 `WorkspaceMutate(OpenProject)` 后等待回执，`Applied`/`Unchanged` 才继续；超时（10 s）或 `Rejected`/`Conflict` 一律非零退出，不启动 GUI。

`StopProject`/`ForgetProject`/`CloseTab` 的顺序与 D5 一致：解绑 → 更新布局 → 结构持久化（fsync）→ 投递 `CloseSession` 效果 → 回执。回执 MUST NOT 等待会话结束；回执携带的快照不含目标 pane，会话结束 MAY 稍后完成，迟到的 `SessionExited` 被忽略。

否决“成功不单独回包、靠 revision 前进确认”：两个客户端并发时 revision 前进可能来自对方；no-op 操作则永远不会前进，调用方只能超时。

### D7 持久化：writer 串行、耐久性边界与格式保护

路径见 D8。文件内容是 `Workspace` 的 JSON，`format_version = 1`，`revision` 一并落盘，保证 revision 跨重启单调。

写入流程（`workspace_store.rs`）：

1. actor 在内存中序列化为 `Vec<u8>`（整棵树，规模是单用户量级），载荷带上本次 `revision` 与可选的 ack 请求 id。
2. 结构变更（`OpenProject`、`StopProject`、`ForgetProject`、`CreateTab`、`CloseTab`、`ActivateTab`、`ReorderTabs`、`SplitPane`、`ClosePane`、`ActivatePane`、`RenameTab`、`SetTabCommand`）投 `Immediate` 并要求 writer ack；比例、焦点、cwd、尺寸投 `Debounced`（只保留最新一份，300 ms 后写），不要求 ack。
3. 单一 writer task 串行写：临时文件 → `sync_all` → 主文件改名为 `workspace.json.bak` → `rename(tmp, main)` → Unix 上目录 `sync_all`。写入进行中到达的新载荷覆盖待写槽（整份状态，天然合并）；但**带 ack 的结构请求不得被后续 debounce 载荷吞掉而未 ack**。writer 为每个带 ack 的请求在其 `revision` 已包含于某次完成写入后单独 ack，可以合并成一次写、不能跳过 ack。
4. 写失败 → writer 投 `PersistenceResult::Err(StorageErrorInfo)`；actor 设置 `storage_error`（快照可见）并重发同 revision 快照。writer 保留最新载荷并按 1 s → 2 s → 4 s … 30 s 退避重试；成功后投 `PersistenceResult::Ok`，actor 清除错误并再次重发同 revision 快照。期间内存继续服务，下一次成功写入包含期间全部修改。结构写入失败时 actor 仍回执 `Applied`（修改已在内存生效），但回执 `durable = false` 且携带同一个 `storage_error`（D6），客户端 MUST 把该修改显示为未落盘；重试成功后错误清除，期间修改随下一次成功写入落盘。
5. 关闭：actor 置 `shutting_down`，取消 debounce、把待写载荷 flush 并等待 writer 的 ack（最长 5 s），再投递关闭会话的效果，最后删除端口文件退出。`serve.rs:329-333` 现有的“删端口文件后立刻 `process::exit(0)`”必须改成这条路径。

耐久性边界：已回执的结构变更在回执前已完成 `fsync`，MUST NOT 丢失；唯一例外是写入失败时携带 `storage_error` 的回执，它明确表示未落盘并由重试补齐。未回执的结构变更等价于没有发生。正常停止或显式 flush 后完整；SIGKILL/崩溃最多丢失最近 500 ms 的布局元数据（300 ms debounce + 一次写入的时间）。规格里的 MUST 按这个边界书写，不承诺崩溃后仍能恢复最后一帧比例。

`format_version` 高于当前版本：加载返回 `FormatTooNew { found, current }`；daemon **不**改写或重命名该文件、不发布端口文件，尽力把一行原因写入 `config_root()/daemon/daemon.error`（`0600`，与 `workspace.json` 同目录；写失败只记日志），以非零状态退出并提示“升级 crossh”。成功启动并发布端口文件后 MUST 删除陈旧的 `daemon.error`；删除失败只记日志，MUST NOT 影响启动，MUST NOT 因此保留或改写该文件。读取方 MUST 以“当前没有可达且兼容的常驻进程”为展示前提（D13）：只要端口可达且握手成功，陈旧的 `daemon.error` 一律忽略，因此删除失败不会掩盖当前状态。这条保护必须发生在任何写入之前：晚一步就会用空工作区覆盖新版文件。`daemon.error` 是格式错误与启动失败在 CLI 侧的观察入口（D13），`storage_error` 只表示可恢复的写入失败。

损坏恢复保持：主文件 → 备份 → 空工作区；主文件解析失败时改名为 `workspace.json.corrupt-<timestamp>` 并保留备份；主文件与备份都损坏时两者都改名隔离，以空工作区启动。

否决“每次变更同步写”：分屏拖拽与 cwd 变化会产生大量写；结构同步、元数据 debounce 已经足够。否决“把工作区写进 cache 目录”：cache 可能被系统清理，布局是用户状态。

### D8 配置根：一个解析器，两个文件族

解析器落在 `crates/crossh-core/src/config/mod.rs`（当前只有一行模块注释），与 `CROSSH_CACHE_DIR` 的语义对称：变量值**直接就是 crossh 配置根**，不再拼接 `crossh`。

```text
config_root() = $CROSSH_CONFIG_DIR                  （设置后直接作为根，debug/release 都不加后缀）
              = home_dir()/.config/crossh[-dev]      （默认；debug 加 -dev，不用 dirs::config_dir()）

settings_write = config_root()/settings.toml        // 新写入的唯一落点
settings_read  = settings_write                     // 存在时读它
               = home_dir()/.config/crossh/settings.toml  // 仅当 settings_write 不存在、未设置 CROSSH_CONFIG_DIR、且该旧文件存在（只读回退）
workspace      = config_root()/daemon/workspace.json
```

**旧路径只读回退。** 旧版 debug 与 release 都写 `~/.config/crossh/settings.toml`（`src/features/settings/persistence.rs:97-102` 与 `crates/crossh-core/src/locale.rs:41-43` 都没有 debug 后缀），新版 debug 根变成 `~/.config/crossh-dev`，因此 debug 首次启动在 `crossh-dev/settings.toml` 读不到任何东西。回退规则因此是：`settings_read` 在 `settings_write` 不存在、未设置 `CROSSH_CONFIG_DIR`、旧文件存在时读旧文件；回退**只用于读取**（`locale.rs` 的语言读取与 GUI 的 settings 读取都走它），新写入永远落在 `settings_write`。debug 的旧文件因此不被新版写入（逐字节不变）；release 的 `settings_write` 与旧文件同路径，迁移完成后的首次保存 MAY 重写该文件并删除旧字段，旧值只保留在 `.pre-workspace` 快照（D11）。`CROSSH_CONFIG_DIR` 设置时不回退：它的值就是根，由用户显式指定。

`CROSSH_CONFIG_DIR` 指到 `~/.config/crossh` 时得到 `~/.config/crossh/settings.toml`，指到临时根时得到 `<root>/settings.toml` 与 `<root>/daemon/workspace.json`；两种情况下都不出现重复的 `crossh` 目录。`CROSSH_CONFIG_DIR` 同时控制 settings 与 workspace：没有它，迁移集成测试无法在干净目录里跑（settings 会读写真实用户目录）。daemon 以 detached 方式启动（`client.rs:99` 的 `spawn_detached`，unix `setsid`、Windows `DETACHED_PROCESS`）且继承环境，因此客户端设置该变量后 daemon 也看到同一根；任务 2.1 用集成测试固定这一点。

理由：`dirs::config_dir()` 在 macOS 返回 `~/Library/Application Support`，与文档、现有 settings 路径和用户预期都不一致；`crates/crossh-note/src/lib.rs:178-181` 用它是既成事实，但 settings 与工作区都不跟随它，避免同一份文档写着两个路径。`-dev` 与现有 daemon cache 根（`instance.rs:24-31`）一致，debug 与 release 使用各自的配置根。

debug 与 release 因此各有独立的 settings 与工作区，迁移标记也在各自工作区内，`crossh-dev` 的首次运行不会消费 release 需要的数据。代价是 debug 构建看不到 release 的实时设置；这是接受的行为（两者本来就是不同用户画像）。旧版本升级的路径是：release 从 `~/.config/crossh/settings.toml` 升到同一路径（不迁移目录）；debug 从 `~/.config/crossh/settings.toml` 升到 `~/.config/crossh-dev`，靠上面的只读回退读旧文件、再写入新根。两侧读到同一份旧文件时各自迁移一次（各自工作区有独立的 `legacy_imported`）；debug 回退源不被新 debug 写入，release 的迁移源与 `settings_write` 同路径，首次保存 MAY 重写它，旧值由 `.pre-workspace` 保留。两侧只有当 `CROSSH_CONFIG_DIR` 指向同一根时才读同一份设置与快照（集成测试与共享配置场景即如此）。`crates/crossh-core/src/locale.rs:41-43` 的 `settings_path()` 改为调用同一解析器，`crossh-git`（`src/bin/crossh-git.rs:53`）、`crossh-note`（`src/bin/crossh-note.rs:60`）与 GUI、`crossh-session` 由此读到同一路径。

否决“保留 `~/.config` 作为父目录、由调用方自行拼 `crossh`”：同一个变量在不同调用点会被拼成不同路径（重复 `crossh` 或漏拼），且与 `CROSSH_CACHE_DIR` 的使用习惯相反。

### D9 协议 v2：消息、快照与能力协商

```text
WorkspaceGet          = 17   // 空载荷；返回一份完整快照
WorkspaceAttach       = 18   // 空载荷；返回当前快照并订阅后续快照
WorkspaceMutate       = 19   // JSON WorkspaceMutateRequest
WorkspaceSnapshot     = 20   // JSON WorkspaceSnapshot（服务端推送）
WorkspaceMutateReply  = 21   // JSON WorkspaceMutateReply
```

```rust
pub struct WorkspaceSnapshot {
    pub revision: u64,
    pub active_project: Option<ProjectId>,
    pub projects: Vec<ProjectSnapshot>,
    pub storage_error: Option<StorageErrorInfo>,
}

pub struct StorageErrorInfo {
    pub kind: StorageErrorKind,     // io | format | corpus
    pub message: String,
    pub path: Option<PathBuf>,
}

pub struct ProjectSnapshot {
    pub id: ProjectId, pub root: PathBuf, pub name: Option<String>,
    pub available: bool,
    pub active_tab: Option<TabId>,
    pub tabs: Vec<TabSnapshot>,
}

pub struct TabSnapshot {
    pub id: TabId, pub title: Option<String>, pub default_command: Option<String>,
    pub active_pane: Option<PaneId>, pub layout: PaneNodeSnapshot,
}

pub enum PaneNodeSnapshot {
    Leaf(PaneSnapshot),
    Split { id: SplitId, axis: Axis, ratio: f32, first: Box<PaneNodeSnapshot>, second: Box<PaneNodeSnapshot> },
}

pub struct PaneSnapshot {
    pub id: PaneId,
    pub cwd: PathBuf,
    pub size: TermSize,
    pub session: Option<SessionId>,  // 运行时绑定；不落盘
    pub cwd_available: bool,         // 运行时；cwd 目录当前存在
    pub status: PaneStatus,
}

pub enum PaneStatusReason {
    NotStarted,          // not_started
    SpawnFailed,         // spawn_failed
    ProjectRootMissing,  // project_root_missing
    CwdMissing,          // cwd_missing
}

pub enum PaneStatus {
    Starting,
    Live,
    Dormant { reason: PaneStatusReason },      // not_started | spawn_failed
    Unavailable { reason: PaneStatusReason },  // project_root_missing | cwd_missing
}
```

`SessionInfo`（`protocol.rs:196-203`）追加 `project_id`/`tab_id`/`pane_id`，临时会话三项为 `None`。快照不单列 `pane_sessions` 映射：`PaneSnapshot.session` 是 pane 与会话的唯一绑定来源，`PaneStatus` 只描述运行阶段、不内嵌会话 id，避免两处真相。不变量：`session = Some` 时 `status` 必为 `Live`，`status = Live` 时 `session` 必为 `Some`（D5 判定顺序）。项目 `available` 与 `PaneSnapshot.cwd_available` 是并列的警告字段，供客户端在 live pane 上显示“cwd 或项目根已缺失”而不影响重新接入。

**活跃 pane 只有两级。** 快照不在 workspace 级重复 `active_pane`；活跃 pane 由 `active_project → ProjectSnapshot.active_tab → TabSnapshot.active_pane` 推导，`ActivatePane` 同时写这三级（D5）。不变量：同一 tab 只有一个 `active_pane`；从非活跃 tab 激活 pane 会同步 `active_tab` 与 `active_project`。这样 GUI 不可能把焦点画在非活跃 tab 上，客户端也不需要判断两级字段谁优先。

**同 revision 快照。** `RefreshAvailability`、`BindSession`/`UnbindSession` 与 `PersistenceResult` 都可以在 revision 不变时重发快照，但两次快照的布局字段（projects/tabs/layout/active 三级）必须逐字段相同，只允许 `available`、`PaneSnapshot.session`、`cwd_available`、`PaneStatus`、`storage_error` 变化；客户端按 D6 的规则合并而不是整份覆盖。

广播用 `tokio::sync::watch<Arc<WorkspaceSnapshot>>`：actor 每次发布后更新；每个 `WorkspaceAttach` 连接先 `subscribe()` 再启动转发任务；`WorkspaceGet` 不需要订阅。

能力协商：新增 `workspace-v1` 标志。客户端与常驻进程都声明它才能使用工作区；常驻进程未声明时客户端 MUST 拒绝连接并提示升级，**不静默降级成 GUI 本地持有布局**（那正是本变更要消除的混合状态）。客户端不认识的服务端标志继续忽略，保持 `session-host` R5 的“未知标志不断开”。

`Input` / `Output` / `Resize` / `Attach` 帧保持不变。版本不匹配的拒绝沿用 v1 的握手路径，文案给出 `crossh daemon stop`。

### D10 物化时序：启动不 spawn，首个 attach 物化

选定“首个 `WorkspaceAttach` 之后物化活跃 tab”，而不是“daemon 启动即物化”：

- 启动即物化会在无人使用、甚至没有任何客户端的情况下启动一堆 shell；daemon 只是被 `crossh ls` 拉起时尤其明显。
- attach 是“有人要看”的最早可靠信号；GUI 通过它接入，`crossh open` 只改布局、不 attach，因此 CLI 操作不会拉起 shell。
- 延迟只影响会话创建，不影响布局可读性：重启后快照立即可读，pane 为 `Dormant`。

统一后的时序：daemon 启动加载工作区并发布首份快照，**不**创建任何会话；首个 `WorkspaceAttach` 物化活跃 tab 中每个**可物化**的 leaf——即无会话、所属项目 `available = true`、pane cwd 存在的 pane；已有会话（`Live`）MUST NOT 重复 spawn，`Starting` MUST NOT 重复投递，`Unavailable` MUST NOT spawn。非活跃 tab 仅在 `attached_clients > 0` 时于 `ActivateTab`/`ActivateProject` 按同一规则物化，计数为 0 时激活只改活跃位置、不 spawn；`OpenProject`/`CreateTab`/`SplitPane`/`EnsurePaneSession` 在已有 attach 客户端时立即物化其新建或新激活的可物化 pane，`OpenProject` 的缺失根分支只建 `Unavailable` pane、不 spawn。无客户端窗口期内 `crossh ls` 看到 0 个会话，`crossh daemon status` 仍能显示项目/tab/pane 计数（计数来自工作区，不来自会话表）。

**“已有 attach 客户端”的判定。** actor 在 `Runtime.attached_clients` 上计数：`WorkspaceAttach` 订阅成功时 +1，订阅连接结束（`AttachEnded`）时 −1；`WorkspaceGet` 不计入。只有计数大于 0 时，`OpenProject`/`CreateTab`/`SplitPane`/`EnsurePaneSession` 与 `ActivateTab`/`ActivateProject` 才立即物化；为 0 时它们只修改布局与活跃位置，把 pane 留在 `Dormant`，等下一个 attach 后由 attach 或激活物化。此分支的 `EnsurePaneSession` 返回 `Unchanged`；有 attach 时的休眠物化返回 `Applied` 且 revision 不变（D6）。`crossh open` 只发 `WorkspaceMutate`、不 attach，因此 CLI 打开项目不会拉起 shell，GUI attach 后才物化。计数只影响“是否立即物化”，不影响快照内容与 revision。

### D11 旧设置迁移：daemon 内部、启动时、一次性

迁移由 daemon 在启动、开始服务（发布 `daemon.port`）之前完成，不经过协议，也不依赖 GUI：

1. 解析配置根（D8），并确定**迁移源文件**：`settings_write` 存在时用它；否则按 D8 的只读回退用旧路径 `~/.config/crossh/settings.toml`（`CROSSH_CONFIG_DIR` 设置时不回退）。若源文件旁的同名快照 `<源文件>.pre-workspace` 不存在，而源文件含非空的 `recent_local_dirs` 或 `pinned_local_tabs`，先把整个文件逐字节复制成该快照（`0600`）。复制失败 → 本轮不迁移、不写标记，下次重试。快照写在被读取的源文件旁边：release（`~/.config/crossh/settings.toml`）与经回退读取的 debug 都得到 `~/.config/crossh/settings.toml.pre-workspace`，源文件本身已是 `crossh-dev/settings.toml` 时为 `~/.config/crossh-dev/settings.toml.pre-workspace`；同一源文件的多次启动共享同一份，`CROSSH_CONFIG_DIR` 指向同一根时两侧构建也共享。
2. 用**独立的 legacy 反序列化结构**读取迁移源（快照存在时读快照，否则读源文件）：`recent_local_dirs`、`recent_local_dirs_max`、`pinned_local_tabs`，以及 `PinnedLocalTab` 的全部字段 `pin_id`、`project_dir`、`cwd`、`custom_name`、`default_command`、`daemon_session_id`。这些结构与运行时 settings 无关，因此运行时字段删除不影响读取。
3. 若工作区 `legacy_imported` 已有值，跳过。
4. 构造导入后的候选工作区：固定标签按设置顺序转成项目 + tab + 单 pane（cwd 缺失时用 `project_dir`，`daemon_session_id` 丢弃），最近目录按顺序转成空项目并按规范化根去重（与已导入的项目去重），固定标签在前、最近目录在后；活跃项目取第一个固定标签项目，否则第一个最近目录。缺失根保留并标 `available=false`。
5. 一次性持久化候选（含标记 `LegacyImport { imported_at, source }`）。写入成功才采纳候选；失败则丢弃候选、保持未迁移状态并继续启动（内存服务不依赖迁移结果），下次重试。数据与标记在同一次写入中落盘，因此不存在“已导入但没记录”。

迁移与“工作区是否为空”无关：即使 CLI 先执行 `crossh open` 让工作区非空，也会合并导入（同根项目去重）。因为 daemon 在开始服务前迁移，正常情况下 CLI 不可能抢在前面；这条规则是对降级/升级混用的防御。

旧 settings 写保护：GUI 在首次 `WorkspaceAttach` 成功前 MUST NOT 保存 settings。此后新结构保存写到 `settings_write`，不再写旧字段：release 的 `settings_write` 就是旧的 `~/.config/crossh/settings.toml`，保存会重写该文件并丢掉旧字段，旧值仅存于 `~/.config/crossh/settings.toml.pre-workspace`；debug 的 `settings_write` 是 `crossh-dev/settings.toml`，旧文件 `~/.config/crossh/settings.toml` 不被新版触碰，回退读取在新文件出现后停止。

否决“GUI 读取旧设置并发 `ImportLegacy`”：GUI 的启动归一化（`shell/mod.rs:226-241`）会先修剪并写回旧字段，`WorkspaceGet` 为空这个条件又会被 CLI 先 `open` 破坏；协议里多一条只为迁移存在的消息，还得处理幂等与失败重试。

`ImportLegacy` 从协议与 GUI 触发路径中删除。

### D12 GUI 投影：活动键、顺序与删除清单

`WorkspaceState` 改为：

```rust
pub struct WorkspaceState {
    pub snapshot: WorkspaceSnapshot,
    pub pane_views: BTreeMap<PaneId, Entity<TerminalView>>,
    pub compose: BTreeMap<PaneId, ComposeEntry>,
    pub git_status: BTreeMap<PaneId, GitStatus>,
    pub git_refresh: BTreeMap<PaneId, GitStatusRefresh>,
    pub connection: ConnState,
    pub dragging_split: Option<(SplitId, f32)>,
}
```

- **活动键改为 `PaneId`**：`ActiveView` 与 `active_view` 删除，活动位置由 `snapshot.active_project → ProjectSnapshot.active_tab → TabSnapshot.active_pane` 投影得出（快照没有 workspace 级 `active_pane`）；`terminal_splits`/`compose`/三组尺寸全部以 `PaneId` 或 `SplitId` 为键。`SessionRegistry.local_sessions`、`LocalSession`、`LocalSessionId` 随布局语义一并删除。
- **git 状态改为按 `PaneId`**：`LocalSession.git_status`（`state.rs:30`）与 `AppShell.git_sync`/`git_fetching`（`shell/mod.rs:203-205`）改键；pane 身份跨 daemon 重启稳定，会话重建不会让 git 状态错位。
- **读取点改快照**：状态栏与“在编辑器中打开”从 pane 的 cwd 取（`view.rs:768`、`:922-925`）；命令面板从项目根/pane cwd 取（`command_palette.rs:348`、`:364`）；tab strip 的标题、cwd、默认命令从 `TabSnapshot`/`PaneSnapshot` 取（`tab_strip.rs:222`、`:257`、`:342`）；empty state 的“继续”入口改读项目顺序（`empty_state.rs:40-44`）；sidebar 排序改读 `projects` 顺序（`sidebar.rs:341-344`）。
- **项目顺序**：侧栏严格按快照 `projects` 顺序；打开已有项目移到最前，新项目插到最前，由 daemon 在 `OpenProject` 中完成。
- **`new_tab`**：有活跃项目 → 在该项目内 `CreateTab`，cwd 取活跃 pane 的 cwd、没有则项目根；无活跃项目 → 以 `current_local_cwd()` 发 `OpenProject`，成功后再 `CreateTab`。`SwitchToTab1..9` 按活动项目的 tab 顺序发 `ActivateTab`；分屏/焦点动作映射为 `SplitPane`/`ClosePane`/`SetSplitRatio`/`ActivatePane`。
- **`default_command` 的执行者**：daemon 只持久化 `TabSnapshot.default_command`（`CreateTab`/`SetTabCommand`），不解析也不执行；GUI 在该 tab 的 pane 首次空闲（会话 attach 且回放完成）时执行，失败只显示状态，不改布局。
- **事件语义**：`TerminalEvent::Closed` 不再触发 `close_local_session`，布局关闭只由快照 diff 驱动；`TerminalEvent::SessionAttached` 不再写 `daemon_session_id`（字段已删除），只用于解除“等待接入再投递输入”的门控；`TerminalEvent::CwdChanged` 只刷新链接解析与 git，不再影响项目归属。
- **启动顺序**：读 settings（只读）→ `ensure_daemon` → 连接 → `WorkspaceAttach` → 渲染；失败保留最后一份快照并显示重连态，断开期间禁用修改。
- **连接分层**：GUI 保留一条工作区连接（attach + mutate），每个 pane 的 `DaemonLink` 仍然独立 attach 自己的会话（`crates/crossh-terminal-view/src/daemon_link.rs` 的 `LinkMode::Create/Attach` 不变）。

删除清单（`rg` 必须为 0 的符号见 tasks 5.6）：`ActiveView`、`active_view`、`LocalSession`、`LocalSessionId`、`SessionRegistry.local_sessions`、`LocalDir`、`local_dirs`、`rebuild_local_dirs`、`sync_local_dirs`、`pinned.rs` 及其 `restore_pinned_local_tabs`/`restore_pinned_tabs_for_project`、`adopt_orphan_daemon_sessions`、`TerminalSplitState`、`SplitSide`、`split_widths`/`split_heights`/`split_heights_right`、`pin_id`、`custom_name`、`default_command`（迁到 `TabSnapshot`）、`daemon_session_id`、`WorkspaceSettings.recent_dirs`/`recent_dirs_max`/`pinned_local_tabs`、`activate_local_dir`、`try_forward` 的目录实参（主窗口只传 `None`）。

保留：`scratch.rs` 的临时终端继续走 daemon 的临时会话，不出现在工作区快照；`link_preview`、note 与 git 入口不变。

### D13 CLI、单实例与旧 daemon

**`ensure_daemon` 区分“无监听”与“存活但不兼容”。** probe 返回三态：

- `Absent`：端口文件缺失或连接失败 → 端口文件存在时视为陈旧，删除并 detached 启动一次；spawn 后等待端口文件出现（最长 10 s）。等待失败时读取 `daemon.error` 与 `workspace.json` 的 `format_version`，把“格式过新需升级”之类的真实原因返回给调用方，而不是只说“启动失败”。
- `Ready(addr)`：当前版本握手成功 → 直接返回。
- `Incompatible { addr, message }`：TCP 通、但握手返回版本错误（v1 daemon 的版本检查先于令牌检查，`handshake.rs:93-98`，因此不发令牌也能识别）→ 返回明确错误，**不删除 `daemon.port`、不 spawn**。错误文案包含两侧版本与 `crossh daemon stop`。

**`daemon status` 使用无副作用的 `probe_daemon()`。** 三态分类由独立函数 `probe_daemon()`（与 `ensure_daemon` 同模块）实现：只读 `daemon.port`、尝试 TCP 连接并用当前版本握手，返回 `Absent`（无端口文件；或连接失败并标记陈旧）、`Incompatible { found, expected }` 与 `Ready(addr)`。它 MUST NOT 删除端口文件、MUST NOT `spawn_detached`、MUST NOT 调用 `ensure_daemon`。`ensure_daemon` 的三态保留给启动与连接路径；`daemon status` MUST 只调用 `probe_daemon()`，因此 v1 daemon 运行时不会出现第二个 daemon，端口文件也不会被删除。

**`daemon_stop` 不再 `ensure_daemon`。** 流程：`read_daemon_addr()`（`instance.rs:50`）→ 直接连端口 → 用当前版本握手 → 失败且是版本错误时，用 `version = 1`、空能力重试握手（同一 token，v1 的 `Shutdown = 16` 编号与空载荷不变）→ 成功后发 `Shutdown` → 等端口失效（最长 10 s）。两步都失败时返回可执行错误：打印端口文件路径、说明运行中的 daemon 可能来自更旧或更新的 crossh、要求用匹配版本执行 `crossh daemon stop`，退出非零。任何分支都不拉起新实例。

**`crossh open <path>`：**

1. 解析路径（沿用 `resolve_open_path`，非法/不存在路径在 CLI 侧就报错退出）。
2. `ensure_daemon()`；版本不兼容或启动失败 → 报错退出（含 `daemon.error` 的原因），不启动 GUI。
3. 连接并发送 `WorkspaceMutate { request_id, expected_revision: None, op: OpenProject { root } }`，等待回执（10 s 超时）。
4. 回执为 `Applied`/`Unchanged` → 唤起 GUI：单实例转发成功则打印“已在运行实例中打开”；无实例则 spawn GUI（不传路径）。
5. `Rejected`/`Conflict`/超时 → 非零退出，GUI 不动。

`expected_revision: None` 是刻意的 last-writer-wins：CLI 不知道 GUI 的当前 revision，携带旧值只会制造无意义的 `Conflict`。代价是当用户在 GUI 里正看同一项目时，CLI 的 `OpenProject` 可能把它重排到最前并激活；GUI 通过快照收敛到新顺序，这是接受的语义，不在 CLI 侧做额外合并。

**单实例通道只表示“显示窗口”。** 真实实现在 `crates/crossh-core/src/single_instance.rs`（`src/app/single_instance.rs` 只是 re-export）：`try_forward(Option<&Path>)`、`try_forward_git`、`try_forward_note` 共用 `try_forward_to` 与线格式，而空载荷（空行）已经表示“仅聚焦”。因此主窗口不改共享函数语义，只改调用方式：`src/main.rs:129` 调用 `try_forward(None)`（或新增只接受无载荷的 `try_forward_window()` 包装它），GUI 侧的 `serve` 回调忽略 `Some` 目录；`crossh git` / `crossh note` 继续使用带目录的 `try_forward_git` / `try_forward_note`。`app_menu::activate_project(cx, directory)`（`app_menu.rs:223-231`）改为 `show_main_window(cx)`，不再调用 `activate_local_dir`（`shell/mod.rs:579`）。目录语义既然已从载荷中移除，`app_menu` 就没有可转发的项目；`OpenProject` 上移到 `crossh open`（上文第 3 步），GUI 启动后 attach 即可看到活跃项目。

混合版本行为：旧 GUI 收到空载荷只唤起窗口（现有 `read_and_validate` 已把空行当仅聚焦），可接受；新 GUI 收到旧版客户端发来的带目录载荷时忽略目录，只 attach 工作区。两种情况下项目激活都只发生在 daemon 中。

**`crossh ls` 保留 ROLE 列**，输出 `ID ROLE ALIVE PROGRAM PROJECT TAB PANE CWD`：前四列顺序与宽度不变（`main.rs:265-267` 现有格式），`PROJECT`/`TAB`/`PANE` 输出稳定 id（无空格，临时会话显示 `-`），`CWD` 从第五列移到最后，含空格路径按“前七列之后的剩余字段”解析。这样 `role_of`（`client.rs:489`）的 controller/observer 可观察性不回退，层级列也不会破坏按空白切分的脚本。

**`crossh daemon status` 保留第一行原文**，追加第二行：

```text
running (3 sessions, port 54321)
workspace: 2 projects, 4 tabs, 6 panes
```

第一行格式不变，按行解析的脚本不受影响。`status` 复用同一套三态分类，但 MUST 由无副作用的 `probe_daemon()` 实现（D13 上文），不把“端口可达但握手不兼容”降级为陈旧端口：

| 情形 | 输出 | 退出码 |
| --- | --- | --- |
| 端口文件缺失，或端口不可连接 | `not running` / `not running (stale port file)`；若 `config_root()/daemon/daemon.error` 存在则 `not running: <原因>`（例如 `workspace format 2 is newer than supported 1; upgrade crossh`） | 非零 |
| 端口可达，但握手返回版本不兼容 | `incompatible: found v1, expected v2; run crossh daemon stop` | 非零 |
| 端口可达且握手成功 | 上面的两行原文 | 0 |

`daemon.error` 的展示前提是“没有可达且兼容的 daemon”：只要握手成功，陈旧的错误文件一律忽略，因此成功启动后删除失败不会掩盖当前状态。无监听且错误文件不存在时，才用 `workspace.json` 的 `format_version` 推断“升级 crossh”，否则退回原文 `not running` / `not running (stale port file)`。`Absent` 时 `daemon status` MUST NOT spawn：没有端口文件只打印原文，端口文件存在但连接失败时保留文件并按陈旧端口报告；`Incompatible` 时 MUST NOT 删除 `daemon.port`。`daemon.error` 只在没有可达且兼容的 daemon 时展示，其删除失败只记日志。

### D14 缺失项目根与可用性

`available` 在生成快照时计算，不落盘。根目录缺失时项目、tab、pane、比例、焦点原样保留，`available = false`；激活项目不删除也不迁移 pane。pane 状态按 D5 的判定表：已绑定会话的 pane 一律 `Live` 并保留 `session`，即使项目根或 cwd 已缺失，客户端仍可重新接入；无会话时，项目不可用为 `Unavailable(project_root_missing)` 且不尝试 spawn，项目可用而 pane cwd 缺失为 `Unavailable(cwd_missing)`，其余照常物化或保持 `Dormant`；`cwd_available` 与项目 `available` 作为并列警告字段暴露，目录缺失 MUST NOT 结束既有会话。`SetPaneCwd` 对 `Unavailable` 生效（`Live` 仍拒绝），因此目录恢复或换 cwd 后可以直接重试。刷新触发见 D4；目录恢复后下一次 `Get`/`Attach`/`ActivateProject`/显式刷新即变 `available = true`（live pane 的 `cwd_available` 同时变 `true`），`EnsurePaneSession` 可以重试，不会被重复 spawn。

否决“启动时清理不存在目录的项目”：用户可能只是外接盘未挂载或目录被暂时重命名。

### D15 与 session-daemon 的衔接

**前置顺序。** `session-daemon` MUST 先 apply；本变更的代码实施可在其归档前开始，但本变更归档前 `session-daemon` MUST 已归档并合入 `session-host` 主规格。本变更的 `specs/session-host/spec.md` 是对该主规格的 MODIFIED delta；`openspec validate --strict` 目前给出 `Archive would refuse this delta: target spec does not exist`，因此该 delta MUST 在 `session-daemon` 归档后随本变更合入。这是归档阻断条件，不是 apply 阻断条件。

**C8 六个显式关闭调用点的对应关系**（行号为本仓库当前行号，session-daemon 评审时引用的是同一批函数）：

| 原调用点（session-daemon C8） | 当前位置 | 本变更后的语义 |
| --- | --- | --- |
| `view.rs:789` `request_close` | `crates/crossh-terminal-view/src/view.rs:864`（`item.rs:99-100` 委托） | 布局 pane → `WorkspaceOp::ClosePane`；临时终端保持本地 detach |
| `shell/tabs.rs:208/217` | `tabs.rs:203`（关闭活动 tab）、`tabs.rs:208`（关闭单个会话） | `CloseTab` / `ClosePane` |
| `shell/split.rs:127` | `split.rs:127`（关闭 split 的 bottom） | `ClosePane` |
| `tab_strip.rs:301/373` | `tab_strip.rs:301`、`:373`（标签关闭按钮） | `CloseTab`（当前语义是关闭整个分屏容器） |
| `shell/mod.rs:365` | `shell/mod.rs:363`（菜单 `CloseLocalSession`） | `ClosePane`；`CloseOtherLocalSessions` 逐 pane 发 `ClosePane` |
| `quit.rs:135/151` | `quit.rs:41-47` 已无结束会话调用 | 关窗口只 detach，调用点消失，保留为回归断言 |
| `shell/split.rs:39-78` `toggle_terminal_split`（批量关闭在 `:66`/`:72`；入口 `view.rs:722`、`command_palette.rs:313`） | `split.rs:39`、`:66`、`:72`、入口 `view.rs:722` 与 `command_palette.rs:313` | 退出分屏时保留当前焦点 pane，其余 pane 发 `ClosePane`，tab 保留为单 pane |
| `shell/split.rs:156-168` `retire_split_pane` | `split.rs:156-168` | `ClosePane`（保留为普通标签的分支不动） |
| `shell/split.rs:172-220` `prepare_terminal_split_view_close` | `split.rs:172-220` | 布局 pane 发 `ClosePane`；临时终端本地 detach |

**“退出分屏”是折叠 split，不是结束整个 tab。** 新模型里 `toggle_terminal_split` / `toggle_vertical_split` 的关闭分支只移除被折叠的 pane，保留焦点 pane 与 tab；旧实现里会顺带结束会话或标签的副作用随 `close_local_session` 一起删除。`retire_split_pane` 只有在风险判定为“需要结束”时才发 `ClosePane`，`KeepAsTab` 分支保留 pane。`prepare_terminal_split_view_close` 对布局 pane 发 `ClosePane`，临时终端仍走本地 detach，不进入工作区。

**B1 与“自动重启旧 daemon”。** `session-daemon` 把自动重启列为遗留风险；本变更不依赖它，也不做它：升级路径是“客户端识别不兼容 → 明确报错 → 用户或 CLI 用 `crossh daemon stop`（对 v1 也有效）→ 下次启动自然拉起新 daemon”。因此即使自动重启永远不实现，升级也是可执行的。

### D16 测试与验证边界

纯逻辑测试放在 `crossh-session`：

- `workspace.rs`：每个 `WorkspaceOp` 的 serde 往返与纯函数 apply；`OpenProject` 的“已存在补 tab / 新项目建 tab / 缺失根建 tab + Unavailable pane”；`ActivateProject` 不建 tab 与空项目保持 0 tab；项目去重与顺序；`CreateTab` 空项目默认 cwd；`SplitPane` cwd 继承；`CloseTab` 相邻激活；`ClosePane` 折叠；比例 clamp；`SetPaneCwd` 拒绝 `Starting`/`Live`；pane 状态判定（会话绑定优先：项目根或 cwd 缺失的 live pane 仍为 `Live` 且 `session = Some`；无会话时 Starting → Dormant → Unavailable）；`ActivatePane` 三级同步与同 tab 唯一 `active_pane`；生命周期规则；2×2 形状到递归树的等价映射。
- revision：持久布局变化 `Applied` +1；仅运行时字段变化的 `Applied`（休眠 `EnsurePaneSession`、有变化的 `RefreshAvailability`、`BindSession`/`UnbindSession`、`PersistenceResult`）revision 不变并重发同 revision 快照；`EnsurePaneSession` 的 `Live`/`Starting`/`Dormant` 且 0 attach/`Unavailable` 分支与无变化的 `RefreshAvailability` 为 `Unchanged`；`Conflict`/`Rejected` 不变、`expected_revision` 命中与不命中；同 revision 快照只变运行时字段。
- 回执持久性：结构变更成功 → `durable = true` 且 `storage_error = None`；结构写失败 → `Applied` + `durable = false` + `storage_error = Some`；元数据与仅运行时字段 → `Applied` + `durable = false` + `None`；`Unchanged`/`Conflict`/`Rejected` → `durable = false` + `None`；四种组合的 serde 往返（含 `storage_error` 有/无 path 两种形态）。
- `workspace_store.rs`：序列化往返、原子写、备份回退、双损坏隔离、`format_version` 过新时文件逐字节不变且写 `daemon.error`、成功启动删除陈旧 `daemon.error`（删除失败只记日志、不影响启动）、写失败后的退避重试与恢复、`PersistenceResult` 成功与失败都触发同 revision 重发且不推进 revision、`CROSSH_CONFIG_DIR` 路径（变量值就是根，不再拼 `crossh`）、debug/release 默认后缀、三平台路径解析；结构变更的 ack 在 `fsync` 之后才返回，元数据入队即返回。
- 迁移：旧 TOML fixture（含全部 `PinnedLocalTab` 字段）、缺失目录保留、CLI 先 open、写失败重试、只导入一次、debug/release 各自根各导入一次、debug 回退读取 `~/.config/crossh/settings.toml` 且旧文件逐字节不变（不被新 debug 写入）、`crossh-dev/settings.toml` 出现后不再回退、快照写在实际读取的源文件旁边、release 首次保存重写同一 `settings.toml` 后旧字段消失且 `.pre-workspace` 仍可恢复旧值。
- 时钟与故障注入：debounce 用可控时钟 + 注入 store 断言合并次数与 shutdown flush；写入中断用可注入的 writer 钩子断言 tmp/bak 状态，不依赖真实 kill 时序；结构写入进行中再次 dirty 时，两个 ack 都必须在该 revision 落盘后返回，debounce 载荷不得吞掉待 ack 的结构状态。

协议与集成测试：

- 每个新 kind 的 serde 往返（含 `WorkspaceMutateReply = 21` 的 `durable`/`storage_error` 四种组合与 `PaneSnapshot.session`/`cwd_available`）、能力协商、版本拒绝。
- 两个客户端并发 mutate、no-op、CAS 冲突、拒绝、新建 ID 回执。
- 结构变更的回执在其 writer ack 之后到达（注入 store 断言顺序）；元数据回执不等待写入。
- 结构写失败 → 回执 `Applied` + `durable = false` + `storage_error`，快照携带同一错误，客户端显示“未落盘”；恢复后的下一次成功写入触发同 revision 快照且 `storage_error = None`。
- attach 物化只覆盖可物化 pane：活跃 tab 同时含 `Live`/`Dormant`/`Unavailable` 时只 spawn `Dormant`；缺失项目根或 cwd 缺失的 pane 不 spawn；GUI 已 attach 时 `OpenProject`/`CreateTab`/`SplitPane` 后新 pane 不停在 `Dormant`；CLI `open`（不 attach）后 pane 保持 `Dormant`，GUI attach 才物化；0 attach 时 `ActivateTab`/`ActivateProject` 只改活跃位置、不 spawn。
- `EnsurePaneSession` 与 `RefreshAvailability` 的分支：休眠且可物化 → `Applied`、revision 不变、快照转 `Starting`；`Live`/`Starting`/`Unavailable` 与 0 attach → `Unchanged`；`RefreshAvailability` 有运行时变化 → `Applied`（revision 不变），无变化 → `Unchanged` 并重发同 revision 快照。
- 显式关闭路由、自然退出、resize、daemon 重启后 attach 物化时序、无客户端窗口期的 `ls`/`status`。
- `StopProject`/`ForgetProject`/`CloseTab` 解绑先于 `CloseSession`；注入延迟退出的会话，断言回执不等待退出、回执快照不含目标 pane；随后的迟到与重复 `SessionExited` 被忽略且不重复删 pane。
- 退出分屏的折叠路径（`toggle_terminal_split`/`retire_split_pane`/`prepare_terminal_split_view_close`）只发 `ClosePane`。
- v2 客户端连 v1 假 daemon：不产生第二个 daemon、端口文件仍归旧实例；`crossh daemon stop` 停止 v1 daemon；`crossh daemon status` 对 v1 假 daemon 输出 `incompatible: ...` 且退出非零，断言没有第二个 daemon、`daemon.port` 未被删除；无端口文件时 status 不 spawn。
- `crossh open` 在 GUI 已运行/未运行、daemon 拒绝三种情况下都只经 daemon 打开项目。
- 单实例转发三种路径：GUI 已运行（空载荷只唤起窗口）、GUI 未运行（spawn 后 attach 看到项目）、旧 GUI 混合版本（空载荷只聚焦、带目录载荷被忽略）。
- `crossh ls` 列（含含空格 cwd 按剩余字段解析）与 `daemon status` 两行；controller/observer 易手后 ROLE 变化；`daemon.error` 存在且无 live daemon 时 status 输出 `not running: <原因>`；兼容 daemon 可达时陈旧错误文件被忽略；仅 `format_version` 过新时推断“升级 crossh”。

GUI 侧把投影拆成不依赖 `gpui` 的纯函数（快照 → 项目/tab 行、树遍历、焦点迁移、git 状态重绑），用单元测试覆盖；渲染层只做组合。

本机验证受磁盘限制：实现阶段不运行 `cargo build/check/test/clippy/fmt`，由 CI 的 macOS/Linux/Windows 任务承接；macOS 实机冒烟由用户执行。

## Risks / Trade-offs

- **GUI 状态重构面大** → 按“daemon 模型与协议 → GUI 投影 → CLI/迁移”分块，每块可独立编译与验证；投影逻辑先纯函数化。
- **递归分屏渲染比 2×2 复杂** → 先做树遍历、比例与焦点的纯逻辑测试，再替换渲染。
- **快照广播风暴** → 拖动比例只发最终值；`watch` 只保留最新快照，慢客户端不会积压历史。
- **持久化阻塞终端输出** → 写入在 actor 外的 writer task，元数据 debounce；PTY 读线程只发 `PaneCwdChanged`。
- **daemon 重启仍会丢进程** → 规格写成“布局保留、会话重建”；进程恢复需要独立的 supervisor/PTY 交接。
- **多客户端活跃位置互相干扰** → 工作区级活跃位置是刻意简化；输入权仍按 session 维持现有 controller/observer 语义。
- **旧 daemon 升级会结束会话** → 协议 v2 拒绝旧 daemon，`crossh daemon stop` 对 v1 也能生效；发布说明写清“布局保留、进程结束”。
- **迁移与 settings 的竞态** → 旧设置先复制成 `<迁移源>.pre-workspace`，迁移只读快照；GUI 在首次 attach 前不写 settings。
- **工作区文件损坏** → 原子写 + 备份 + 双损坏隔离；最坏情况从空工作区启动，不阻塞 daemon。
- **项目根缺失被误判** → 只标记不可用，不自动删除；刷新触发见 D14。

## Migration Plan

1. 发布前：协议 v2 与工作区格式 v1 一同上线；旧 daemon 由握手拒绝，`crossh daemon stop` 可停 v1。
2. daemon 首次启动：确定迁移源（`config_root()/settings.toml`，或 D8 回退的 `~/.config/crossh/settings.toml`）→ 在源文件旁创建 `.pre-workspace` 快照 → 导入固定标签与最近目录 → 写工作区与 `legacy_imported` 标记 → 发布端口文件。
3. 用户可见变化：分屏与项目分组跨重启保留；固定标签变成普通 tab；`crossh daemon stop` 结束会话但保留布局。
4. 回滚：恢复旧二进制后，旧 daemon 不读 `workspace.json`，旧 GUI 回到原有推断逻辑；工作区文件保留但不被使用。新版已从 settings 删除 `recent_local_dirs` / `pinned_local_tabs`，回滚后最近目录与固定标签都不会自动恢复——发布说明必须要求升级前备份 `settings.toml`，并指出 `settings.toml.pre-workspace` 只供再次升级使用。

## Open Questions

- 多窗口的每窗口活跃项目/tab/pane 是否作为下一变更引入；本变更使用工作区级活跃位置。
- 是否实现“版本不匹配时自动重启旧 daemon”；本变更只提供显式 stop 与兼容性错误。
- 工作区规模上限与快照分块策略；当前按单用户规模设计，若项目/pane 数接近 8 MiB 帧上限再引入分页或上限。

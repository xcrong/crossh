# daemon-workspace 变更评审

**结论（收尾确认 2026-11-17）：通过**（N1、N2、E3-1、E4-1 已在第四轮定点修订中关闭；C1–C12 与 D1–D8 全部关闭，无剩余 must-fix。见文末「收尾确认（2026-11-17）」。历史轮次结论保留在文末各节）

**评审日期：** 2026-11-17

**复审日期：** 2026-11-17（见文末「复审记录（2026-11-17）」「条件状态（复审 2026-11-17）」「最终复审（2026-11-17）」与「收尾复审（2026-11-17）」）

## Review Scope

本次评审只读检查，未运行任何 Rust 编译、检查或测试命令。阅读范围：

- `AGENTS.md` 的 OpenSpec Change Review Gate、工程规则、测试与构建约束
- `openspec/config.yaml`
- `openspec/changes/daemon-workspace/proposal.md`
- `openspec/changes/daemon-workspace/specs/workspace-state/spec.md`
- `openspec/changes/daemon-workspace/design.md`
- `openspec/changes/daemon-workspace/tasks.md`
- `openspec/changes/session-daemon/` 的 `proposal.md`、`design.md`、`specs/session-host/spec.md`、`tasks.md`、`review.md`
- `openspec/specs/` 目录清单（仅 `note-search`，无 `workspace-state`）
- `crates/crossh-session/src/protocol.rs`、`handshake.rs`、`serve.rs`、`session.rs`、`feed.rs`、`instance.rs`、`client.rs`、`spawn.rs`
- `src/features/workspace/state.rs`、`registry.rs`、`settings.rs`、`pinned.rs`、`local_paths.rs`、`sidebar.rs`、`empty_state.rs`、`tab_strip.rs`、`view.rs`、`command_palette.rs`、`shell/mod.rs`、`shell/tabs.rs`、`shell/split.rs`、`shell/quit.rs`
- `src/features/settings/persistence.rs`
- `src/app/cli.rs`、`src/app/single_instance.rs`、`src/infrastructure/app_menu.rs`、`src/main.rs`
- `crates/crossh-terminal-view/src/daemon_link.rs`、`crates/crossh-terminal-view/src/view.rs`
- `crates/crossh-note/src/lib.rs`（配置目录对照）
- `docs/engineering-notes/session-daemon-protocol.md`
- 只读校验：`openspec validate daemon-workspace --strict` 通过；`git status --short` 仅显示未跟踪的变更目录

## Findings

### Blocking

**B1. 协议升到 v2 后，文档承诺的 `crossh daemon stop` 无法停止旧 daemon，并会额外拉起一个新 daemon。**

证据链：

- `proposal.md:19`、`design.md:197`、`spec.md:181-187` 均声明版本不匹配时提示执行 `crossh daemon stop`。
- `crates/crossh-session/src/client.rs:455-471` 的 `daemon_stop` 使用 `PROTOCOL_VERSION` 发起 v2 握手；旧 v1 daemon 在 `handshake.rs:93-98` 拒绝版本，`daemon_stop` 在 `client.rs:466-470` 只看到非 `Welcome`，直接返回 `PermissionDenied("handshake failed")`，不会解析并展示 `handshake.rs:62-65` 中已包含的停止命令。
- 更严重的是 `daemon_stop` 先调用 `ensure_daemon()`（`client.rs:456`）。`ensure_daemon` 在 `client.rs:162-170` 以 v2 `probe`；版本不匹配被当作探活失败，随后删除旧 daemon 的 `daemon.port`（`:168`）并 `spawn_detached`（`:169`）。结果是一个新 daemon 被拉起、又被停止，旧 daemon 及其会话继续运行，端口文件已被删除。
- `design.md:260` 的“旧进程本来就会结束”与上述调用链不符。

这是升级路径上的用户可见行为错误，也是 spec R11 场景的可执行性缺口；实现按当前 design 落地会直接产生双 daemon 与受管会话失控，必须在实现前补齐。

**B2. `WorkspaceMutate` 成功不返回回包，调用方无法取得新建 ID、无法确认自己的操作已应用，也无法区分并发修改。**

证据：

- `design.md:190` 明确写“成功不单独回包；客户端通过快照的 `revision` 前进确认”。
- `WorkspaceSnapshot`（`design.md:177-181`）没有请求 id、操作结果或新建对象列表；`tasks.md:8` 的 `WorkspaceOp` 覆盖 `OpenProject`、`CreateTab`、`SplitPane` 等会创建稳定 ID 的操作，但没有任何回执携带这些 ID。
- watch 只保留最新完整快照（`design.md:187-192`）。两个客户端并发修改时，A 观察到 revision 前进，可能只是 B 的操作；如果 A 的操作是 no-op（例如激活已经活跃的项目、聚焦已聚焦的 pane、重命名成相同值），revision 可能完全不前进，调用方只能超时。
- `design.md:267-272` 的 `crossh open` 在发送 `WorkspaceMutate(OpenProject)` 后直接进入“确保 GUI 运行”，没有等待回执或快照确认；`spec.md:213-226` 又要求 CLI 打开结果与 GUI 一致。当前设计无法给出可执行的确认条件。
- `tasks.md:20` 只要求 revision 单调递增，没有规定 no-op、失败、并发冲突时 revision 是否变化，也没有规定 mutation 回执与广播快照的先后。

这会让 GUI 的新建 tab/分屏、CLI 的 open、并发客户端的状态收敛都缺少确定性答案，属于实现阶段必然返工的互操作缺口。

### Important

**I1. Workspace actor 的命令集合不完整，无法承载 design 自己承诺的物化、关闭、resize 与可用性刷新。**

`design.md:119-126` 只定义 `Get`、`Attach`、`Mutate`、`SessionExited`、`PaneCwdChanged`、`Shutdown`。但：

- `design.md:131` 要求 PTY spawn 在 actor 外执行后把 `(pane_id, session_id)` 交回 actor 绑定，命令集中没有对应的绑定命令。
- `design.md:195` 要求 `Resize` 同步写入 `pane.size`，`design.md:207` 要求显式关闭先解绑再发 `Close`，命令集中没有 resize 或关闭会话的命令。现有 `serve.rs:319-333` 在连接任务里直接处理 `Close` 与 `Shutdown`；若继续直接访问工作区或运行时映射，就违反 `spec.md:11` 的“所有修改由常驻进程串行应用”和 `design.md:135` 的 actor 边界。
- `design.md:210` 的 spawn 失败重试、`design.md:280-283` 的项目可用性恢复，也没有触发命令。
- `design.md:181` 给 `SessionInfo` 增加 `project_id`/`tab_id`/`pane_id`，但唯一构造点是 `serve.rs:81-82` 的 `info_of`。连接任务若在持有 session 锁时向 actor 查询归属，可能出现 session 锁与 workspace 锁的反向等待；`design.md:130` 只约束了 workspace 锁内不得 await session 锁，没有定义反方向。

**I2. 操作载荷与错误路径未定义；设计承诺的“修改 cwd”入口在 `WorkspaceOp` 中不存在。**

- `tasks.md:8` 的操作列表没有 `SetPaneCwd`，但 `design.md:210` 给 spawn 失败或 cwd 缺失的 pane 提供“修改 cwd”入口；该入口没有可发送的操作。
- `tasks.md:31` 只写“定义载荷”，没有列出字段。`SplitPane` 的方向与目标 pane、新 pane 的初始 cwd，`CreateTab` 的来源项目与初始 cwd/标题，`OpenProject` 对已存在项目与缺失根的处理，`EnsurePaneSession` 的触发条件，`ReorderTabs` 的目标位置，`CloseTab` 与多 pane tab 的关系，均未定义。
- spec 只有主路径场景。`spec.md:57-76` 只覆盖写入中断与主文件损坏，没有覆盖持久化写失败；`spec.md:96-109` 没有覆盖 spawn 失败或 cwd 不存在；`spec.md:181-194` 只覆盖握手拒绝；`format_version` 高于当前版本时 daemon 的行为（退出、只读、是否仍可修改）没有可观察断言。
- `design.md:131`、`design.md:210` 说快照可以附带运行时错误，但 `design.md:177-181` 的 `WorkspaceSnapshot` 没有错误字段或 dormant 状态字段，D8 的 GUI 无法按当前 schema 显示错误并重试。
- `src/features/workspace/local_paths.rs:17-23` 的 `normalize_local_cwd` 只接受存在的目录；缺失项目根和缺失 pane cwd 必须绕过该函数，否则 `spec.md:196-211` 的“保留布局并暴露不可用”无法实现。design 没有区分“项目根保留”和“pane cwd 物化”的归一化规则。

**I3. spec R6 要求 daemon 重启后立即为活跃 tab 创建会话，design 与 tasks 要求首个 `WorkspaceAttach` 才创建。**

- `spec.md:96`：常驻进程重启后 MUST 为当前活跃 tab 的 pane 创建新会话。
- `design.md:201`：`WorkspaceAttach` 时才确保活跃 tab 的 leaf 有 session；`tasks.md:25` 明确“启动不主动创建 session”，测试断言“首个 `WorkspaceAttach` 物化活跃 tab”。
- `spec.md:105-109` 只允许非活跃 tab 延迟，不能让活跃 tab 也延迟到 attach。

因此 daemon 重启后、没有客户端 attach 的窗口期内，spec 与 design 对“活跃 tab 是否有 session”给出相反答案。`crossh ls`、`crossh daemon status` 在该窗口期看到的状态也没有定义。必须统一为一种可观察时序：要么 daemon 启动即物化，要么把 R6 改写为“首个客户端 attach 后物化活跃 tab”，并同步场景与测试。

**I4. 持久化 MUST 与 debounce 写入存在冲突；writer 顺序和 `format_version` 拒绝后的写保护未闭环。**

- `spec.md:59-76` 要求工作区持久化、原子替换、至少一份上一版，并断言重启后恢复比例与焦点。`design.md:132` 对比例、焦点、cwd、尺寸采用 200–500ms debounce。daemon 在 debounce 窗口内被 SIGKILL 时，最后一次变更必然丢失，与“重启后恢复焦点与比例”的 MUST 冲突。需要把崩溃语义写成显式边界（例如“正常停止/显式 flush 后保证；崩溃最多丢失最近 N ms 的元数据”），或对焦点/比例采用同步写。
- `design.md:150-156` 的 tmp → `sync_all` → main 改名为 bak → rename → 目录 sync 顺序本身成立；但没有定义 writer task 的串行性、写入进行中再次 dirty 的合并规则、shutdown 时如何取消 debounce 并等待最后一次 flush。现有 `serve.rs:329-333` 的 `Shutdown` 在删除端口文件后立即 `std::process::exit(0)`，按现状无法保证 `tasks.md:25` 的“先 flush 再结束会话”。
- `design.md:156` 对更高 `format_version` 只说“拒绝加载并报错”。daemon 是否继续启动、是否拒绝所有 mutation、是否会用空工作区覆盖新文件，都没有规定；`spec.md:181-194` 也没有对应场景。若实现先加载失败再以空工作区启动，第一次修改就会覆盖新版文件，违背设计初衷。

**I5. 配置根路径在 macOS、debug/release 与测试隔离三个维度上不一致。**

- `design.md:140-146` 与 `tasks.md:13` 规定 `~/.config/crossh[-dev]/daemon/workspace.json`，`CROSSH_CONFIG_DIR` 为覆盖项。
- 现有 settings 路径是 `dirs::home_dir().join(".config/crossh/settings.toml")`（`src/features/settings/persistence.rs:97-102`），`CROSSH_CONFIG_DIR` 在当前代码中不存在（全仓仅 `CROSSH_CACHE_DIR`，见 `instance.rs:24`）。
- `dirs::config_dir()` 在 macOS 返回 `~/Library/Application Support`，而 `crates/crossh-note/src/lib.rs:178-181` 已经使用该解析器。如果 `workspace_store` 采用 `dirs::config_dir()`，实际路径会变成 `~/Library/Application Support/crossh/...`，与 proposal/design 写死的 `~/.config` 不一致；如果采用 `home_dir().join(".config")`，则与 `crossh-note` 不一致。任务 2.1 没有指定解析器。
- debug 工作区使用 `crossh-dev`，settings 没有 `-dev` 后缀。调试版首次运行会把旧 settings 的 pinned tab 迁移进 `crossh-dev` 工作区并删除旧字段，release 版的 `crossh` 工作区仍为空，迁移却已“发生过”，导致固定标签语义在 release 中永久丢失。需要共享迁移标记，或让工作区根与 settings 根采用同一 debug/release 策略。
- `CROSSH_CONFIG_DIR` 只隔离工作区文件，settings 仍读写真实 `~/.config/crossh/settings.toml`；`tasks.md:52` 的迁移集成测试无法在干净配置目录中运行。测试需要可注入的 settings 路径或同时支持 `CROSSH_CONFIG_DIR`。
- 把工作区放在 config 而不是 cache 的理由（`design.md:160`）成立；问题在于解析器与隔离策略未定。

**I6. 旧设置迁移的读取路径与启动顺序会丢数据；CLI 先打开项目会让迁移永久跳过。**

- `design.md:251-259` 要求 GUI 读取 `recent_dirs` 与 `pinned_local_tabs` 后发送 `ImportLegacy`，`tasks.md:52` 同样要求；但 `design.md:241` 与 `tasks.md:45` 又要求从 `WorkspaceSettings` 删除这两个字段。现有真实键名是 `recent_local_dirs` / `recent_local_dirs_max`（`settings.rs:34-38`，serde rename），删除字段后旧 TOML 键会被 serde 忽略，GUI 没有读取入口。需要独立的 legacy 反序列化结构，覆盖 `PinnedLocalTab` 的 `pin_id`、`project_dir`、`cwd`、`custom_name`、`default_command`、`daemon_session_id`（`settings.rs:15-26`）。
- `src/features/workspace/shell/mod.rs:226-241` 在 `AppShell::new` 启动时就会归一化 `recent_dirs`、修剪 `pinned_local_tabs`，并在值变化时立即 `settings::save`。如果这一步发生在 daemon 迁移之前，缺失目录会被删除并落盘，`ImportLegacy` 只能导入被修剪后的数据。迁移必须在任何归一化/保存之前读取旧值，或在迁移完成前禁止保存。
- `local_paths.rs:26` 的 `normalize_recent_dirs` 会丢弃不存在的目录；spec R12 要求缺失项目根保留布局，因此迁移不能用该函数处理 pinned 的 `project_dir`。
- `design.md:251` 规定迁移由 GUI 发起，触发条件是 `WorkspaceGet` 的 `projects` 为空。用户升级后先运行 `crossh open /repo`（`design.md:267-272`），daemon 工作区立即非空；GUI 之后启动时不会再导入 `pinned_local_tabs`，旧固定标签永久丢失。需要独立于“项目是否为空”的迁移标记，或让 CLI 在首次连接时也执行导入。
- 迁移失败（daemon 拒绝、写盘失败、GUI 崩溃）后的重试与“只导入一次”的提交点没有定义；`ImportLegacy` 对非空工作区是否幂等也未说明。

**I7. `crossh ls` 删除 ROLE 列，回退了 session-daemon 已关闭的可观察性条件。**

- `design.md:275` 与 `tasks.md:51` 把输出固定为 `PROJECT TAB PANE SESSION ALIVE PROGRAM CWD`，移除了 ROLE。
- 当前 `src/main.rs:265-267` 输出 `ID ROLE ALIVE PROGRAM CWD`；session-daemon 的 `specs/session-host/spec.md:33` 要求“输入权归属 MUST 可观察：会话列表显示每个会话当前由哪个客户端持有输入权”，其 `review.md:208`、`:230` 的 C6 正是以该可观察断言关闭的。
- 删除 ROLE 会让 controller/observer 易手在 CLI 上不可见，且与 `role_of`（`client.rs` 末尾）现有的可观察输出断裂。若认为 ROLE 不再重要，需要在 session-host 规格中同步修改并说明替代观察方式；否则应保留 ROLE 并追加新列。

**I8. `crossh open` 的单实例路径仍然绕过 daemon；proposal 的影响文件清单不完整。**

- `main.rs:129` 在 `crossh open` 时调用 `try_forward(initial_dir)`；`src/infrastructure/app_menu.rs:223-231` 的 `activate_project` 进一步调用 `shell.activate_local_dir(directory)`，直接开本地会话。
- `design.md:270-273` 只说单实例通道“可以只承担显示/唤起窗口”，没有列出要修改的 `single_instance.rs`、`app_menu.rs`、`activate_local_dir` 调用点，也没有定义 GUI 已运行与未运行两种情况下的确认时序。只要转发路径仍传目录，GUI 就会绕过 daemon 建会话，违反 `spec.md:11` 与 `spec.md:213-226`。
- `proposal.md:34-39` 的 Impact 未包含 `src/app/single_instance.rs`、`src/infrastructure/app_menu.rs`；`tasks.md:50` 的任务 6.1 只验证“GUI 未运行与已运行两种情况各测一次”，没有覆盖单实例转发路径仍传目录、`crossh open` 等待 daemon 回执这两条关键路径。
- 这与 session-daemon 的 `tasks.md:51`（5.3）和 `review.md:231` 的 C7 直接冲突：后者把 `crossh <path>` 逐字不变作为已完成条件，本变更却必须改变它。两处需要显式对齐。

**I9. GUI 迁移清单不足以覆盖真实的读取点，按 `tasks.md:42` 的删除符号做 `rg` 会留下大量编译失败或错误行为。**

`design.md:235-241` 的删除清单遗漏了以下仍然持有或读取布局语义的结构：

- `ActiveView`（`state.rs:20`）与 `active_view`：工作区渲染、关闭、焦点、快捷键都以它为主键；`compose` 也是 `BTreeMap<ActiveView, ComposeEntry>`（`registry.rs:191`）。`design.md:219-223` 的 `WorkspaceState` 没有说明 `ActiveView` 如何被 `PaneId` 取代。
- git 状态：`LocalSession.git_status` / `git_refresh`（`state.rs:30-31`）与 `AppShell.git_sync` / `git_fetching`（`shell/mod.rs:203-205`）以 `LocalSessionId` 为键；daemon 重启或 pane 身份稳定后需要重新映射到 `PaneId`，否则 git 状态会丢或错位。
- `command_palette.rs:348` 读取 `LocalSession.project_dir`，`:364` 读取 `session.cwd`。
- `view.rs:768`、`:922-925`、`:1135` 读取 `session.cwd` / `git_status` 渲染状态栏与 git 状态。
- `tab_strip.rs:222`、`:257`、`:342` 读取 `dir.project_dir` 与 `session.cwd`。
- `sidebar.rs:341-344` 仍按 `recent_dirs` 位置排序项目；`recent_dirs` 删除后项目顺序没有替代规则。
- `empty_state.rs:40-44` 用 `recent_dirs` 与 `local_dirs` 生成“继续”入口。
- 快捷键 `SwitchToTab1..9`、分屏与焦点动作、`new_tab`（`tabs.rs:411-420`，无活动会话时回退到 `current_local_cwd()` 并开新项目）都直接操作旧的布局模型。
- `local_paths.rs` 的“只接受存在目录”语义需要拆分：pane cwd 与项目根的行为不同（见 I2）。

`proposal.md:34-39` 的 Impact 也未列出 `command_palette.rs`、`view.rs`、`single_instance.rs`、`app_menu.rs`。任务 5.6 只按少数符号做 `rg`，不足以证明迁移完成。

**I10. 与 session-daemon 的衔接没有前置条件或规格修订，多个已完成条件会被本变更推翻。**

- `proposal.md:30` 只说明本变更建立在 session-daemon 之上，没有声明“必须先合入/归档”或任何顺序约束。
- session-daemon 的 `specs/session-host/spec.md:22` 仍要求“原有的固定标签重新接入各自的同一个会话”，而本变更删除 `pinned.rs`、`pinned_local_tabs` 与 `daemon_session_id`，该场景将失去实现基础。
- session-daemon 的 `tasks.md:41`（4.4 固定标签按 id 恢复）仍未完成，`tasks.md:51`（5.3 `crossh <path>` 逐字不变）已勾选；本变更分别使其作废和失效。
- session-daemon 的 `review.md:231` C7 明确把 `crossh <path>` 移出该变更，本变更的 D10 又必须改变它；`review.md:210`/`:232` 的 C8 把“显式结束会话”定义为“向 daemon 发 close 请求 + 断开连接”，并穷举了六个 GUI 调用点，而本变更 D8 要求布局 pane 的关闭改发 `ClosePane`/`CloseTab`（`design.md:233`），这些调用点需要重新覆盖。`review.md:214` 还把“自动重启旧 daemon”列为另立变更的风险，而 B1 表明本变更正依赖这条未解决的升级路径。
- 若 session-daemon 先归档，主规格将包含固定标签场景；本变更的 `Modified Capabilities: 无` 将不准确。需要显式的前置/顺序声明，并在本变更中加入对 `session-host` 的 MODIFIED delta，或先修订 session-daemon。

**I11. 项目可用性恢复缺少快照刷新触发点。**

`design.md:280-283` 在生成快照时计算 `available`，`spec.md:196-211` 要求目录恢复后项目重新可用。但快照只在 actor 收到 mutation 或 `Attach`/`Get` 时生成；如果用户只是把目录重命名回来，没有发生任何布局操作，就没有新快照，GUI 会一直显示不可用。需要定义刷新触发（例如 `WorkspaceGet`/`WorkspaceAttach`/`ActivateProject` 时重新扫描，或显式的 `RefreshProjectAvailability` 操作），并给出对应场景与测试。

**I12. 项目顺序、空项目与 `CreateTab` 的目标语义未定义。**

- `design.md:44-52` 同时有 `Vec<Project>` 顺序与 `last_opened_at`，没有说明侧栏按哪一个排序、新项目插入位置、重新打开项目是否移动位置。现有 `sidebar.rs:341-344` 按 `recent_dirs` 排序，该依据被删除后没有替代规则。
- `design.md:257` 把 `recent_dirs` 迁移成“空项目”（无 tab），但 `design.md:201` 的 `WorkspaceAttach` 只在存在活跃 tab 时物化；激活一个空项目后没有 pane/session，`active_tab` 为 `None` 时 `CreateTab` 的目标项目与初始 cwd 也未定义。
- `tabs.rs:411-420` 的 `new_tab` 当前在没有活动会话时用 `current_local_cwd()` 打开新项目；新模型下的 `CreateTab` 是“在当前项目新建 tab”还是“以当前 cwd 新建项目”需要明确，否则 `cmd-t` 行为会变。

**I13. 测试计划没有覆盖本次评审识别出的高风险路径。**

`design.md:288-306` 的 D12 与 `tasks.md:57-60` 的 7.1–7.4 覆盖了模型、store、协议、daemon 生命周期与 GUI 纯投影，但没有列出：v2 客户端停止 v1 daemon（B1）、多客户端并发 mutation 与 no-op（B2）、debounce 窗口内 kill 与 shutdown flush（I4）、CLI 先 open 再启动 GUI 的迁移顺序（I6）、`crossh ls` 的 ROLE 回归（I7）、单实例转发路径（I8）以及 `ActiveView`/git 状态/状态栏读取点（I9）。这些验证已分别写入 C1、C2、C6、C8、C9、C10、C11；D12 与 tasks 的验证清单需要同步展开，否则 CI 绿灯不能证明上述风险已覆盖。

### Suggestions

**S1. `proposal.md:15` 对现有固定标签存储的描述不准确。** 原文说“固定标签只保存 `daemon_session_id`（`settings.rs:15`）”；`settings.rs:15` 是 `PinnedLocalTab` 结构体起始行，实际字段包括 `pin_id`、`project_dir`、`cwd`、`custom_name`、`default_command`、`daemon_session_id`（`settings.rs:16-26`）。迁移范围按 design D9 是正确的，但 proposal 的这句话会误导读者低估迁移字段。

**S2. 部分 `file:line` 是结构起始行而非所引字段。** `design.md:12` 的 `feed.rs:33` 实际是 `osc7: Osc7Scanner` 字段，`Osc7Scanner` 定义在更下方；`session.rs:139` 是 `SessionMeta` 结构体起始行而非 `cwd` 字段；`protocol.rs:17` 是 `MessageKind` 枚举起始行，`Shutdown = 16` 在 `protocol.rs:49`。建议改成精确字段行，避免后续实现按行号定位失败。

**S3. 四件套存在重复复述。** proposal 的 What Changes 与 design 的 Decisions、tasks 的阶段划分在多处重复持久化流程、协议编号与删除清单；按 `AGENTS.md` 的评审要求，proposal 应只保留动机与范围，design 保留决策与理由，tasks 只保留可执行项与验证，减少一处修改后多处不同步的风险。

**S4. 回滚说明低估数据损失。** `design.md` 的 Migration Plan 只说“回滚会丢失迁移后的固定标签语义”；旧版 GUI 还会忽略 `workspace.json`，且新版已从 settings 删除 `recent_dirs` / `pinned_local_tabs`，回滚后最近目录与固定标签都无法恢复。发布说明应写清这一点，并给出保留旧 settings 备份的建议。

**S5. `workspace-v1` 能力标志的协商失败语义未定义。** `design.md:197` 只写“未知标志继续忽略”，session-daemon 的 `design.md:95` 要求每个能力标志写清“对不支持它的对端该怎么办”。需要说明：v2 客户端连接不带 `workspace-v1` 的 daemon 时是拒绝、降级为无工作区，还是仅隐藏相关命令。

**S6. `tasks.md:16` 的 debounce 测试用“可控时钟”可行，但 `tasks.md:15` 的“在 rename 前后注入中断”需要明确模拟方式（子进程被杀、故障注入点、或仅断言 tmp/bak 状态），否则测试容易依赖平台时序。**

**S7. 2×2 到任意二叉树的布局迁移缺少等价性验证。** 现有 `TerminalSplitState` 固定四个槽位与三组尺寸（`registry.rs:185-194`），新模型是递归 `PaneNode`；任务 5.1 的测试覆盖 2×2 与三层嵌套，但没有验证“旧 2×2 布局在迁移后视觉与焦点顺序一致”。建议补一条纯逻辑映射测试或迁移说明。

**S8. `docs/engineering-notes/session-daemon-protocol.md` 已有“重连带 `since_seq`”的表述，与当前 `AttachRequest` 不一致；`tasks.md:53` 更新文档时应顺手修正，避免 v2 文档继续传播旧描述。**

**S9. `tasks.md:51` 的 `crossh daemon status` 计数与当前 `main.rs:213` 的 `"running (N sessions, port P)"` 差异较大，建议明确输出格式与向后兼容策略（脚本可能解析该行）。**

## Must-Fix Conditions

以下条件（原评审 C1–C12 与复审新增 D1–D8）全部关闭前不得进入 apply。逐条状态见文末「条件状态（复审 2026-11-17）」。

**C1（B1）升级与停服路径闭环。**
范围：`proposal.md:19`、`design.md:197`/`:260`、`spec.md:181-194`、`client.rs:455-471`、`client.rs:162-170`、`handshake.rs:93-98`。
必须：定义可执行的旧 daemon 停止方式（例如 `daemon_stop` 在 v2 握手失败后用同一 token 尝试 v1 握手，或使用 PID/信号），并让 `ensure_daemon` 把“版本不匹配”识别为存活 daemon，不得删除其 `daemon.port`、不得再 spawn 第二个 daemon；同步 spec R11 场景与发布说明。
验证：用 v1 假 daemon（或旧二进制）跑 `crossh daemon stop`，断言退出码 0、旧 daemon 消失、系统中只有一个 daemon、`daemon.port` 由属主删除；再断言 v2 客户端连接 v1 daemon 时不会产生第二个 daemon。

**状态（复审 2026-11-17）：已关闭。** 证据：`design.md:376-382` 定义 `Absent`/`Ready`/`Incompatible` 三态且 `Incompatible` 明确不删 `daemon.port`、不 spawn；`design.md:384` 的 `daemon_stop` 改为先 `read_daemon_addr()`（`instance.rs:50`）、当前版本失败且为版本错误时用 `version=1`/空能力重试、两步失败给可执行错误、任何分支不拉起新实例；`spec.md:296-311` 给出“新客户端连旧常驻进程”“停止旧版本常驻进程”两个场景，另有“缺少工作区能力”场景（`spec.md:313-315`）。源码前提成立：`handshake.rs:93-100` 的版本检查确实先于令牌检查，`verify_handshake` 在版本不符时直接返回 `VersionMismatch`；`protocol.rs:49` 的 `Shutdown = 16` 与 `client.rs:456-471` 的现状一致，回退可行。残留风险见 D4（`daemon status` 仍把不兼容报成 stale）。

**C2（B2）mutation 回执、稳定 ID 与并发仲裁。**
范围：`design.md:164-192`、`tasks.md:8`/`:20`/`:31`、`spec.md:213-226`、`design.md:267-272`。
必须：为 `WorkspaceMutate` 定义带 `request_id` 的成功回执，至少包含应用结果、当前 `revision` 与本次创建/变更的 `ProjectId`/`TabId`/`PaneId`/`SplitId`；定义 no-op、失败、并发修改时的 revision 规则与仲裁策略（last-writer-wins 或 expected-revision/CAS）；定义回执与广播快照的先后保证；让 `crossh open` 等待回执后才返回成功。
验证：协议 serde 往返测试；集成测试覆盖新建项目/tab/分屏后调用方拿到 ID、两个客户端并发 mutation、no-op mutation、失败 mutation；CLI `crossh open` 在 daemon 拒绝时不误报成功。

**状态（复审 2026-11-17）：已关闭。** 证据：`design.md:199-203` 追加 `WorkspaceMutateReply = 21`；`design.md:204-215` 的请求/回执结构与 `created` 四类 ID；`design.md:217` 的四种 outcome 与 revision 规则；`design.md:219` 的回执/快照同 revision 与“`>=` 整体替换、`<` 丢弃”的快照应用规则；`design.md:221` 的 `crossh open` 等待回执（10 s）；`spec.md:192-218` 四个场景覆盖新建 ID、no-op、CAS、拒绝；`tasks.md:24`/`:37`/`:72` 对应验证。

**C3（I1）actor 命令边界与 `Close` 路由。**
范围：`design.md:115-135`、`:195`、`:207`、`tasks.md:20-25`、`serve.rs:319-333`。
必须：补齐 actor 命令（至少包括 spawn 绑定结果、显式关闭/解绑、resize→pane.size、可用性刷新），明确 `Close` 消息由连接任务转发给 actor、actor 先解绑再触发会话关闭的时序；明确 `SetSplitRatio` 与 `Resize` 的职责差异；定义 `info_of` 获取归属信息的锁顺序或改用 actor 维护的镜像表。
验证：集成测试覆盖显式关闭、自然退出、resize、daemon 重启重绑；`rg` 证明工作区/运行时映射只在 actor 内被写入；并发关闭与 resize 无死锁。

**状态（复审 2026-11-17）：已关闭。** 证据：`design.md:115-134` 的命令集包含 `BindSession`/`UnbindSession`/`SessionSpawnFailed`/`SessionExited`/`PaneCwdChanged`/`ResizePane`/`ClosePane`/`CloseTab`/`SetPaneCwd`/`RefreshAvailability`/`Shutdown`；`design.md:145-149` 定义 `Close` 帧经归属镜像转 `ClosePane`、先解绑再投 `CloseSession`；`design.md:151-153` 规定 `SessionExited` 幂等与 `shutting_down` 忽略；`design.md:155-157` 把 `Resize` 限定为 controller 并区分 `SetSplitRatio`；`design.md:159-161` 固定锁顺序（`Sessions → OwnershipMirror`，任何锁不跨 actor 调用，`info_of` 不再在锁内拼归属）；`design.md:163` 给出可用性刷新触发点。`spec.md:144-146` 补了迟到退出通知不得重复删 pane；`tasks.md:23-28` 有对应验证。

**C4（I2、I11、I12）操作 schema 与错误路径。**
范围：`tasks.md:8`/`:31`、`design.md:177-181`/`:210`/`:280-283`、`spec.md:57-76`/`:96-109`/`:181-211`。
必须：逐个列出 `WorkspaceOp` 的 serde 载荷、目标与前置条件；增加 `SetPaneCwd`（或等价的修改 cwd 操作）；定义 `CreateTab` 在空项目/无活跃 tab 时的目标、`SplitPane` 的方向与初始 cwd、`OpenProject` 对重复/缺失根的处理、`EnsurePaneSession` 的触发与重试语义、`ReorderTabs` 的目标位置；在快照中增加 dormant/错误/`available` 的可观察字段；为 spawn 失败、cwd 不存在、项目根缺失、持久化写失败、`format_version` 过新、目录恢复后重新可用补充 spec 场景。
验证：每个 op 至少一条 serde 往返与纯函数 apply 测试；集成测试覆盖上述错误路径；spec 场景均可映射为测试或人工步骤。

**状态（复审 2026-11-17）：部分关闭。** 已落地：`design.md:168-190` 的 17 个 op（`OpenProject`…`SetTabCommand`）逐项给出字段、目标、前置条件与默认值；`design.md:185` 新增 `SetPaneCwd` 并区分 dormant/live（live → `pane_live`）；`design.md:193` 给出 8 个错误码；`design.md:273-306` 的快照含 `available`、`PaneStatus`、`storage_error`；`design.md:195` 把 `normalize_project_root`/`normalize_pane_cwd` 与 GUI 的 `normalize_local_cwd` 分开；`spec.md:85-99`/`:250-264`/`:317-334` 补了写失败、spawn 失败、pane cwd 缺失、项目根缺失与恢复场景。**未闭合：** D1（`Dormant`/`Unavailable` 边界与 `OpenProject` 初始 tab 语义）、D2（`active_pane` 两级定义）、D4（`storage_read_only` 无触发点）。

**C5（I3）daemon 重启物化时序。**
范围：`spec.md:96`/`:105-109`、`design.md:201`、`tasks.md:25`。
必须：在“daemon 启动即物化活跃 tab”和“首个 `WorkspaceAttach` 时物化”之间选定一种，并同步 spec 的 MUST/MAY、design、tasks 与测试断言；说明无客户端窗口期内 `crossh ls`/`daemon status` 看到的状态。
验证：集成测试在 daemon 重启后、无客户端 attach 时断言活跃 tab 的 session 集合，再 attach 后断言最终集合与 spec 一致。

**状态（复审 2026-11-17）：已关闭。** 证据：`spec.md:117-136` 已把 R6 改写为“启动不创建会话、首个 attach 物化活跃 tab、非活跃 tab 激活时物化”，并给出“重启后没有客户端接入”“首个 attach 物化活跃 tab”“后台 tab 延迟创建”三个场景；`design.md:317-325` 选定同一时序并说明无客户端窗口期 `crossh ls` 为 0、`daemon status` 仍有计数；`tasks.md:29`（3.7）与 `tasks.md:31`（3.9）按同一时序断言。四件套不再有“启动即物化”的相反表述。

**C6（I4）持久化耐久性与格式版本保护。**
范围：`spec.md:57-76`、`design.md:132`/`:150-160`、`tasks.md:16`、`serve.rs:329-333`。
必须：明确崩溃与正常停止的耐久性边界（若保留 debounce，则把 R4 的 MUST 限定为“正常停止/显式 flush 后”，并说明崩溃最多丢失的时间窗）；定义 writer task 串行、写入中 dirty 合并、shutdown 取消 debounce 并等待 flush 的顺序；定义 `format_version` 过新时的 daemon 行为，并保证在人工处理前不写回/不覆盖该文件（包括禁用 mutation 或直接拒绝启动，且向客户端暴露错误）。
验证：故障注入测试覆盖写入中断、debounce 窗口内 kill、shutdown flush；更高 `format_version` 文件在启动与随后的 mutation 尝试后逐字节不变。

**状态（复审 2026-11-17）：部分关闭。** 已落地：`spec.md:59` 把耐久性写成“正常停止/显式 flush 后完整、崩溃最多丢 500 ms 元数据”，并配“崩溃最多丢失元数据”“写入中断不产生半份文件”“持久化写失败仍可服务”“格式版本高于当前版本”四个场景（`spec.md:67-99`）；`design.md:229-241` 定义 single writer、dirty 合并、`tmp → sync_all → main→bak → rename → dir sync`、写失败 1 s→30 s 退避、shutdown 取消 debounce 并等 ack（最长 5 s）、`FormatTooNew` 不改写不发布端口文件；`tasks.md:16-18`/`:30` 有对应验证。**未闭合：** D3（`spec.md:59` 的“结构变更 MUST NOT 丢失”与“入队即回执”的机制不符）、D4（格式过新的提示只有日志，客户端不可观察）。

**C7（I5）配置根、macOS 路径与测试隔离。**
范围：`design.md:140-146`、`tasks.md:13`、`persistence.rs:97-102`、`instance.rs:24-27`、`crates/crossh-note/src/lib.rs:178-181`。
必须：指定唯一的路径解析器（`home_dir().join(".config")` 或 `dirs::config_dir()`），并让 workspace 路径与 settings 路径的 macOS 实际位置一致；正式引入并文档化 `CROSSH_CONFIG_DIR`；定义 debug/release 的迁移共享策略，避免 `crossh-dev` 消费一次性迁移；让 settings 读取支持测试注入或同样受 `CROSSH_CONFIG_DIR` 控制；确认 detached daemon 继承该环境变量。
验证：macOS/Linux/Windows 路径单元测试；debug 写入后 release 读取的迁移测试；`CROSSH_CONFIG_DIR` 隔离的集成测试不触碰真实用户目录。

**状态（复审 2026-11-17）：部分关闭。** 已落地：`design.md:245-257` 明确单一解析器 `config_root()`（`$CROSSH_CONFIG_DIR` 整体替换 `~/.config`，默认 `home_dir().join(".config")`，不用 `dirs::config_dir()`），settings 为 `config_root()/crossh/settings.toml`（与 `persistence.rs:97-102` 的现状一致，因此 `CROSSH_CONFIG_DIR` 指向 `~/.config` 时不会出现重复 `crossh` 目录）、workspace 为 `config_root()/crossh[-dev]/daemon/workspace.json`；debug/release 各迁移一次且共享只读快照；`design.md:257` 说明 detached daemon 继承环境；`tasks.md:15`/`:55` 覆盖路径、权限与隔离测试。**未闭合：** D6（`crates/crossh-core/src/locale.rs:41-43` 仍是第二个 settings 读取点，不受 `CROSSH_CONFIG_DIR` 控制）。

**C8（I6）旧设置迁移的读取、顺序与失败重试。**
范围：`design.md:251-260`、`tasks.md:45`/`:52`、`shell/mod.rs:226-241`、`settings.rs:15-38`、`local_paths.rs:26`。
必须：增加独立的 legacy 反序列化结构，覆盖 `recent_local_dirs`、`recent_local_dirs_max`、`pinned_local_tabs` 及 `PinnedLocalTab` 全部字段；在任何归一化/保存之前读取旧值；迁移完成前禁止修剪或落盘旧字段；定义独立于“projects 为空”的一次性标记，并覆盖“CLI 先 open 项目、GUI 后启动”的路径；缺失项目根在迁移中保留；定义 `ImportLegacy` 的幂等性与失败重试。
验证：旧 TOML fixture 迁移测试覆盖 pinned 的 cwd/名称/默认命令、缺失目录、CLI 先打开项目、daemon 不可用后重试、迁移只发生一次；断言旧字段不会在导入前被删除。

**状态（复审 2026-11-17）：已关闭。** 证据：`spec.md:265-292` 把迁移写成 daemon 启动前完成、不依赖 GUI、不以“工作区为空”为条件，字段清单、顺序、活跃项目、缺失根、一次性标记、写失败重试与“迁移完成前旧设置不得被覆盖”齐备；`design.md:327-343` 给出“先复制 `settings.toml.pre-workspace` 快照 → 独立 legacy 结构读取 → `legacy_imported` 跳过 → 候选工作区 → 成功才采纳”的流程，并删除 `ImportLegacy`；`tasks.md:19`/`:40`/`:55` 落地读取、删除与 settings 字段移除。代码前提成立：`settings.rs:15-38` 的字段与 `recent_local_dirs`/`recent_local_dirs_max` 的 serde rename 均与 design 一致，`shell/mod.rs:224-241` 的启动归一化写回必须随字段删除一起消失（`tasks.md:55` 已覆盖）。遗留措辞问题见 S14。

**C9（I7）`crossh ls` 角色可观察性。**
范围：`design.md:275`、`tasks.md:51`、`main.rs:265-267`、`session-host/spec.md:33`、`session-daemon/review.md:208`/`:230`。
必须：保留 ROLE 列（可同时增加 PROJECT/TAB/PANE），或提供等价的、在 CLI 上可观察的 controller/observer 输出，并同步 session-host 规格与 session-daemon 的 C6 记录；更新帮助文本、集成测试与文档。
验证：`crossh ls` 在 controller/observer 易手后输出变化，且与 GUI 只读状态一致。

**状态（复审 2026-11-17）：已关闭。** 证据：`design.md:394-397` 保留 ROLE 并把输出定为 `ID ROLE ALIVE PROGRAM CWD PROJECT TAB PANE`，前五列顺序与宽度沿用 `main.rs:265-283` 的现有格式，临时会话后三列为 `-`，并明确“`role_of`（`client.rs:489`）的可观察性不回退”；`design.md:399-403` 让 `daemon status` 保留第一行原文、追加工作区计数行；`tasks.md:62` 覆盖列、占位符、ROLE 易手与 help 文本。列语义未定见 S10。

**C10（I8）`crossh open` 的单实例路径与影响清单。**
范围：`design.md:267-273`、`main.rs:129`、`app_menu.rs:223-231`、`single_instance.rs`、`proposal.md:34-39`、`session-daemon/tasks.md:51`、`session-daemon/review.md:231`。
必须：把单实例转发载荷缩减为“显示/唤起窗口”，移除或改写 `activate_project`/`activate_local_dir` 的目录语义；定义 GUI 已运行与未运行时 `crossh open` 的等待与确认；把 `src/app/single_instance.rs`、`src/infrastructure/app_menu.rs` 加入 Impact 与任务；明确本变更对 session-daemon C7/5.3 的取代关系。
验证：GUI 已运行与未运行两种情况下 `crossh open` 都只通过 daemon 打开项目；`rg` 证明转发路径不再调用本地开会话；与 session-daemon 的冲突记录已更新。

**状态（复审 2026-11-17）：部分关闭。** 证据：`design.md:384-390` 给出 `crossh open` 五步（解析 → `ensure_daemon` 不兼容即退出 → `WorkspaceMutate(OpenProject)` 等回执 10 s → 成功才唤起 GUI → 失败不动 GUI）；`design.md:392` 把单实例载荷收敛为“显示窗口”、`try_forward()` 无参、`activate_project` 改 `show_main_window`；`proposal.md:14` 显式声明取代 session-daemon 评审 C7 与任务 5.3；`proposal.md:46` 与 `tasks.md:60-61` 覆盖 `single_instance.rs`、`app_menu.rs`。**未闭合：** 原条件指定的 `src/app/single_instance.rs` 只是 re-export（`src/app/single_instance.rs:7-9`），真实载荷在 `crates/crossh-core/src/single_instance.rs` 且与 git/note 共用，Impact/task 未覆盖，见 D5。

**C11（I9）GUI 迁移清单与验证。**
范围：`design.md:214-249`、`tasks.md:37-46`、`state.rs:20-39`、`shell/mod.rs:203-205`、`registry.rs:191-194`、`command_palette.rs:348`/`:364`、`view.rs:768`/`:922-925`、`tab_strip.rs:222`/`:257`/`:342`、`sidebar.rs:341-344`、`empty_state.rs:40-44`、`tabs.rs:411-420`。
必须：把 `ActiveView`/`active_view`、`compose` 的键迁移，git 状态从 `LocalSessionId` 迁到 `PaneId`，状态栏/git/链接的 cwd 读取改从快照或 pane 投影取得；更新快捷键、tab strip、sidebar 排序、empty state 与 `new_tab`；在 tasks 5.6 中加入覆盖这些符号和读取点的可验证 `rg` 清单，并把 `command_palette.rs`、`view.rs`、`single_instance.rs`、`app_menu.rs` 写入 Impact。
验证：`rg` 清单无残留；纯投影测试覆盖视图复用、git 状态重绑、关闭焦点迁移；macOS 冒烟覆盖 tab/sidebar/空状态/快捷键。

**状态（复审 2026-11-17）：已关闭。** 证据：`design.md:345-372` 给出新的 `WorkspaceState`（`snapshot` + `pane_views` + `compose` + `git_status`/`git_refresh` 均以 `PaneId`/`SplitId` 为键）、删除清单、读取点改造（`view.rs:768`/`:922-925`、`command_palette.rs:348`/`:364`、`tab_strip.rs:222`/`:257`/`:342`、`empty_state.rs:40-44`、`sidebar.rs:341-349`）、项目顺序、`new_tab`（`tabs.rs:411-420`）与快捷键映射；`tasks.md:45-54` 对应 5.2–5.8，`tasks.md:49-52` 给出 A/B/C 三组可执行 `rg` 清单；`proposal.md:44-46` 的 Impact 已含 `command_palette.rs`、`view.rs`、`single_instance.rs`、`app_menu.rs`。行号复核全部命中（`state.rs:20`/`:24`/`:30-31`、`registry.rs:190-195`、`shell/mod.rs:203-205`）。

**C12（I10）与 session-daemon 的前置条件与规格修订。**
范围：`proposal.md:30`、`session-host/spec.md:22`、`session-daemon/tasks.md:41`/`:51`、`session-daemon/review.md:210`/`:214`/`:231`。
必须：声明本变更必须在 session-daemon 合入/归档之后 apply，或给出并行实施的明确顺序；在 `openspec/changes/daemon-workspace/specs/` 中增加对 `session-host` 的 MODIFIED delta（至少移除/改写固定标签重连场景与 `crossh <path>` 不变的声明），或在 session-daemon 归档前修订其规格与任务；重新核对 C8 的六个显式关闭调用点在本变更下改发 `ClosePane`/`CloseTab`；说明 B1 与 session-daemon 遗留的“自动重启旧 daemon”风险的关系。
验证：`openspec validate --strict` 通过；session-daemon 归档后的主规格不包含已被本变更推翻的固定标签场景；`rg` 检查 `daemon_session_id`、`try_forward`、`crossh <path>` 的冲突任务已关闭或改写。

**状态（复审 2026-11-17）：部分关闭。** 已落地：`proposal.md:13` 声明 session-daemon 必须先 apply 并归档，`proposal.md:30-35` 增加 `session-host` MODIFIED delta，`specs/session-host/spec.md:1-31` 改写“重新打开后回到原会话”为“回到原布局/仍在运行的会话重连/已结束会话按 pane 重建”，并点名 `session-daemon/tasks.md:41` 与 `review.md:231` 的冲突；`design.md:417-424` 的六处关闭调用点行号全部命中（`crates/crossh-terminal-view/src/view.rs:864`、`tabs.rs:203/208`、`split.rs:127`、`tab_strip.rs:301/373`、`shell/mod.rs:363`、`quit.rs:41-47`）；`design.md:426` 说明与“自动重启旧 daemon”的关系；`openspec validate daemon-workspace --strict` 实测通过并给出 `target spec does not exist` INFO，`design.md:413`、`tasks.md:64`、`tasks.md:122` 已把它写成阻断条件（`tasks.md:122` 的标签误写为 apply，见 D7）。**未闭合：** D7（前置顺序与 `tasks.md:64` 矛盾、session-daemon 4.4 未作废）、D8（`toggle_terminal_split`/`retire_split_pane` 两条关闭路径未列入）。

## Confirmed Good Points

- 新能力声明正确：`openspec/specs/` 仅含 `note-search`，不存在 `workspace-state`，因此使用 ADDED 而非 MODIFIED 成立（`proposal.md:24-30`）。
- 消息编号 17–20 当前未被占用：`protocol.rs:17-49` 到 `Shutdown = 16` 为止；session-daemon 的规格只声明 `PROTOCOL_VERSION` 单调递增与能力标志，没有分配 17–20，也没有占用 v2。编号分配本身没有冲突。
- 版本不匹配的提示文案已经包含 `crossh daemon stop`（`handshake.rs:62-65`）；问题在停止动作，而非文案。
- 现状事实的主干引用成立：`serve.rs:61` 的 `Sessions`、`state.rs:24`/`:66` 的 `LocalSession`/`LocalDir`、`registry.rs:190` 的 `terminal_splits`、`tabs.rs:863` 的 `adopt_orphan_daemon_sessions`、`shell/mod.rs:893` 的 `sync_local_dirs`、`settings.rs:34`/`:38` 的旧字段、`handshake.rs:10` 的 v1、`instance.rs:27` 的 `-dev` cache 根均与代码一致。
- 把工作区放在 config 而非 cache 的理由成立；`CROSSH_CACHE_DIR` 已在 `instance.rs:24` 存在，可作为测试隔离的先例。
- daemon 已经通过 OSC 7 维护 `SessionMeta.cwd`（`feed.rs`、`session.rs`）并在 `List`/`Attach` 前刷新；布局层消费这条已有事实，不需要新增 PTY 解析。
- 关闭 GUI 只 detach 的现状已经存在（`shell/quit.rs:41-47`），`spec.md:133-146` 与实现方向一致。
- `TerminalSplitState` 的 2×2 固定槽位与三组尺寸确实由 GUI 持有（`registry.rs:185-194`）；用递归 `PaneNode` 取代它的动机成立。
- `pinned.rs`、`restore_pinned_local_tabs`、`sync_local_dirs`、`adopt_orphan_daemon_sessions` 都是真实的启发式路径，删除方向有代码依据。
- note 状态与工作区的耦合仅出现在 `view.rs:971-978` 的状态栏按钮与 `command_palette.rs:324-326` 的 `OpenNotes`，两者都不读取布局；迁移时保留入口即可，未发现额外的 note 布局读取点。
- spec 的 13 条 requirement 大多以可观察行为书写并配了场景，覆盖了所有权、身份、持久化、GUI 重启、daemon 重启、pane/session 分离、窗口关闭、项目生命周期、迁移、版本拒绝、缺失根与 CLI；`openspec validate --strict` 通过。
- 六种生命周期路径中，窗口关闭（`spec.md:133-146` 与 `shell/quit.rs:41-47`）、shell 自然退出（`spec.md:113-131`）、显式关闭 pane、停止项目与忘记项目（`spec.md:150-162`）在 spec 与 design 之间没有发现互相矛盾的描述；发现的时序冲突集中在 daemon 重启物化（I3）与 actor 关闭路由（I1）。
- Non-Goals 清晰：不做 VT 网格、不做 PTY 交接、不做多窗口独立视图、不做远程/托盘，范围没有失控。

## Unverified

- 未运行任何 `cargo build` / `cargo check` / `cargo clippy` / `cargo test` / `cargo fmt` / `scripts/check-architecture.sh`，因此无法验证 proposed 的 `SessionInfo` 字段扩展、actor 命令、workspace store、GUI 投影是否能编译，也无法验证任何运行时行为。
- 未运行协议 v2 与 v1 的真实互操作测试；B1 的升级路径结论来自对 `client.rs`、`ensure_daemon` 与握手代码的静态追踪。
- 未验证 macOS 实机上的配置路径（`dirs::config_dir()` 与 `~/.config` 的差异）、文件权限、daemon detached 启动的环境继承。
- 未验证 Windows/Linux 行为（ConPTY、ACL、配置路径）；按仓库约束这些由 CI 负责。
- 未执行迁移集成测试、故障注入测试、GUI 冒烟；所有完成标准仍需在实现阶段由 CI 或用户实机验证。
- `openspec validate daemon-workspace --strict` 已通过；`git status` 确认本变更目录之外没有工作区修改。

## 复审记录（2026-11-17）

**复审结论：带条件通过。** 原评审 C1–C12 中 7 条已关闭（C1、C2、C3、C5、C8、C9、C11），5 条部分关闭（C4、C6、C7、C10、C12）。修订没有推翻原评审的架构判断，也没有引入阻断级的新冲突；剩余缺口集中在 pane 状态语义、持久化强度、配置与载荷的读取点、前置顺序和关闭路径覆盖，收紧条文并补任务即可关闭，因此新增 D1–D8，维持“带条件通过”。

**复审方法与边界。** 逐条重读修订后的 `proposal.md`、`specs/workspace-state/spec.md`、`design.md`、`tasks.md`、`specs/session-host/spec.md` 与 `session-daemon/` 五件套，用 `rg`/`sed` 回查源码行号与实际行为，重跑 `openspec validate daemon-workspace --strict`（通过，带一条 INFO）。全程未运行任何 `cargo`、架构检查或发布脚本，未写入除本文件之外的任何文件。原 Findings 中的 `design.md`/`spec.md` 行号属于首轮评审时的版本；每条 C 条件后的「状态（复审 2026-11-17）」与本节表格给出修订后的行号。

### C1–C12 关闭状态

| 条件 | 状态 | 复审证据 |
| --- | --- | --- |
| C1 升级与停服路径 | 已关闭 | `design.md:374-384`（`Absent`/`Ready`/`Incompatible` 三态、`daemon_stop` 不 ensure、v1 握手回退、任何分支不 spawn）、`spec.md:294-315`（三个场景）、代码前提 `handshake.rs:93-100`、`protocol.rs:49`。残留可观察性问题见 D4。 |
| C2 回执与 revision | 已关闭 | `design.md:199-221`（`= 21`、request_id/expected_revision、四种 outcome、revision 规则、回执与快照同 revision、`>=` 替换规则）、`spec.md:192-218`（四个场景）、`tasks.md:24`/`:37`/`:72`。 |
| C3 actor 边界与 Close 路由 | 已关闭 | `design.md:110-163`（命令集、效果、`Close` 路由、`SessionExited` 幂等、resize 仅 controller、锁顺序、可用性刷新触发）、`spec.md:144-146`、`tasks.md:23-28`。 |
| C4 操作 schema 与错误路径 | 部分关闭 | 17 个 op、`SetPaneCwd`、错误码、快照字段、路径归一化分离均已落地（`design.md:168-195`、`:273-306`）；未闭合：D1（`Dormant`/`Unavailable` 边界与 `OpenProject` 初始 tab 语义）、D2（`active_pane` 两级定义）、D4（`storage_read_only` 无触发点）。 |
| C5 attach 时物化 | 已关闭 | `spec.md:117-136` 统一为“首个 attach 物化活跃 tab、其余激活时物化”，`design.md:317-325`，`tasks.md:29`/`:31`；不再有“客户端窗口期 ls/status 无布局”的语义。 |
| C6 持久化与格式保护 | 部分关闭 | 结构、debounce、writer 串行、shutdown flush、`format_version` 保护、写失败重试均已定义（`spec.md:57-99`、`design.md:229-241`、`tasks.md:16-18`/`:30`）；未闭合：D3（`MUST NOT 丢失` 与回执先于 fsync 的时序）、D4（错误目录与可观察性）。 |
| C7 配置根 | 部分关闭 | `design.md:245-262`、`tasks.md:15`/`:55` 给出 `config_root()`、`CROSSH_CONFIG_DIR`、macOS 路径、debug/release 迁移、测试隔离与 detached 继承；未闭合：D6（`crossh-core/src/locale.rs:41-43` 第三个 settings 读取者）。 |
| C8 旧设置迁移 | 已关闭 | `spec.md:265-292`、`design.md:327-343`（daemon 启动前、独立 legacy 结构、标记独立于空工作区、CLI 先 open、快照与备份、失败重试、旧字段不覆盖、删除 `ImportLegacy`）、`tasks.md:19`/`:40`/`:55`。 |
| C9 `crossh ls` | 已关闭 | `design.md:394-403`（保留 ROLE、前五列顺序、临时会话占位、status 两行）、现状格式 `main.rs:265-283`、`client.rs:489`、`tasks.md:62`。列语义仍有 S10。 |
| C10 `crossh open` 与单实例 | 部分关闭 | `design.md:384-392`（五步、单实例只唤起窗口）、`proposal.md:14`/`:46`、`tasks.md:60-61` 已落地；D5（真实载荷位于 `crates/crossh-core` 且与 git/note 共用）未闭合。 |
| C11 GUI 迁移清单 | 已关闭 | `design.md:345-372`（`ActiveView`/`compose`/git 状态键迁移、读取点、项目顺序、`new_tab`、快捷键）、`tasks.md:45-54`（含 A/B/C 三组 `rg` 清单）、`proposal.md:44-46`；行号复核全部命中。 |
| C12 与 session-daemon 衔接 | 部分关闭 | 前置条件、`session-host` MODIFIED delta、六处关闭调用点、自动重启风险说明均已落地（`proposal.md:13-14`、`specs/session-host/spec.md:1-31`、`design.md:411-426`）；`openspec validate --strict` 的 INFO 已写成归档阻断。未闭合：D7（apply/归档/实施三种措辞）、D8（分屏折叠与退休路径未列入）。 |

### 复审确认的修复

- `ensure_daemon` 的三态与 `daemon_stop` 的 v1 回退符合 `handshake.rs` 的真实版本/令牌检查顺序；v1 的 `Shutdown = 16` 与空能力载荷兼容。
- `WorkspaceMutateReply = 21` 与现有 `MessageKind` 1–16 不冲突（`protocol.rs:17-49`）；`>=` 替换、`<` 丢弃的客户端规则与同 revision 重发自洽。
- actor 效果（`Spawn`/`CloseSession`/`ResizeSession`）、锁方向 `Sessions → OwnershipMirror` 与 `SessionExited` 幂等在 `design.md:112-161` 成体系，且与 `serve.rs:61`/`:81`/`:329-333` 的现状前提一致。
- 17 个 op 的字段、前置条件、默认值、错误码与快照结构已完整列成表，`SetPaneCwd` 的 dormant/live 区分与 `pane_live` 拒绝均可实现。
- 路径归一化明确拆成 `normalize_project_root`/`normalize_pane_cwd`，与 GUI 只接受存在目录的 `local_paths.rs:17-23` 分离，解决了“缺失根被丢弃”的根因。
- 迁移改为 daemon 启动、开始服务前执行，独立 legacy 结构 + `settings.toml.pre-workspace` 快照 + `legacy_imported` 标记 + 写失败不设标记，完整覆盖 CLI 先 open 与旧字段被覆盖的风险。
- `crossh ls` 保留 `ID ROLE ALIVE PROGRAM CWD` 顺序并追加层级列，`daemon status` 保留第一行原文，向后兼容脚本解析。
- 六处显式关闭调用点的行号（`crates/crossh-terminal-view/src/view.rs:864`、`tabs.rs:203`/`:208`、`split.rs:127`、`tab_strip.rs:301`/`:373`、`shell/mod.rs:363`、`quit.rs:41-47`）逐一命中。
- `RenameTab`/`SetTabCommand` 有对应 UI 改造任务（`tasks.md:47` 的 5.4 写“重命名/默认命令发对应 op”），覆盖表 16 个 requirement 均有任务。
- `openspec validate daemon-workspace --strict` 实测 exit 0，INFO 为 `session-host: target spec does not exist`；`design.md:413`、`tasks.md:64`、`tasks.md:122` 已把它写成阻断条件（措辞问题见 D7）。

### 复审新发现

#### 阻断

无。

#### 重要

**D1（C4）`OpenProject` 的初始 tab 语义与 pane 状态存在多重读法。**

- `spec.md:222` 只写“打开已存在的项目 MUST 激活它并保证至少有一个 tab”，没有点名操作；`design.md:173` 让 `OpenProject` 对已存在项目保证至少一个 tab，对根缺失的新项目建“项目 + dormant pane”，却没有定义“新项目且根存在”时是否建 tab/pane；`design.md:176` 又让 `ActivateProject` “不新建 tab”。同一份工作区里，迁移来的空项目（`design.md:334`，无 tab）被 sidebar 激活时保持 0 tab，被 `crossh open` 打开时却要补一个 tab，两条路径给出不同结果而规格没有区分。
- “缺失根建项目 + dormant pane”（`design.md:173`）与“最近目录导入为空项目”（`design.md:334`）本身不冲突：后者不建 pane，前者只补一个 pane；冲突在于同一个空项目经 `OpenProject` 会得到 tab、经 `ActivateProject` 保持 0 tab。
- 建议：规格改成“`OpenProject` MUST 激活并保证至少一个 tab；`ActivateProject` MUST 只切换活跃项目，不创建 tab；空项目激活后的 `CreateTab` 用项目根作初始 cwd”，与 `spec.md:224-228` 的场景一致。`CreateTab` 的默认 cwd 本身已经明确（`spec.md:222`/`:228`、`design.md:175`），无需再改。
- `Dormant` 与 `Unavailable` 的边界自相矛盾：`spec.md:250` 写“cwd 不存在时保留原值并保持休眠”，`spec.md:260-264` 的场景却期望“标为不可用”；`design.md:185` 的 `SetPaneCwd` 允许“保留并保持 dormant/不可用”，`design.md:407` 的物化又规定 cwd 不存在即 `Unavailable`。项目 `available=false` 时 pane 应显示什么状态也没有规定。
- 建议给一张判定表：`Dormant` = 尚未物化或物化失败；`Unavailable` = 项目根或 pane cwd 缺失，且项目不可用时其 pane 一律 `Unavailable`；同步 `spec.md:248-264`、`design.md:173`/`:185`/`:407` 与 `tasks.md:11`。

**D2（C4）`active_pane` 同时在 workspace 与 tab 两级出现，缺少一致性规则。**
模型只有 `Tab.active_pane`（`design.md:64`），快照却在 `WorkspaceSnapshot`（`design.md:277`）与 `TabSnapshot`（`design.md:291`）两级都带 `active_pane`，GUI 读的是 `snapshot.active_pane`（`design.md:361`）。`ActivatePane`（`design.md:183`）只设置 `tab.active_pane` 与 `active_project`，没有设置 `active_tab`，因此从非活跃 tab 激活 pane 后，workspace 级字段指哪个 tab 没有定义，客户端可能把焦点画在错误的 tab。建议删除 workspace 级字段，或把它定义为“活跃 tab 的 `active_pane` 镜像”并让 `ActivatePane` 同时设 `active_tab`；`tasks.md:11` 需要写明投影哪一级并加一条一致性测试。

**D3（C6）耐久性 MUST 与“回执先于 fsync”的时序冲突。**
`spec.md:59` 写“结构变更 MUST NOT 丢失”，`design.md:219` 的时序是“应用 → 序列化并入队 → 回执 → 广播”，`design.md:237` 以“入队即写”解释结构变更不丢。入队只保证 writer 最终会写，回执返回时 `sync_all` 与 `rename` 未必完成；崩溃可能恰好落在“已回执、未落盘”的窗口。建议二选一：回执前等待该次结构写入的 fsync&rename（结构变更频率低，代价可接受），或把 `spec.md:59` 改成“崩溃最多丢失最近 500 ms 的元数据与一次未完成的写入”，与 500 ms 边界和 `tasks.md:18` 的验证保持一致。

**D4（C1/C4/C6）`storage_read_only` 无触发路径，格式过新的原因在 CLI 上不可观察。**
`design.md:193` 把 `storage_read_only` 列为固定错误码，但 D7 没有任何路径返回它：目录不可写被定义为 `storage_error` + 退避重试（`design.md:234`），`format_version` 过新则 daemon 直接不发布端口文件、非零退出（`design.md:239-240`），`tasks.md:60` 却把“格式过新导致 `storage_read_only`”写成验证场景。同时“提示升级 crossh”只进入日志：daemon 以 detached 启动，stdout/stderr 均为 null（`client.rs:99-104`），`crossh daemon status` 在连接失败时固定打印 `not running (stale port file)`（`main.rs:211-225`），`design.md:403` 又保留第一行原文，用户看不到版本原因。这既没有满足 `spec.md:61` 对格式过新“以非零状态退出且提示升级”的要求，也与 `spec.md:294-298` 对存活但不兼容的 daemon “报告可执行的错误”不一致。建议删除 `storage_read_only` 或给出真实触发条件；让 `daemon status` 区分“端口文件在但握手不兼容”和“陈旧端口文件”；把格式过新的原因通过退出码、`run.log` 路径或 CLI 错误行暴露。

**D5（C10）单实例载荷的真实实现位于 `crossh-core`，且与 git/note 共用转发函数。**
`proposal.md:46` 与 `tasks.md:61` 只点名 `src/app/single_instance.rs`，该文件仅做 re-export（`src/app/single_instance.rs:7-9`）；真实实现是 `crates/crossh-core/src/single_instance.rs:88` 的 `try_forward(Option<&Path>)`、`:93`/`:98` 的 `try_forward_git`/`try_forward_note`、`:102` 的 `try_forward_to`、`:139-145` 的 `write_request` 与 `:214-221` 的 `read_and_validate`。把主窗口载荷改成“显示窗口”时若直接改共享函数，会同时影响 `crossh git` / `crossh note` 的目录请求；Impact 与任务也没有把 `crates/crossh-core` 列入。线格式本身已经支持空行表示“仅聚焦”（`read_and_validate` 注释与 `:216-218`），所以旧 GUI 收到新载荷会退化为“只唤起窗口”，这可以接受，但应写进设计而不是留给推断。建议：Impact 与 task 6.2 加入 `crates/crossh-core/src/single_instance.rs`；主窗口使用独立的无参转发入口或独立的校验，保留 git/note 的目录语义，并把“旧 GUI 只聚焦”的混合版本行为写进测试。

**D6（C7）`locale.rs` 是第三个 settings 路径读取者，绕过 `config_root()`。**
`crates/crossh-core/src/locale.rs:41-43` 硬编码 `~/.config/crossh/settings.toml`，`src/bin/crossh-git.rs:53` 与 `src/bin/crossh-note.rs:60` 用它读取语言设置。D8 声称“一个解析器，两个文件族”（`design.md:245-257`），tasks 2.1/5.9 只改 `persistence.rs` 与 `crossh-session`；设置 `CROSSH_CONFIG_DIR` 后 GUI 读写 `$CROSSH_CONFIG_DIR/crossh/settings.toml`，git/note 仍读家目录，语言设置会分叉。建议把该读取点路由到共享解析器，或在 design/tasks 中显式声明 git/note 继续使用旧路径并加测试覆盖。

**D7（C12）前置顺序有三种措辞，且 session-daemon 4.4 未作废。**
`proposal.md:13` 要求 session-daemon “先 apply 并归档”；`design.md:413` 允许归档前“实施代码”、只禁止归档，却称之为“apply 阻断条件”；`tasks.md:64` 允许“代码实施可在归档前开始”；`tasks.md:122` 又把“未归档”写成“apply 阻断条件”。三处分别把阻断点放在 apply、归档与实施。建议统一为“可实施、不可归档”，并把 `tasks.md:122` 的标签改成归档阻断。另外 `session-daemon/tasks.md:41` 的 4.4（固定标签按 id 恢复）仍未完成，本变更删除 `pinned.rs` 与 `daemon_session_id` 后该任务无法完成；应在 `proposal.md:14` 的取代声明中一并点名 `session-daemon/tasks.md:41` 与 `session-daemon/review.md:231`。

**D8（C12）显式关闭调用点清单漏掉分屏折叠与退休路径。**
`design.md:419-424` 的六个调用点行号全部正确，但 `rg` 还命中三类会结束会话的路径：`toggle_terminal_split`（`shell/split.rs:39-78`，批量 `close_local_session` 在 `:66`/`:72`；入口 `view.rs:722`、`command_palette.rs:313`）、`retire_split_pane`（`shell/split.rs:156-168`）、`prepare_terminal_split_view_close`（`shell/split.rs:172-220`）。这些路径在本变更后必须改发 `ClosePane`/`CloseTab`，否则 GUI 仍会本地结束布局会话；`tasks.md:53` 只要求核对六个调用点。建议把三类路径加入 D15 的表与 task 5.7，并明确“退出分屏时保留为普通 tab”在新模型下对应折叠 split，而不是结束会话。

#### 建议

**S10. `crossh ls` 的层级列未定 id 还是名称，且 CWD 不再是最后一列。** `design.md:394-397` 只定列名；若 PROJECT/TAB/PANE 用名称，含空格的名称会破坏按空格解析；即使全用 id，CWD 含空格时后接三列也无法按空白切分。建议明确三列输出稳定 id、把 CWD 保持为最后一列（新列插在 CWD 之前），或在 `--format json` 中提供结构化输出。`tasks.md:62` 目前只验证列、占位符与 ROLE 易手，没有覆盖含空格名称，建议一并补上。

**S11. 迁移快照的完整路径未写全。** `design.md:331`/`:339` 只写 `settings.toml.pre-workspace`，应写明它位于 settings 根 `config_root()/crossh/settings.toml.pre-workspace`（与 workspace 的 `crossh[-dev]` 分离），并说明 debug/release 各自迁移时都读取这同一份只读快照（`design.md:259`）。`tasks.md:19` 的验证已按此意图书写，条文中补全即可。

**S12. `StopProject`/`ForgetProject`/`CloseTab` 的副作用列表与时序未定义。** `design.md:174`/`:175`/`:178` 只写“结束会话/关闭全部 pane”，没有说明每个 live pane 发一次 `CloseSession`、显式解绑与结束会话的先后、回执是否在会话真正结束前返回，以及 `ForgetProject` 之后迟到的 `SessionExited` 如何处理（依赖 `design.md:151-153` 的“找不到映射就忽略”，但需要先保证解绑发生）。这些操作按 `specs/session-host/spec.md` 属于“会话被显式关闭”，与自动重启/窗口关闭不同，建议补效果列表与顺序，并在 `tasks.md:25` 增加迟到退出事件的测试。

**S13. 测试隔离还需要 `CROSSH_CACHE_DIR`。** `tasks.md:15` 只要求注入 home 与 `CROSSH_CONFIG_DIR`；现有 daemon 端口路径由 `instance.rs:24-31` 的 `CROSSH_CACHE_DIR`（或 `-dev` 缓存根）决定，不隔离会连到开发机上真实运行的 daemon。建议 tasks 2.1/7.x 同时设置两个变量。

**S14. 迁移 MUST 与失败继续的措辞需要对齐。** `spec.md:267` 写“MUST 在启动、开始服务之前导入”，`design.md:335` 又允许持久化失败时“保持未迁移状态并继续启动”。建议写成“迁移尝试 MUST 先于开始服务；失败时以未迁移状态继续服务并在下次启动重试”，避免把“必须完成”读成启动前置条件。

**S15. 同 revision 快照允许字段不同，但缺少不变量。** `design.md:163` 允许 `RefreshAvailability` 在 revision 不变时重发快照，`design.md:219` 的客户端规则是“同 revision 整体替换”。若两次同 revision 快照的 `available` 不同，客户端接受哪一份取决于到达顺序。建议明确“同 revision 只允许运行时字段（`available`/`PaneStatus`/`storage_error`）不同，其余布局字段必须一致”，或在可用性变化时推进 revision。

**S16. “协议仍为 v2”的措辞与现码不符。** `design.md:199` 写“协议仍为 v2”，而 Context 已说明当前 `PROTOCOL_VERSION = 1`（`design.md:12`、`handshake.rs:10`）。建议改成“本变更把协议升到 v2，工作区消息都在这一版内”，避免读者误以为现网已是 v2。

**S17. `crossh open` 固定 `expected_revision: None` 会静默覆盖并发修改。** `design.md:388` 的请求不带 revision，按 last-writer-wins 处理；当 GUI 正在操作同一项目时，CLI 的 `OpenProject` 可能把项目重排/激活到用户不期望的位置。建议 GUI 已运行时把 `OpenProject` 交给 GUI 自己发（复用已知 revision），或在设计里接受并说明该覆盖语义（`design.md:217` 已有通用规则，D13 补充一句即可）。

**S18. 四件套仍有复述。** `proposal.md:17-28` 的 What Changes 与 `design.md` 的 D1–D14 重复了持久化、物化、迁移、CLI 的结论；`tasks.md:76-116` 的两张覆盖表又把 requirement 与 design 决策重列一遍。与原评审 S3 同类，属于维护成本而非阻断项，建议后续把 proposal 收敛为动机/范围、表格留在 tasks。

**S19. `StorageErrorInfo` 与 `default_command` 的执行者仍未定义。** `design.md:279` 使用 `Option<StorageErrorInfo>` 但没有给出字段；`default_command` 的自动执行仍留在 GUI（`design.md` Open Questions），而 `RenameTab`/`SetTabCommand` 已由 daemon 持久化。建议补 `StorageErrorInfo { kind, message, path? }` 的最小字段，并在 tasks 1.5/5.x 写明默认命令的执行时机。

**S20. `CROSSH_CONFIG_DIR` 的语义与 `CROSSH_CACHE_DIR` 不对称。** D8 把 `CROSSH_CONFIG_DIR` 定义为“整体替换 `~/.config`”（`design.md:248`），而 `CROSSH_CACHE_DIR` 直接指到 `crossh`/`crossh-dev` 根（`instance.rs:24-27`）。若用户按后者习惯把变量指到 `~/.config/crossh`，会得到 `~/.config/crossh/crossh/settings.toml`。建议在 design 与 tasks 2.1 的测试名里显式写“变量值是 `~/.config` 的替代目录”，并加一条“值已包含 crossh 时不重复拼接”或明确拒绝的断言。

### 复审仍未验证事项

- 未运行任何 `cargo build` / `cargo check` / `cargo clippy` / `cargo test` / `cargo fmt` / `scripts/check-architecture.sh`；`workspace.rs`、`workspace_store.rs`、`workspace_actor.rs`、协议 v2 与 GUI 投影能否编译、是否满足 2000 行限制均未验证。
- 未运行 v2 与 v1 的真实互操作：`daemon_stop` 用 v1 握手时，v2 客户端是否接受 `Welcome.version = 1`、是否忽略 v1 应答的未知/缺失字段，均只有静态推断（`handshake.rs:31-36`/`:93-100`）。
- 未在 macOS 实机验证 `dirs::home_dir().join(".config")`、`0700`/`0600` 权限、detached 进程的环境继承与 `CROSSH_CONFIG_DIR` 传播；Linux/Windows 按仓库约束交给 CI。
- 未执行迁移集成测试、故障注入（rename 前后 kill、写失败退避）、GUI 冒烟与 `crossh ls` 列解析测试。
- 未验证同 revision 快照在真实 `watch` 通道上的到达顺序，以及 `RefreshAvailability` 与并发 mutation 交错时客户端是否出现一次错误焦点。

## 条件状态（复审 2026-11-17）

| 条件 | 状态 | 关闭证据 / 未闭合缺口 |
| --- | --- | --- |
| C1 升级与停服路径 | 已关闭 | `design.md:374-384`、`spec.md:294-315`、`handshake.rs:93-100`；残留可观察性问题转入 D4。 |
| C2 回执与 revision | 已关闭 | `design.md:199-221`、`spec.md:192-218`、`tasks.md:24`/`:37`/`:72`。 |
| C3 actor 边界与 Close 路由 | 已关闭 | `design.md:110-163`、`spec.md:144-146`、`tasks.md:23-28`。 |
| C4 操作 schema 与错误路径 | 部分关闭 | 已落地 17 op、`SetPaneCwd`、错误码、快照字段、路径归一化；D1/D2/D4 未闭合。 |
| C5 attach 时物化 | 已关闭 | `spec.md:117-136`、`design.md:317-325`、`tasks.md:29`/`:31`。 |
| C6 持久化与格式保护 | 部分关闭 | 结构/debounce/writer/flush/格式保护齐备；D3/D4 未闭合。 |
| C7 配置根 | 部分关闭 | `design.md:245-262`、`tasks.md:15`/`:55`；D6 未闭合。 |
| C8 旧设置迁移 | 已关闭 | `spec.md:265-292`、`design.md:327-343`、`tasks.md:19`/`:40`/`:55`。 |
| C9 `crossh ls` | 已关闭 | `design.md:394-403`、`main.rs:265-283`、`tasks.md:62`；S10 待明确。 |
| C10 `crossh open` 与单实例 | 部分关闭 | `design.md:384-392`、`proposal.md:14`/`:46`、`tasks.md:60-61` 已落地；D5 未闭合。 |
| C11 GUI 迁移清单 | 已关闭 | `design.md:345-372`、`tasks.md:45-54`、`proposal.md:44-46`。 |
| C12 session-daemon 衔接 | 部分关闭 | delta、六处调用点与 validate INFO 已落地；D7/D8 未闭合。 |
| D1 `OpenProject` 初始 tab 与 pane 状态 | 未关闭 | 见「重要 D1」：`spec.md:222`/`:250`/`:260-264`、`design.md:173`/`:176`/`:185`/`:407`。 |
| D2 `active_pane` 两级定义 | 未关闭 | 见「重要 D2」：`design.md:64`/`:183`/`:277`/`:291`/`:361`。 |
| D3 结构变更耐久性边界 | 未关闭 | 见「重要 D3」：`spec.md:59`、`design.md:219`/`:237`、`tasks.md:18`。 |
| D4 `storage_read_only` 与格式错误可观察性 | 未关闭 | 见「重要 D4」：`design.md:193`/`:239-240`/`:403`、`tasks.md:60`、`main.rs:211-225`。 |
| D5 单实例载荷归属与混跑行为 | 未关闭 | 见「重要 D5」：`crates/crossh-core/src/single_instance.rs:88`/`:93`/`:98`/`:102`/`:139`/`:214`、`proposal.md:46`、`tasks.md:61`。 |
| D6 `locale.rs` settings 路径 | 未关闭 | 见「重要 D6」：`crates/crossh-core/src/locale.rs:41-43`、`src/bin/crossh-git.rs:53`、`src/bin/crossh-note.rs:60`。 |
| D7 前置顺序措辞与 session-daemon 4.4 | 未关闭 | 见「重要 D7」：`proposal.md:13`、`design.md:413`、`tasks.md:64`/`:122`、`session-daemon/tasks.md:41`。 |
| D8 分屏折叠与退休关闭路径 | 未关闭 | 见「重要 D8」：`shell/split.rs:39-78`/`:156-168`/`:172-220`、`design.md:419-424`、`tasks.md:53`。 |

**最终结论：带条件通过。** D1–D8 关闭并回填本表之后，C4、C6、C7、C10、C12 转为已关闭。在此之前不得进入 apply 的完整条件集（C1–C12 + D1–D8）以本表为准。

## 最终复审（2026-11-17）

**最终结论：带条件通过。** 第二轮修订关闭了 D2、D5、D7、D8；D1、D3、D4、D6 仍有实现前缺口，新增条件 E1–E7。C4、C6、C7 保持“部分关闭”，C10、C12 转为“已关闭”。本节取代文末「条件状态（复审 2026-11-17）」的结论；原 C1–C12、D1–D8 与 S1–S20 的历史记录全部保留。

**复审边界。** 只读重读四件套、`specs/session-host/spec.md`、`session-daemon/` 的 tasks/review、`openspec/config.yaml`，并用 `rg`/`nl` 回查源码行号与行为；重跑 `openspec validate daemon-workspace --strict`（通过，仍有一条 `target spec does not exist` INFO）。全程未运行任何 `cargo build/run/check/clippy/test/fmt`、`scripts/check-architecture.sh` 或发布脚本，未写入除本文件之外的任何文件；`git status --short` 仅显示未跟踪的 `openspec/changes/daemon-workspace/`。

### D1–D8 最终状态

| 条件 | 最终状态 | 关闭证据 / 剩余缺口 |
| --- | --- | --- |
| D1 `OpenProject` 分支、`ActivateProject` 不建 tab、pane 判定表 | 部分关闭 | 三分支、`ActivateProject`、`Dormant`/`Unavailable` 旧矛盾均已落地；剩 R6/D10/3.7 的“每个 leaf”与 R12/D5/D14 的“不可用不 spawn”直接冲突、`OpenProject` 未列入物化触发、`PaneStatus` 优先级会遮蔽 live 会话绑定（E2、E6）。 |
| D2 `active_pane` 两级定义 | 已关闭 | `WorkspaceSnapshot` 已无 workspace 级 `active_pane`（`design.md:294-299`）；活跃 pane 由 `active_project → active_tab → active_pane` 推导（`design.md:336`）；`ActivatePane` 同步三级（`design.md:183`）；`tasks.md:11`/`:44` 有投影与测试。 |
| D3 结构变更 fsync、失败回执 | 部分关闭 | fsync 前回执与“失败仍 Applied”的耐久性例外已对齐（`spec.md:59`/`:61`、`design.md:233`/`:252`/`:255`、`tasks.md:18`）；但 `WorkspaceMutateReply` 没有承载 `storage_error` 的字段，且错误清除后的同 revision 快照重发路径未定义（E1）。 |
| D4 `storage_read_only` 与格式错误可观察性 | 部分关闭 | `storage_read_only` 已从四件套删除（`rg` 仅命中本文件历史）；`daemon.error` + `daemon status` 的格式过新路径已闭环（`design.md:257`/`:437`、`spec.md:101-106`、`tasks.md:16`/`:62`/`:72`）。剩 `crossh daemon status` 对“端口可达但握手不兼容”仍会报 stale、`tasks.md:60` 的 `not_found`/`Conflict` 示例不可触发（E3、E5）。 |
| D5 单实例载荷归属与混跑行为 | 已关闭 | Impact 与任务已覆盖 `crates/crossh-core/src/single_instance.rs`（`proposal.md:43`、`tasks.md:61`）；主窗口与 git/note 载荷分离（`design.md:424-426`、`tasks.md:61`/`:73`）；混合版本行为可测试（`design.md:426`、`tasks.md:73`）。任务 5.6 的 C 组 `rg` 范围错误另列 E3。 |
| D6 `config_root()` 与其读取点 | 部分关闭 | `config_root()`/`settings_path()`/`workspace_path()` 落在 `crates/crossh-core/src/config/mod.rs`（`tasks.md:15`）；`locale.rs:41-43`、`workspace_store.rs`、`persistence.rs` 共用（`design.md:279`、`tasks.md:15`/`:55`）；`CROSSH_CONFIG_DIR` 直接作为根、不重复拼接（`design.md:265-275`）；备份与默认路径一致（`design.md:360`）。剩 debug 旧 settings 实际在 `crossh` 根而新 debug 根为 `crossh-dev`，`design.md:279` 的“旧版本升级只发生在各自根下”不成立（E4）。 |
| D7 “可实施、不可归档”与 session-daemon 4.4 | 已关闭 | `proposal.md:13-14`、`design.md:447`、`tasks.md:64`/`:122` 统一为“代码实施可在归档前开始、归档前必须已归档 session-daemon”；`proposal.md:14` 点名 `session-daemon/tasks.md:41` 与 `review.md:231` 的 C7；`session-host/spec.md:7` 把 delta 合入写成归档前提。 |
| D8 分屏折叠与退休关闭路径 | 已关闭 | `design.md:459-461` 已列入 `toggle_terminal_split`、`retire_split_pane`、`prepare_terminal_split_view_close`；`tasks.md:53` 要求这三条路径发 `ClosePane`；`design.md:463` 明确“退出分屏 = 折叠 split，不是结束 tab”，与 `spec.md:156-160` 的 ClosePane 场景一致。 |

#### D1（C4）`OpenProject` / `ActivateProject` / pane 状态

已关闭的部分：

- `OpenProject` 的三个分支已明确：已存在 → 激活并补齐至少一个 tab；新项目且根存在 → 插到最前并建 tab + 单 pane；根缺失 → 建 `available=false` 的项目 + tab + 单 pane（`design.md:173`）。`spec.md:235` 的 MUST 与 `spec.md:244-249` 的“打开根缺失的新项目”场景一致；`tasks.md:9`（1.3）逐项落地。
- `ActivateProject` 只切换活跃项目与 `active_tab`，空项目保持 `None`、不建 tab（`design.md:176`）；`spec.md:235`/`:237-242` 与 `tasks.md:9` 一致。
- `Dormant`/`Unavailable` 的旧矛盾已消除：cwd 缺失现在统一为 `Unavailable(cwd_missing)`（`spec.md:277`、`design.md:200`），`SetPaneCwd` 接受缺失 cwd 并保持不可用（`design.md:185`），不再出现“保留并休眠”与“不可用”两种读法。项目不可用时其 pane 一律 `Unavailable(project_root_missing)`（`design.md:199`、`spec.md:293-298`）。
- “缺失根建 tab + pane”与“最近目录导入空项目”的差异是刻意的：迁移导入最近目录时建 0 tab 的空项目（`spec.md:302`、`design.md:363`），显式 `OpenProject` 则保证至少一个 tab（`design.md:173`、`spec.md:235`）；`ActivateProject` 对空项目保持 `active_tab = None`（`design.md:176`、`spec.md:237-242`）。三者不构成新的不一致。

未闭合的部分：

1. **“物化每个 leaf”与“不可用不 spawn”直接冲突。** `spec.md:126` 要求首个 attach 后为活跃 tab 的**每个 pane**创建会话；`design.md:354` 与 `tasks.md:29`（3.7）要求物化活跃 tab 的**每个 leaf**。但 `spec.md:277`、`design.md:205`/`:441` 与 `tasks.md:28`（3.6）要求项目不可用或 pane cwd 缺失时 **MUST NOT** spawn。活跃 tab 完全可能包含 `Unavailable` pane（`OpenProject` 缺失根就创建一个，`spec.md:244-249`；cwd 也可能在重启前被删除）。按字面实现会违反其中一条 MUST；`tasks.md:29` 的验证“attach 并断言活跃 tab 有会话”在没有排除不可用 pane 时也无法通过。
2. **`OpenProject` 不在物化触发列表。** `design.md:354` 的触发只有首个 attach、`ActivateTab`/`ActivateProject`、`CreateTab`/`SplitPane`/`EnsurePaneSession`；`OpenProject` 会建 tab + pane（`design.md:173`），但未列入 `design.md:354` 或 `tasks.md:29`。当 GUI 已经 attach 时，`crossh open` 打开一个空项目或缺失根的新项目不会触发 attach/激活之外的物化，新 pane 会停在 `Dormant`，直到用户做别的动作。`spec.md:251-255` 对 `CreateTab` 承诺“随即被物化”，对 `OpenProject` 没有对应承诺。
3. **`PaneStatus` 的优先级会遮蔽 live 会话绑定（新增，E6）。** `spec.md:277` 与 `design.md:195-205` 的判定顺序是“项目不可用 → cwd 缺失 → Starting/Live/Dormant”，先命中者生效；而 `design.md:334` 又说 `PaneStatus::Live` 是 pane 与会话的**唯一**绑定来源。一个 live shell 的 cwd 被删除（或项目根被删除）时，快照会返回 `Unavailable` 而不是 `Live`，客户端因此拿不到 `session_id`，无法重新接入仍在运行的会话。与此同时 `design.md:185` 的 `SetPaneCwd` 只拒绝 `Live`、允许 `Unavailable`，于是这种 pane 又会被允许修改 cwd，和“live → `pane_live`”的规则矛盾。

#### D2（C4）`active_pane` 两级

已关闭。`WorkspaceSnapshot`（`design.md:294-299`）只有 `active_project`，没有 workspace 级 `active_pane`；`design.md:336` 明确“活跃 pane 只有两级”，由 `active_project → ProjectSnapshot.active_tab → TabSnapshot.active_pane` 推导；`ActivatePane` 同时写 `tab.active_pane`、`project.active_tab` 与 `workspace.active_project`（`design.md:183`）；`tasks.md:11` 与 `tasks.md:44` 分别覆盖快照投影和 GUI 合并测试。`spec.md:207` 的同 revision 不变量也按三级布局字段书写。未发现残留的第二套定义。

#### D3（C6）结构变更耐久性与失败回执

已关闭的部分：

- `spec.md:59` 明确“结构变更 MUST 在返回修改回执前完成原子写入与 fsync”；`design.md:233` 的时序是“应用 → 序列化 → 投 writer → 结构变更等 writer 完成原子写入与 fsync（或明确报告失败）→ 回执 → 广播快照”；`tasks.md:18`（2.4）要求结构回执在 writer ack 之后。
- 失败例外在 spec 内部自洽：`spec.md:59` 只对“已回执且未携带存储错误”的结构变更承诺不丢失；`spec.md:61` 要求写失败时结构回执携带错误以表示未落盘；`design.md:255` 把“写入失败时携带 `storage_error` 的回执”写成唯一例外。因此“`Applied` + `storage_error`”本身不是 `MutateOutcome` 冲突，只要回执有字段承载它。
- `design.md:252` 的“结构写入失败时仍回执 `Applied`、快照携带 `storage_error`、客户端显示未落盘、重试成功后清除”与 `spec.md:87-92` 的“持久化写失败仍可服务”场景一致。

未闭合的部分（E1）：

- `WorkspaceMutateReply`（`design.md:222-228`）只有 `error: Option<OpError>`，没有 `storage_error` 字段；`design.md:193` 又明确“持久化写失败不是操作错误：错误只经快照的 `storage_error` 暴露”，与 `design.md:255` 的“携带 `storage_error` 的回执”和 `spec.md:61` 的 MUST 冲突。`tasks.md:18`（2.4）要求“失败期间的结构回执携带 `storage_error`”，但 `tasks.md:24`（3.2）与 `tasks.md:36`（4.2）的字段清单都没有它。按当前 schema，spec 的 MUST 无法实现，也无法写 serde 测试。
- `storage_error` 的清除没有定义快照重发路径：`design.md:252` 说成功后清除，`design.md:235` 允许同 revision 只合并运行时字段，但 writer 成功后由谁通知 actor、是否立即重发一份同 revision 快照、客户端如何区分“错误仍在”与“错误已清除”都没有规定。需要与 `RefreshAvailability` 一样，把 `storage_error` 的变化纳入 actor 的运行时字段更新并重发快照。

#### D4（C1/C4/C6）`storage_read_only` 与格式错误的可观察性

已关闭的部分：

- `storage_read_only` 已从 `proposal.md`、`spec.md`、`design.md`、`tasks.md` 全部删除；全目录 `rg` 只命中本文件的历史记录。`design.md:193` 的 8 个错误码不再包含它。
- 格式版本过新的原因已可从 CLI 观察：`design.md:257` 写 `config_root()/daemon/daemon.error`（`0600`，与 `workspace.json` 同目录），成功启动删除；`design.md:437` 在端口文件缺失或不可连接时优先读它，输出 `not running: <原因>` 并非零退出；`spec.md:101-106` 给出对应场景；`tasks.md:16`（2.2）、`:62`（6.3）、`:72`（7.5）覆盖写入、读取与逐字节保护。

未闭合的部分：

- **`crossh daemon status` 对存活但不兼容的 daemon 仍会给错原因。** `design.md:406-410` 的三态只用于 `ensure_daemon`；`design.md:430-437` 的 status 只在“端口文件缺失或端口不可连接”时读 `daemon.error`。一个 v1 daemon 的端口是可达的，握手会失败，但 `daemon.error` 通常不存在（格式没有过新），当前 `main.rs:209-224` 会把任何 `connect_to` 错误都打印成 `not running (stale port file)`。这违反 `spec.md:331` 的“客户端发现常驻进程存活但版本不兼容时 MUST 报告可执行的错误”，也没有满足 D4 原条件中的“区分端口文件在但握手不兼容与陈旧端口文件”。
- **`tasks.md:60`（6.1）的拒绝示例不可触发。** `crossh open` 发送 `WorkspaceMutate { expected_revision: None, op: OpenProject }`（`design.md:418`）。`OpenProject` 没有目标对象，`design.md:169` 的“目标不存在 → `not_found`”不适用；`expected_revision` 为 `None`，`design.md:231` 的 `Conflict` 也不可能出现。任务却要求验证 `not_found` 与 `Conflict` 两种情况，测试无法按字面构造。应改为非法路径、daemon 启动失败/`daemon.error`、版本不兼容、回执超时；若要测 `Conflict`，用直接携带 `expected_revision` 的 `WorkspaceMutate` 独立测试。
- **`daemon.error` 删除失败的生命周期未定义。** `design.md:257` 要求成功启动后删除陈旧文件，但没有说删除失败时怎么办；如果删除失败，下一次端口缺失/不可连接时 status 可能展示上一次格式错误的旧原因，掩盖当前真实状态。至少需要规定“删除失败只记日志，并在展示前用 workspace 的当前 `format_version`/mtime 做一次校验”，或把错误文件改成带启动代次的结构。

#### D5（C10）单实例载荷

已关闭。`proposal.md:43` 的 Impact 已包含 `crates/crossh-core/src/` 的 `config/mod.rs`、`locale.rs:41-43` 与 `single_instance.rs`；`tasks.md:61`（6.2）明确真实实现在 `crates/crossh-core/src/single_instance.rs`（`src/app/single_instance.rs` 只是 re-export），主窗口改 `try_forward(None)`/`try_forward_window()`，`try_forward_git`/`try_forward_note` 的目录语义不变；`design.md:424-426` 说明空载荷已经表示“仅聚焦”、旧 GUI 收到空载荷只唤起窗口、新 GUI 收到旧目录载荷时忽略目录；`tasks.md:73`（7.6）覆盖 GUI 已运行、GUI 未运行、旧 GUI 混合版本三种路径。代码前提成立：`crates/crossh-core/src/single_instance.rs:88`/`:93`/`:98`/`:102`/`:139`/`:214` 与 `src/app/single_instance.rs:7-8` 的 re-export、`src/bin/crossh-git.rs:35`/`crossh-note.rs:46` 的调用点均与 design 描述一致。

任务 5.6 的 C 组 `rg` 范围错误另列 E3：它把 `try_forward`/`resolve_open_path` 限制在 `src/main.rs` 与 `src/app/cli.rs`，但定义在 `crates/crossh-core/src/single_instance.rs:74`/`:88`，re-export 在 `src/app/single_instance.rs:7-8`，且裸 `try_forward` 子串会命中 `src/bin/crossh-git.rs:35`/`crossh-note.rs:46` 的 `try_forward_git`/`try_forward_note`。该门禁按字面无法通过。

#### D6（C7）`config_root()` 与读取点

已关闭的部分：

- `config_root()`/`settings_path()`/`workspace_path()` 落在 `crates/crossh-core/src/config/mod.rs`（当前只有一行模块注释，`crates/crossh-core/src/config/mod.rs:1`）；`tasks.md:15`（2.1）要求 `locale.rs:41-43`、新增的 `workspace_store.rs` 与 `persistence.rs` 全部改调同一解析器。
- `CROSSH_CONFIG_DIR` 的值直接作为 crossh 配置根，不再拼接 `crossh`；未设置时用 `home_dir()/.config/crossh[-dev]`，不使用 `dirs::config_dir()`（`design.md:265-275`、`tasks.md:15`）。指到 `~/.config/crossh` 时得到 `~/.config/crossh/settings.toml`，不出现重复目录。
- settings 为 `config_root()/settings.toml`，workspace 为 `config_root()/daemon/workspace.json`；迁移快照为 `config_root()/settings.toml.pre-workspace`（`design.md:360`、`proposal.md:53`），release 路径与现有 `persistence.rs:97-102` 一致。
- debug/release 使用各自配置根、各自迁移标记，`crossh-dev` 不会消费 release 的一次性迁移（`design.md:277-279`）；`CROSSH_CONFIG_DIR` 指向同一根时两侧共享（`design.md:279`）。

未闭合的部分（E4）：

- 旧版本的 settings 路径没有 debug 后缀：`persistence.rs:97-102` 与 `locale.rs:41-42` 都硬编码 `~/.config/crossh/settings.toml`，当前 debug 构建写的是 `~/.config/crossh`，不是 `~/.config/crossh-dev`。新设计把 debug 根改成 `crossh-dev`（`design.md:269`），因此 `design.md:279` 的“旧版本升级只发生在各自根下”对 debug 不成立：debug 首次启动会读 `crossh-dev/settings.toml`（不存在），不会迁移旧 debug 的 `crossh/settings.toml`；`proposal.md:53` 让 debug 用户备份 `~/.config/crossh-dev/settings.toml` 也指向了不存在的文件。release 升级路径不受影响（旧 release 与新 release 同为 `crossh`）。
- 需要二选一：debug 在 `crossh-dev/settings.toml` 不存在时回退读取 `crossh/settings.toml` 做一次性迁移；或明确声明 debug 不迁移旧 settings，并在发布说明要求手动复制，同时修正 `proposal.md:53` 的 debug 备份路径。无论哪种，都要补一条旧 debug fixture 的测试。

#### D7（C12）“可实施、不可归档”与 session-daemon 4.4

已关闭。`proposal.md:13` 要求 `session-daemon` 先 apply、本变更代码可在其归档前开始、但本变更归档前 `session-daemon` MUST 已归档并合入 `session-host`；`design.md:447` 明确这是归档阻断条件、不是 apply 阻断条件；`tasks.md:64`（6.5）与 `tasks.md:122` 使用同一措辞；`proposal.md:14` 点名 `session-daemon/tasks.md:41`（4.4）与 `session-daemon/review.md:231`（C7）被本变更取代；`specs/session-host/spec.md:7` 把 delta 合入写成归档前提。`session-daemon/tasks.md:41` 目前仍是未勾选状态，按 `proposal.md:14` 应在该变更归档前标记取消；这属于 session-daemon 的收尾动作，不构成本变更的规格缺口。

#### D8（C12）分屏折叠与退休关闭路径

已关闭。`design.md:459-461` 的 D15 表已加入 `toggle_terminal_split`（`shell/split.rs:39-78`，批量关闭 `:66`/`:72`，入口 `view.rs:722`、`command_palette.rs:313`）、`retire_split_pane`（`shell/split.rs:156-168`）与 `prepare_terminal_split_view_close`（`shell/split.rs:172-220`）；`tasks.md:53`（5.7）要求这三条路径按“保留焦点 pane、其余发 `ClosePane`、tab 保留为单 pane”实现，并给出 `rg` 验证；`design.md:463` 明确“退出分屏是折叠 split，不是结束整个 tab”，与 `spec.md:156-160` 的 ClosePane 场景一致。行号复核全部命中。

### 原条件 C4、C6、C7、C10、C12 的最终状态

| 条件 | 原状态 | 最终状态 | 依据 / 剩余缺口 |
| --- | --- | --- | --- |
| C4 操作 schema 与错误路径 | 部分关闭 | 部分关闭 | 17 op、`SetPaneCwd`、错误码、快照字段、路径归一化均已落地（`design.md:168-209`、`:293-338`、`spec.md:235-298`、`tasks.md:9`/`:11`/`:28`）；D2 已关闭；D1（E2、E6）与 D4（E3、E5）仍有缺口。 |
| C6 持久化与格式保护 | 部分关闭 | 部分关闭 | 结构/debounce/writer/flush/格式保护/`daemon.error` 已齐备（`spec.md:57-106`、`design.md:245-259`、`tasks.md:16-18`/`:30`）；D4 的格式过新部分已关闭；D3（E1）与 D4 的 live-incompatible status（E5）未关闭。 |
| C7 配置根 | 部分关闭 | 部分关闭 | `config_root()` 已统一，`CROSSH_CONFIG_DIR` 与备份路径已明确（`design.md:263-279`、`tasks.md:15`/`:55`）；D6 的 debug 旧 settings 路径问题（E4）未关闭。 |
| C10 `crossh open` 与单实例 | 部分关闭 | 已关闭 | D5 已关闭：Impact/tasks 覆盖 `crates/crossh-core/src/single_instance.rs`，主窗口与 git/note 载荷分离，混合版本行为可测试（`proposal.md:43`、`design.md:424-426`、`tasks.md:61`/`:73`）。任务 5.6 C 组 `rg` 的错误由 E3 修正，不改变 D5 的设计结论。 |
| C12 与 session-daemon 衔接 | 部分关闭 | 已关闭 | D7/D8 已关闭：`proposal.md:13-14`、`design.md:447-465`、`tasks.md:64`/`:122`、`specs/session-host/spec.md:7-27`；`openspec validate --strict` 的 INFO 已写成归档阻断条件。 |

**C9 复核。** C9 仍为已关闭。`design.md:428` 已把原评审记录中的“前五列顺序不变”明确改写为“前四列顺序与宽度不变，`CWD` 从第五列移到最后”；`tasks.md:62`（6.3）与 `tasks.md:63`（6.4）同步了帮助文本、测试与发布说明。`src/app/cli.rs:103` 的 `help_text` 目前只列命令、不列 `ls` 列序，因此没有待改的旧列名；`tasks.md:62` 仍要求实现时让 `--help` 与实际输出一致。原 C9 状态段里的“前五列”是首轮记录，以本轮 `design.md:428` 为准。ROLE 列保留，`role_of`（`client.rs:489`）的可观察性不回退。

**C11 复核。** C11 的设计层结论仍为已关闭；任务 5.6:52 的 C 组 `rg` 范围错误由 E3 修正，不改变 GUI 迁移清单的关闭结论。

### 新增条件 E1–E7

**E1（C6 / D3）回执承载 `storage_error` 并定义清除后的快照重发。**

必须：在 `WorkspaceMutateReply`（`design.md:222-228`）增加 `storage_error: Option<StorageErrorInfo>`（或等价的 `durable: bool` + `StorageErrorInfo`），并写清语义：`Applied` + `None` = 已落盘；`Applied` + `Some` = 内存已应用、未落盘，客户端 MUST 显示为未落盘；`Unchanged`/`Conflict`/`Rejected` 不改变持久性；`OpError` 继续只用于 `Rejected`/`Conflict`。同步 `design.md:193`/`:252`/`:255`、`spec.md:61`、`tasks.md:18`（2.4）、`tasks.md:24`（3.2）、`tasks.md:36`（4.2）与 D16 的 serde 测试。定义 writer 写成功/失败如何通知 actor、`storage_error` 变化是否在同 revision 重发快照（与 `design.md:235`/`design.md:338` 的运行时字段合并规则一致），避免错误清除后客户端仍显示旧错误。

验证：注入 store 使结构写入失败，断言回执 `Applied` 且 `storage_error` 有值、随后快照携带同一错误；恢复后下一次成功写入触发同 revision 快照且 `storage_error` 清空；serde 往返覆盖有/无 `storage_error`；客户端在 `Applied + storage_error` 时显示“未落盘”。

**E2（C4 / D1）物化“每个 leaf”排除不可用 pane，并把 `OpenProject` 纳入触发。**

必须：把 `spec.md:126`、`design.md:354`、`tasks.md:29`（3.7）的“每个 pane/leaf”改为“活跃 tab 中**可物化**的每个 pane/leaf”，显式引用 `spec.md:277`、`design.md:205`/`:441`、`tasks.md:28`（3.6）的“不可用 MUST NOT spawn”；把 `OpenProject` 加入 `design.md:354` 与 `tasks.md:29` 的物化触发列表，或规定“任何在活跃 tab 中创建 pane 的操作都立即物化该 pane”。同步 `spec.md:128-144` 的场景与 `tasks.md:29` 的测试断言。

验证：活跃 tab 同时含 Live、Dormant、`Unavailable` pane 时，首个 attach 只为可物化者 spawn；项目不可用或 cwd 缺失的 pane 不 spawn；GUI 已 attach 时 `crossh open` 新项目/空项目补 tab 后 pane 不会永久停在 `Dormant`；`OpenProject` 的缺失根分支仍保持 `Unavailable` 且不 spawn。

**E3（D4 / D5 / C10 / C11）修正任务门禁与不可触发的拒绝示例。**

必须：把 `tasks.md:52`（5.6 C 组）改为可执行的 `rg` 范围：允许 `crates/crossh-core/src/single_instance.rs:74`/`:88` 的定义、`src/app/single_instance.rs:7-8` 的 re-export、`src/main.rs:129` 的 `try_forward(None)` 调用，以及 `src/bin/crossh-git.rs:35`/`crossh-note.rs:46` 的 `try_forward_git`/`try_forward_note`；用 `try_forward\(` 或词边界避免裸 `try_forward` 子串误伤 git/note。把 `tasks.md:60`（6.1）的 `not_found`/`Conflict` 示例删除或改写为非法路径、daemon 启动失败/`daemon.error`、版本不兼容、回执超时；若要覆盖 `Conflict`，另写一条直接携带 `expected_revision` 的 `WorkspaceMutate` 测试。

验证：按修正后的 `rg` 命令在实现后只剩允许文件；6.1 的每个拒绝分支都能由真实 CLI 或假 daemon 触发；`Conflict` 的 CAS 测试不依赖 `crossh open`。

**E4（C7 / D6）修正 debug 旧 settings 的迁移路径与备份说明。**

必须：承认旧 debug 构建写的是 `~/.config/crossh/settings.toml`（`persistence.rs:97-102`、`locale.rs:41-42` 无 debug 后缀），新 debug 根是 `~/.config/crossh-dev`（`design.md:269`）；修正 `design.md:279` 的“旧版本升级只发生在各自根下”和 `proposal.md:53` 的 debug 备份路径。给出二选一：debug 在 `crossh-dev/settings.toml` 不存在时回退读取 `crossh/settings.toml` 做一次性迁移；或明确声明 debug 不迁移旧 settings、发布说明要求手动复制，并说明旧文件不会被新版覆盖。迁移快照仍按 `design.md:360` 写到各自根。

验证：旧 debug fixture 放在 `~/.config/crossh/settings.toml`，新 debug 首次启动后要么导入到 `crossh-dev`，要么按文档不导入且旧文件逐字节不变；release 升级路径（`crossh` → `crossh`）不受影响；`CROSSH_CONFIG_DIR` 指向同一根时两侧共享同一快照。

**E5（C1 / C4 / C6 / D4）`daemon status` 识别存活但不兼容的 daemon，并收紧 `daemon.error` 生命周期。**

必须：让 `crossh daemon status` 复用 `ensure_daemon` 的三态，或对 `Client::connect_to` 的握手错误分类；端口可达但版本不兼容时输出 `incompatible: ... crossh daemon stop`（而非 `not running (stale port file)`），并保持非零退出。明确 `daemon.error` 的读取/删除失败处理：成功启动删除失败时只记日志，并在展示前用 workspace 的当前 `format_version`/mtime 校验，避免旧错误掩盖当前 live 状态。同步 `design.md:430-437`、`tasks.md:62`（6.3）、`spec.md:331-351` 与 `main.rs:209-224` 的实现任务。

验证：v1 假 daemon 运行时 `crossh daemon status` 输出兼容性错误与 stop 命令、退出非零；格式过新时输出 `not running: <原因>`；成功启动后陈旧 `daemon.error` 不再出现；端口文件缺失且无错误文件时仍输出原文 `not running` / `not running (stale port file)`。

**E6（C4 / D1）`PaneStatus` 不得遮蔽 live 会话绑定，且与 `SetPaneCwd` 的 live 拒绝一致。**

必须：调整判定表，使 live 会话在项目根缺失或 cwd 缺失时仍能向客户端提供 `session_id`。可选方案：在 `Unavailable` 变体携带 `session_id: Option<SessionId>`；或在 `PaneSnapshot` 增加独立的 `session: Option<SessionId>` 字段；或把判定改为“项目不可用/cwd 缺失时，若 `Runtime.pane_session` 命中，返回 `Live` 并另带 `available=false`/`cwd_missing` 警告”。同步 `design.md:195-205`/`:334`、`spec.md:277`、`tasks.md:11`/`:28`，并让 `SetPaneCwd` 的 `Live` 拒绝（`design.md:185`）与状态判定一致。

验证：live shell 的 cwd 被删除、项目根被删除两种情况下，快照仍提供 session id，GUI 可继续 attach；`SetPaneCwd` 对 live session 仍返回 `pane_live`；目录恢复后状态回到 `Live`/`Dormant` 且不重复 spawn；同一 revision 的 `PaneStatus` 变化按运行时字段合并。

**E7（C6 / D3）统一 `CloseSession` 效果与回执的顺序。**

必须：`design.md:207` 写“解绑 → 更新布局 → 结构持久化 → 投递 `CloseSession` 效果 → 回执”，`design.md:239` 却写“`CloseSession` 效果因此一定晚于回执”；`tasks.md:25`（3.3）与 `design.md:207` 一致。统一为“解绑 → 更新布局 → 结构持久化 → 回执；`CloseSession` 作为异步效果可在回执前投递或回执后投递，但回执 MUST NOT 等待会话结束”，并保留 `spec.md:191` 的“回执快照不含目标 pane、会话结束 MAY 稍后完成、迟到 `SessionExited` MUST 被忽略”。

验证：注入一个延迟退出的会话，断言回执快照不含目标 pane、回执不等待退出、随后 `SessionExited` 不改布局；`CloseTab`/`StopProject`/`ForgetProject` 三条路径行为一致；重复 `SessionExited` 不重复删 pane。

### 复核确认的修复

- workspace 级 `active_pane` 已删除，活跃 pane 改为三级投影；`ActivatePane` 同步 `tab.active_pane`、`project.active_tab`、`workspace.active_project`（`design.md:183`/`:336`、`tasks.md:11`/`:44`）。
- `storage_read_only` 已从四件套删除；格式过新的原因通过 `daemon.error` 与 `daemon status` 暴露（`design.md:257`/`:437`、`spec.md:101-106`、`tasks.md:16`/`:62`/`:72`）。
- `config_root()` 已统一到 `crates/crossh-core/src/config/mod.rs`，`locale.rs`、`workspace_store.rs`、`persistence.rs` 共用；`CROSSH_CONFIG_DIR` 直接是根；settings/workspace/迁移快照路径一致（`design.md:263-279`/`:360`、`tasks.md:15`/`:55`）。
- 单实例真实实现已进入 Impact 与任务，主窗口改无载荷，git/note 的目录语义不变；混合版本行为有测试项（`proposal.md:43`、`design.md:424-426`、`tasks.md:61`/`:73`）。
- D7/D8 的措辞与调用点已统一；“可实施、不可归档”在 proposal/design/tasks 三处一致；分屏折叠与退休路径已进入 D15 表与 task 5.7（`proposal.md:13-14`、`design.md:447`/`:459-463`、`tasks.md:53`/`:64`/`:122`）。
- `crossh ls` 的列序已明确改写：保留 `ID ROLE ALIVE PROGRAM`，层级列使用稳定 id，`CWD` 保持最后一列；status 保留第一行原文并追加工作区行（`design.md:428-437`、`spec.md:372-376`、`tasks.md:62`）。
- `OpenProject` 的三分支与 `ActivateProject` 不建 tab 已写成明确 MUST，迁移来的空项目与显式打开的项目不再混淆（`design.md:173`/`:176`、`spec.md:235-242`、`tasks.md:9`）。
- 同 revision 快照的合并规则在客户端规则、`RefreshAvailability` 与 spec 三处一致：只允许 `available`、`PaneStatus`、`storage_error` 变化，布局字段必须相同（`design.md:235`/`:338`、`spec.md:207`、`tasks.md:44`）；`storage_error` 清除后的重发路径仍缺，见 E1。
- `session-host` delta 的 MODIFIED 场景与 workspace spec 的“pane 身份稳定 / 会话重建”一致：GUI 重启重连运行中的会话，已结束的会话在原 pane 按最后 cwd 重建，判定依据是当前绑定会话 id 而非持久化历史 id（`specs/session-host/spec.md:21-27`、`spec.md:41-55`/`:108-144`）。

### 最终状态总表

> 本节为上一轮的状态记录；收尾复审后的最终状态见文末「收尾复审最终状态总表」。

| 编号 | 状态 | 说明 |
| --- | --- | --- |
| C1 升级与停服路径 | 已关闭 | 停服路径闭环；`daemon status` 的诊断残留由 E5 承接。 |
| C2 回执与 revision | 已关闭 | `design.md:222-241`、`spec.md:207-231`、`tasks.md:24`/`:37`/`:72`。 |
| C3 actor 边界与 Close 路由 | 已关闭 | `design.md:110-163`、`spec.md:148`、`tasks.md:23`/`:25`。 |
| C4 操作 schema 与错误路径 | 部分关闭 | D2 已关闭；D1（E2、E6）与 D4（E3、E5）未关闭。 |
| C5 attach 时物化 | 已关闭 | `spec.md:126-144`、`design.md:346-354`、`tasks.md:29`。 |
| C6 持久化与格式保护 | 部分关闭 | D4 的格式过新部分已关闭；D3（E1）与 live-incompatible status（E5）未关闭。 |
| C7 配置根 | 部分关闭 | D6 的 debug 旧 settings 路径（E4）未关闭。 |
| C8 旧设置迁移 | 已关闭 | `spec.md:302-327`、`design.md:356-372`、`tasks.md:19`/`:40`/`:55`。 |
| C9 `crossh ls` | 已关闭 | 列序按 `design.md:428` 改写；ROLE 保留，CWD 最后；`tasks.md:62`/`:63` 同步。 |
| C10 `crossh open` 与单实例 | 已关闭 | D5 已关闭；C 组 `rg` 由 E3 修正。 |
| C11 GUI 迁移清单 | 已关闭 | 设计层关闭；5.6 C 组 `rg` 由 E3 修正。 |
| C12 与 session-daemon 衔接 | 已关闭 | D7/D8 已关闭；delta 与归档顺序明确。 |
| D1 `OpenProject` 与 pane 状态 | 部分关闭 | 三分支/ActivateProject/旧 Dormant 矛盾已关闭；E2、E6 未关闭。 |
| D2 `active_pane` 两级 | 已关闭 | `design.md:294-299`/`:336`、`spec.md:207`、`tasks.md:11`/`:44`。 |
| D3 结构变更耐久性 | 部分关闭 | fsync/例外已对齐；E1 未关闭。 |
| D4 `storage_read_only` 与可观察性 | 部分关闭 | 错误码删除与格式过新路径已关闭；E3、E5 未关闭。 |
| D5 单实例载荷 | 已关闭 | `proposal.md:43`、`design.md:424-426`、`tasks.md:61`/`:73`。 |
| D6 `config_root()` | 部分关闭 | 解析器/路径/备份已关闭；E4 未关闭。 |
| D7 “可实施、不可归档” | 已关闭 | `proposal.md:13-14`、`design.md:447`、`tasks.md:64`/`:122`。 |
| D8 分屏折叠与退休路径 | 已关闭 | `design.md:459-463`、`tasks.md:53`。 |
| E1 回执 `storage_error` | 未关闭 | 新增条件，见上。 |
| E2 物化排除不可用与 `OpenProject` 触发 | 未关闭 | 新增条件，见上。 |
| E3 任务门禁与拒绝示例 | 未关闭 | 新增条件，见上。 |
| E4 debug 旧 settings 迁移 | 未关闭 | 新增条件，见上。 |
| E5 `daemon status` 兼容性诊断 | 未关闭 | 新增条件，见上。 |
| E6 `PaneStatus` 与 live 绑定 | 未关闭 | 新增条件，见上。 |
| E7 `CloseSession` 与回执顺序 | 未关闭 | 新增条件，见上。 |

### 仍未验证事项

- 未运行任何 `cargo build` / `cargo check` / `cargo clippy` / `cargo test` / `cargo fmt` / `scripts/check-architecture.sh`；`workspace.rs`、`workspace_store.rs`、`workspace_actor.rs`、协议 v2 与 GUI 投影能否编译、是否满足 2000 行限制均未验证。
- 未运行 v2 与 v1 的真实互操作：`daemon_stop` 用 v1 握手时，v2 客户端是否接受 `Welcome.version = 1`、是否忽略 v1 应答的未知/缺失字段，均只有静态推断（`handshake.rs:31-36`/`:93-100`）。
- 未在 macOS 实机验证 `dirs::home_dir().join(".config")`、`0700`/`0600` 权限、detached 进程的环境继承与 `CROSSH_CONFIG_DIR` 传播；Linux/Windows 按仓库约束交给 CI。
- 未执行迁移集成测试、故障注入（rename 前后 kill、写失败退避）、GUI 冒烟与 `crossh ls` 列解析测试。
- 未验证同 revision 快照在真实 `watch` 通道上的到达顺序，以及 `RefreshAvailability`/`storage_error` 更新与并发 mutation 交错时客户端是否出现一次错误焦点或错误提示。
- 未验证 `session-host` delta 的实际合入（当前 `openspec validate --strict` 仍报 `target spec does not exist`）；只能确认 delta 文本与 workspace spec 的 R3/R6/R8 语义一致。
- 未验证 `session-daemon` 归档后主规格是否真的移除了“固定标签重新接入同一个会话”；这依赖该变更归档时的操作。

### 最终判定

**带条件通过。** 第二轮修订解决了 D2、D5、D7、D8，并把 D1、D3、D4、D6 收窄到可定位的条文/任务缺口；没有推翻“daemon 持有布局、GUI 投影”的架构判断，也没有发现无法通过条文修订解决的根冲突。E1–E7 是 apply 前必须落到四件套的修订项：E1–E7 关闭并在本文件回填之前不得开始代码实现；关闭后 C4、C6、C7 才能转为已关闭。归档仍受 `session-daemon` 归档、`session-host` delta 合入、CI 回填与 7.1–7.3 结论的约束。

## 收尾复审（2026-11-17）

### 复审边界与方法

本轮只做只读核对，未运行任何 Rust 编译、检查或测试命令：未执行 `cargo build/run/check/clippy/test/fmt`、`scripts/check-architecture.sh`、发布脚本，也未写入 `target/` 或下载依赖。允许范围内执行了 `openspec validate daemon-workspace --strict`（退出 0，仅一条已知 INFO：`session-host/spec.md: Archive would refuse this delta: session-host: target spec does not exist; only ADDED requirements are allowed for new specs.`）与 `git status --short`（仅 `?? openspec/changes/daemon-workspace/`）。

核对范围：`proposal.md`、`specs/workspace-state/spec.md`、`specs/session-host/spec.md`、`design.md`、`tasks.md` 与 `openspec/config.yaml`；源码事实用 `rg`/只读读取核对 `crates/crossh-core/src/{config,locale,single_instance}.rs`、`crates/crossh-session/src/{protocol,client,serve,handshake}.rs`、`src/app/single_instance.rs`、`src/main.rs:209-224`、`src/features/settings/persistence.rs:97-102`、`src/features/workspace/shell/{split,quit}.rs`、`src/infrastructure/tray.rs`。下文路径除特别注明外均相对 `openspec/changes/daemon-workspace/`。

### E1–E7 逐项结论

| 条件 | 结论 | 主要证据 |
| --- | --- | --- |
| E1 回执 `storage_error` 与清除重发 | 已关闭 | `design.md:228-244`、`:246`、`:250-252`、`:269`、`:511`；`spec.md:87`、`:89-107`；`tasks.md:18`/`:24`/`:36`/`:77` |
| E2 物化排除不可用与 `OpenProject` 触发 | 已关闭 | `spec.md:141`、`:149-177`；`design.md:176`、`:382-384`；`tasks.md:29`/`:31` |
| E3 任务门禁与拒绝示例 | 部分关闭 | `tasks.md:49-57`、`:65`、`:77`；剩余 E3-1（`src/infrastructure/tray.rs:199`） |
| E4 debug 旧 settings 迁移 | 部分关闭 | `design.md:288-300`、`:390-398`；`spec.md:379-387`；`tasks.md:15`/`:19`/`:59`；剩余 E4-1（release 覆盖语义） |
| E5 `daemon status` 兼容性诊断 | 已关闭 | `design.md:436-442`、`:467-475`；`spec.md:395-429`；`tasks.md:67`/`:77`/`:78`；新增 N1 |
| E6 `PaneStatus` 与 live 绑定 | 已关闭 | `spec.md:309-346`；`design.md:204-207`、`:319-366`、`:478`；`tasks.md:11`/`:28`/`:44`/`:77` |
| E7 `CloseSession` 与回执顺序 | 已关闭 | `design.md:150`、`:209`、`:256`；`spec.md:190-197`、`:226-228`；`tasks.md:25` |

#### E1：已关闭

`WorkspaceMutateReply` 现在有 `durable: bool` 与 `storage_error: Option<StorageErrorInfo>`（`design.md:230-231`），四种组合逐项定义在 `design.md:235-244`：结构写入成功 `Applied/true/None`；结构写入失败 `Applied/false/Some`；元数据入队 `Applied/false/None`；`Unchanged`/`Conflict`/`Rejected` 为对应结果加 `false/None`，且 MUST NOT 改变持久性。`OpError` 继续只服务 `Rejected`/`Conflict`（`design.md:196`、`:244`）。这套语义同时覆盖“结构变更已应用但未落盘”“元数据允许崩溃丢失”“无操作不改变持久性”三类可观察状态，`spec.md:87` 的 R5 MUST 与 `:89-121` 的场景逐条对应，`tasks.md:18`（2.4）、`:24`（3.2）、`:36`（4.2）、`:77`（7.5）与 `design.md:511` 的 serde/故障注入测试同步。`PersistenceResult` 回路闭合：writer 每次尝试后向 actor 报 `Ok` 或 `Err`，actor 记录或清除 `storage_error` 并重发同 revision 快照，MUST NOT 推进 revision（`design.md:250`）；写失败时的结构回执携带同一个错误（`design.md:269`）；恢复后成功写入触发同 revision 快照清除错误（`tasks.md:18`/`:24`/`:77`）。同 revision 重发只合并运行时字段的规则在 `design.md:252`、`spec.md:241` 与 `tasks.md:24` 一致。

**剩余建议（不构成条件）**：`design.md:244` 把 `Applied + durable=false + storage_error=None` 描述为“只表示已入队、允许崩溃丢失最近 500 ms 元数据”，并把 `Applied + storage_error=Some` 称为“唯一的已应用但未落盘组合”。当 store 正处于失败状态时，元数据回执同样落在 `Applied/false/None`，它也没有落盘，客户端需要结合快照的 `storage_error` 判断，而不能仅凭该组合断言“已落盘”。建议把“唯一”改为“结构变更的唯一错误组合”，并补一句客户端展示规则。

#### E2：已关闭

`spec.md:141` 的 R6 把“可物化”定义为“无会话、所属项目可用、pane cwd 存在”，并明确 `Live` 不重复 spawn、`Starting` 不重复投递、项目不可用或 cwd 缺失 MUST NOT 创建会话；首个 attach 物化活跃 tab 的可物化 pane，非活跃 tab 在激活时按同一规则物化。`design.md:382` 的 D10 与 `tasks.md:29`（3.7）/`:31`（3.9）逐条复述，测试断言覆盖活跃 tab 同时含 Live/Dormant/Unavailable 时只 spawn Dormant。`OpenProject` 已列入物化触发：`design.md:176` 明确“有 attach 客户端时物化新建或新激活的可物化 pane”，`design.md:382-384` 与 `tasks.md:29` 同步。`attached_clients > 0` 门控覆盖 `OpenProject`/`CreateTab`/`SplitPane`/`EnsurePaneSession`；计数为 0 时只改布局、pane 保持 `Dormant`，`crossh open` 只发 `WorkspaceMutate` 不 attach，因此 CLI 打开项目不拉 shell（`design.md:384`、`tasks.md:29`）。缺失根分支只建 `Unavailable` pane、不 spawn（`design.md:176`/`:382`、`spec.md:278-283`、`tasks.md:29`）。

**剩余建议（不构成条件）**：`ActivateTab`/`ActivateProject` 在 `design.md:382` 中按“同一规则”物化，但 `design.md:384` 的 `attached_clients > 0` 门控清单只列了 `OpenProject`/`CreateTab`/`SplitPane`/`EnsurePaneSession`；`spec.md:141` 对“非活跃 tab 被激活”也没有附加 attach 计数条件。产品 CLI 不暴露这两个操作，GUI 激活时必然已 attach，因此当前路径没有用户可见问题；R18 允许任意前端，建议把这两个操作也写入“仅在有 attach 客户端时立即物化”，或明确它们要求调用方已 attach。

#### E3：部分关闭

已落地的部分：`tasks.md:49-57` 的 5.6 C 组允许清单覆盖真实定义（`crates/crossh-core/src/single_instance.rs:74`/`:88`、`try_forward_to`/`write_request`/`read_and_validate`）、re-export（`src/app/single_instance.rs:7-8`）、调用（`src/main.rs:119`/`:129`）、`src/app/cli.rs:47` 的注释与 git/note 的 `try_forward_git`/`try_forward_note`；检查命令使用 `\btry_forward\b`/`try_forward\(` 词边界，不会误伤 `try_forward_git`、`try_forward_note` 或 `try_forward_to`。6.1 的拒绝示例已改为非法/不存在路径、daemon 启动失败或 `daemon.error`、版本不兼容、回执超时四类可由真实 CLI 或假 daemon 触发的分支，并明确 `OpenProject` 无目标对象且 `expected_revision: None`，因此不在此验证 `not_found`/`Conflict`（`tasks.md:65`）；`Conflict` 的 CAS 测试已移到 7.5，用直接携带 `expected_revision` 的 `WorkspaceMutate`（`tasks.md:77`）。

未关闭的 E3-1：`tasks.md:57` 的精确命令 `rg -n '\btry_forward\b|\btry_forward\(|\bresolve_open_path\b' src crates` 在当前代码上仍命中 `src/infrastructure/tray.rs:199` 的注释（该注释提到 `try_forward` 会把新进程变成给运行实例开窗后退出），该行不在 C 组允许清单中；`tasks.md:52`（5.6 C 组）与 `:76`（7.4）都规定“其余命中即失败”，按字面执行会失败。核对其余命中均在允许范围：`src/main.rs:119`/`:129`、`src/app/single_instance.rs:7-8`、`src/app/cli.rs:47`、`crates/crossh-core/src/single_instance.rs:74`/`:88` 及其单元测试（`:284`/`:288`/`:302-304`）。修复方式是补一条“注释提及”允许项，或把命令限定为代码行并显式排除注释；同时更新 7.4 的同一判据。

#### E4：部分关闭

已落地的部分：`settings_read_path()` 在 `settings_path()` 不存在、未设置 `CROSSH_CONFIG_DIR` 且旧路径存在时只读回退；保存始终写 `settings_path()`（`design.md:288-294`、`tasks.md:15`（2.1）/`:19`（2.5）/`:59`（5.9））；`CROSSH_CONFIG_DIR` 的值直接作为配置根，设置时不回退（`design.md:294`、`spec.md:379`、`tasks.md:15`/`:19`）；release 的 `settings_write` 保持 `~/.config/crossh/settings.toml`，debug 使用 `~/.config/crossh-dev/settings.toml`（`design.md:300`/`:398`、`proposal.md:53`）；debug 旧文件迁移、`<源文件>.pre-workspace` 只读快照、`legacy_imported` 标记、失败重试与“迁移完成前不保存”均已写清（`design.md:390-398`、`spec.md:350`/`:379-387`、`tasks.md:19`、`proposal.md:53`）。

未关闭的 E4-1：`spec.md:379` 的 R15 写“同一旧文件 MUST 可被两者各迁移一次，且旧文件 MUST 保持逐字节不变”，`:386` 的 debug 场景重复“旧文件逐字节不变”；`design.md:294` 写“旧文件不会被新版覆盖”，`:300` 写“旧文件本身不被覆盖”；`tasks.md:19` 写“旧文件只读、不被新版写入”。这些无条件表述对 release 不成立：release 的迁移源与 `settings_write` 都是 `~/.config/crossh/settings.toml`，迁移在 daemon 开始服务前完成，随后 GUI 首次保存会重写该文件并丢掉旧字段；旧值只保留在 `~/.config/crossh/settings.toml.pre-workspace`（`design.md:398`、`proposal.md:53`）。`design.md:513` 的测试项也只在 debug 回退路径上断言“旧文件逐字节不变”，说明该不变量实际只适用于 debug；spec/design D8/tasks 2.5 的通用表述需要按此收窄。`spec.md:350` 的“迁移完成前 MUST NOT 被新版保存覆盖”是正确的，应保留；需要限定的是迁移完成后的“逐字节不变/不被覆盖”语句。建议把 R15、design D8 与 tasks 2.5 的表述改成“debug 回退读取的旧文件保持逐字节不变且不被新版写入；release 的旧文件与新的 settings 落点同路径，首次保存后旧字段不再出现，只能从 `.pre-workspace` 快照恢复”，并在发布说明中同步“逐字节不变只适用于 debug 回退文件”。

#### E5：已关闭

`design.md:436-442` 定义 `ensure_daemon` 的 `Absent`/`Ready`/`Incompatible` 三态，`Incompatible` 不删端口、不 spawn；`daemon_stop` 不再 `ensure_daemon`，失败时用同一 token 以 v1 握手重试，任何分支不拉起新实例。`design.md:467-475` 的 status 三态表覆盖“端口缺失/不可连接 → not running（有 `daemon.error` 时 `not running: <原因>`）”“端口可达但版本不兼容 → `incompatible: found vN, expected vM; run crossh daemon stop` 且非零退出”“端口可达且握手成功 → 两行 running”。`daemon.error` 只在“没有可达且兼容的常驻进程”时展示，握手成功后一律忽略陈旧的错误文件（`design.md:274`/`:475`、`spec.md:87`/`:395`）。spec 的 R16（`spec.md:395-429`）的场景覆盖：新客户端连旧 daemon、停止旧 daemon、status 识别旧 daemon、陈旧错误文件不掩盖运行状态、缺少工作区能力；`tasks.md:67`（6.3）、`:77`（7.5）、`:78`（7.6）覆盖四种验证。现状 `src/main.rs:209-224` 把 `connect_to` 的任何错误都打印成 `not running (stale port file)`，正是本轮要修的缺陷；`handshake.rs:85-101` 的版本检查先于令牌检查与设计的 v1 回退前提一致。

新增 N1（见下）：`design.md:467` 与 `tasks.md:67` 写“status 复用 `ensure_daemon` 的三态”，而 `ensure_daemon` 的 `Absent` 分支会删除端口文件并 spawn（`design.md:438`、`client.rs:162-169`）。status 必须复用三态分类，但实现必须走无副作用 probe；这条实施约束单独列为 N1，E5 的 spec 可观察行为本身已闭合。

#### E6：已关闭

`spec.md:309-346` 的 R13 与场景覆盖：快照同时暴露 `session`、`cwd_available`、`PaneStatus` 与 `storage_error`；判定顺序为先看会话绑定，已绑定即 `Live` 并给 `session`，项目根或 cwd 缺失也不降级；无会话时才依次 `Starting`、可物化 `Dormant`、`Unavailable`；`cwd_available` 与项目 `available` 独立；无会话且不可用 MUST NOT spawn；已有会话 MUST NOT 因目录缺失被结束。`design.md:204-207` 的判定表与优先级、`:319-362` 的 `PaneSnapshot`/`PaneStatus` 结构、`:366` 的同 revision 不变量、`:478` 的 D14 与之一致；`SetPaneCwd` 对 live pane 返回 `pane_live`（`design.md:188`），`EnsurePaneSession` 对 Live/Starting/Unavailable 返回 `Unchanged` 且不尝试 spawn（`design.md:189`）。`tasks.md:11`（1.5）、`:28`（3.6）、`:44`（5.1）、`:77`（7.5）覆盖“有会话 → Live 且 session=Some，即使根/cwd 缺失”和 `cwd_available` 独立变化。`PaneStatus` 的 `reason` 字段记录最近一次未物化/失败原因，不参与判定（`design.md:207`），与 `session`/`cwd_available` 没有遮蔽或矛盾。

**剩余建议（不构成条件）**：`Dormant { reason: String }`/`Unavailable { reason: String }`（`design.md:357-358`）的类型未定义取值，测试和 GUI 只能做字符串匹配；建议改成枚举或固定错误码（如 `project_root_missing`、`cwd_missing`、`spawn_failed`），与 `OpError` 的稳定 code 风格一致。

#### E7：已关闭

三条关闭路径已统一为“解绑 → 更新布局 → 结构持久化（fsync）→ 投递 `CloseSession` 效果 → 回执”：`design.md:150` 的 `Close` 路由、`:209` 的 `StopProject`/`ForgetProject`/`CloseTab`、`:256` 的重复说明一致；`tasks.md:25`（3.3）逐条落地并覆盖“回执 MUST NOT 等待会话结束、回执快照不含目标 pane、迟到的 `SessionExited` 不改布局、不重复删 pane”。`spec.md:190-197` 的关闭分屏 pane 场景与 `:226-228` 的 R10 给出同样的可观察行为；`design.md:151-153` 的 `SessionExited` 幂等与 `shutting_down` 忽略规则保证迟到/重复退出一致。现状 `quit.rs:41-49` 关闭窗口只 detach、不结束会话，`serve.rs:319-333` 的 `Close`/`Shutdown` 是旧实现前提，与本设计不冲突。

### 主管 7 点答复

1. **`durable: bool` + `storage_error` 是否足够**：足够。四种组合覆盖结构成功、结构写失败、元数据入队、无操作/冲突/拒绝；枚举不会增加表达力，反而扩大协议。需要修正的是 `design.md:244` 对“唯一未落盘组合”的措辞，让客户端在 store 失败期间结合快照 `storage_error` 判断元数据回执。
2. **`Unchanged` 的 `durable=false` 是否会被误解**：规格与设计已写明它只表示“不改变持久性”（`spec.md:87`、`design.md:244`），语义自洽。建议在客户端展示规则里补一句“`durable` 只在 `Applied` 时有展示意义；`Unchanged`/`Conflict`/`Rejected` 的 `false` 不显示为未落盘”，并修正 `design.md:230` 的字段注释“本次修改是否已落盘”为“本次 Applied 是否已落盘”。
3. **`EnsurePaneSession` 在 0 个 attach 客户端时保持 `Dormant`**：与 spec/CLI 语义一致。`spec.md:141` 把“显式确保会话”列入 `attached_clients > 0` 才立即物化的集合，`design.md:189`/`:384` 明确计数为 0 时只改布局、保持 Dormant；CLI 不暴露该操作，`crossh open` 也不 attach。需要补的是该操作的 reply outcome/revision（N2）。
4. **release 首次保存会重写旧 settings，逐字节不变只对 debug 成立**：`proposal.md:53` 与 `design.md:398` 已按此写明；`spec.md:379`、`design.md:294`/`:300`、`tasks.md:19` 仍把“旧文件不被覆盖/逐字节不变”写成对 release 也成立，必须按 E4-1 限定。发布说明应明确“逐字节不变只适用于 debug 回退读取的 `~/.config/crossh/settings.toml`；release 的同一路径会在首次保存时重写，旧值只能从 `.pre-workspace` 恢复”。
5. **`PaneStatus` 的 `reason` 是否与 `session`/`cwd_available` 冗余或矛盾**：不矛盾。`session` 是判定 `Live` 的权威字段，`cwd_available` 是独立警告，`reason` 只记录最近一次未物化/失败原因（`design.md:207`），三者可以同时出现；`Unavailable{cwd_missing}` 与 `cwd_available=false` 是同一事实的不同视图。建议把 `reason` 的类型固定为枚举/错误码，避免不可断言的自由字符串。
6. **新增两个 Requirement 的拆分是否导致 MUST 归属不清**：R5（持久化失败/格式过新可观察性）与 R15（设置路径只读回退）的 MUST 归属基本清楚，场景也能定位。R5 与 R16 都写了 `daemon.error` 的展示前提（`spec.md:87`、`:395`），建议由 R16 作为协议/status 的唯一归属，R5 只引用；R15 的“逐字节不变”越界到 release，是 E4-1 的实质问题，不是 Requirement 拆分本身造成的。
7. **`daemon status` 复用三态是否足够，是否还需独立握手错误枚举**：三态足够定义输出，不需要独立的握手错误枚举；`incompatible` 分支只需携带两侧版本与 `crossh daemon stop`。必须补的是实现方式：status 使用无副作用的 probe，不能调用会删除端口并 spawn 的 `ensure_daemon`（N1）。

### 新增与剩余条件

| 编号 | 所属 | 必须修复 | 验证 |
| --- | --- | --- | --- |
| E3-1 | E3 / C10 / C11 | C 组允许清单补 `src/infrastructure/tray.rs:199` 的注释提及，或把检查命令限定为代码行并排除注释；同步 5.6 与 7.4 的“其余命中即失败”。 | 重跑 `rg -n '\btry_forward\b|\btry_forward\(|\bresolve_open_path\b' src crates`，所有命中均在允许清单内。 |
| E4-1 | E4 / C7 / D6 | 把 `spec.md:379`/`:386`、`design.md:294`/`:300`、`tasks.md:19` 的“旧文件不被覆盖/逐字节不变”限定到 debug 回退文件；明确 release 的 `settings_write` 与迁移源同路径，首次保存会重写并丢旧字段，旧值只在 `.pre-workspace`；发布说明同步。 | release fixture 首次保存后旧字段消失、快照完整；debug fixture 旧文件逐字节不变；两侧迁移标记独立。 |
| N1 | C6 / E5 衍生 | `design.md:467` 与 `tasks.md:67` 改为“复用三态分类，通过无副作用只读 probe 实现”；status MUST NOT 删除 `daemon.port`、MUST NOT spawn；同步 6.3/7.6。 | 无端口文件、陈旧端口、v1 daemon、v2 daemon 四种情况；前两种断言没有新进程/端口文件，后两种断言 `incompatible:` 与两行 running。 |
| N2 | C4 | 定义 `EnsurePaneSession` 与 `RefreshAvailability` 的 reply outcome/revision：`EnsurePaneSession` Live/Starting/Unavailable/0 attach → `Unchanged`；dormant + attach > 0 的物化按 runtime-only 例外返回 `Applied` 且 revision 不变（或给出等价明确规则）。`RefreshAvailability` 无运行时变化 → `Unchanged`；有变化 → `Applied` 且 revision 不变（或等价规则）。`design.md:190` 只定义了“无状态变化时 Unchanged”，`:246` 又把 `RefreshAvailability` 列为 revision 不变时重发快照的例外；把 runtime-only 例外补进 `design.md:246`、`spec.md:241`，并补场景/测试。 | 两个 op 在 live/dormant/0 attach/可用性变化四种分支下断言 outcome、revision 与快照；同一 revision 快照只合并运行时字段。 |

E3-1 和 E4-1 是第三轮修订遗留的条文/任务缺口；N1 和 N2 是第三轮修订引入的新缺口，均在 apply 前必须关闭并在本文件回填。

### C4 / C6 / C7 状态更新

| 条件 | 上一轮 | 收尾复审 | 理由 |
| --- | --- | --- | --- |
| C4 操作 schema 与错误路径 | 部分关闭 | 部分关闭 | 原 C4 的 D1/D2/D4 已由 E2/E6、E5 与 E3 的 6.1 闭合；新增 N2（runtime-only op outcome/revision）仍缺可执行语义。E3-1 的 C 组门禁归 C10/C11，不重复计入 C4。 |
| C6 持久化与格式保护 | 部分关闭 | 部分关闭 | 原 E1/E5 的契约已闭合（回执字段、PersistenceResult 回路、status 三态、format 过新与 `daemon.error`）；新增 N1（status 不得调用有副作用的 `ensure_daemon`）仍需修正。 |
| C7 配置根 | 部分关闭 | 部分关闭 | E4-1 未关闭：release 的旧 settings 覆盖语义与 spec/design/tasks 的“逐字节不变/不被覆盖”表述冲突。 |

### 收尾复审最终状态总表

| 项 | 状态 | 说明 |
| --- | --- | --- |
| C1 升级与停服路径 | 已关闭 | 三态、`daemon_stop` v1 回退、任何分支不 spawn 已定义。 |
| C2 回执与 revision | 已关闭 | `request_id`/`outcome`/`revision`/`created`、CAS 与同 revision 规则已定义。 |
| C3 actor 边界与 Close 路由 | 已关闭 | 命令集、效果、锁顺序、`SessionExited` 幂等已定义。 |
| C4 操作 schema 与错误路径 | 部分关闭 | 原 D1/D2/D4 已闭合；新增 N2。 |
| C5 attach 时物化 | 已关闭 | R6/D10/3.7 统一为“可物化”与 attach 门控。 |
| C6 持久化与格式保护 | 部分关闭 | 原 E1/E5 已闭合；新增 N1。 |
| C7 配置根 | 部分关闭 | E4-1。 |
| C8 旧设置迁移 | 已关闭 | 迁移源、快照、标记、失败重试、`ImportLegacy` 删除已定义。 |
| C9 `crossh ls` | 已关闭 | 列序与稳定 id/占位符已定义。 |
| C10 `crossh open` 与单实例 | 部分关闭 | E3-1 的 C 组 `rg` 门禁仍在。 |
| C11 GUI 迁移清单 | 部分关闭 | E3-1 的 C 组 `rg` 门禁仍在。 |
| C12 与 session-daemon 衔接 | 已关闭 | 变更衔接条文已闭合；归档仍受 `session-daemon` 归档与 `session-host` delta 合入约束。 |
| D1 `OpenProject` 与 pane 状态 | 已关闭 | E2/E6 关闭。 |
| D2 `active_pane` 两级 | 已关闭 | 三级活跃字段与投影规则已定义。 |
| D3 结构变更耐久性 | 已关闭 | E1 关闭。 |
| D4 `storage_read_only` 与可观察性 | 已关闭 | 错误码删除、格式过新、status 与 `daemon.error` 已闭合；E3-1 属任务门禁。 |
| D5 单实例载荷 | 已关闭 | 真实实现位置与空载荷语义已定义。 |
| D6 `config_root()` | 部分关闭 | E4-1。 |
| D7 “可实施、不可归档” | 已关闭 | 前置、实施与归档措辞已拆分。 |
| D8 分屏折叠与退休路径 | 已关闭 | 六处调用点与三条退休路径已列出。 |
| E1 回执 `storage_error` | 已关闭 | 见上。 |
| E2 物化排除不可用与 `OpenProject` 触发 | 已关闭 | 见上。 |
| E3 任务门禁与拒绝示例 | 部分关闭 | E3-1。 |
| E4 debug 旧 settings 迁移 | 部分关闭 | E4-1。 |
| E5 `daemon status` 兼容性诊断 | 已关闭 | 见上；N1 为新增实施条件。 |
| E6 `PaneStatus` 与 live 绑定 | 已关闭 | 见上。 |
| E7 `CloseSession` 与回执顺序 | 已关闭 | 见上。 |
| N1 status 无副作用 probe | 未关闭 | 见「新增与剩余条件」。 |
| N2 runtime-only op outcome/revision | 未关闭 | 见「新增与剩余条件」。 |

### 仍未验证事项

- 未运行 `cargo build`/`check`/`clippy`/`test`/`fmt`、`scripts/check-architecture.sh` 或发布脚本；协议 v2、`workspace.rs`/`workspace_store.rs`/`workspace_actor.rs`、GUI 投影能否编译与是否满足 2000 行限制均未验证。
- 未做 v1/v2 真实互操作：`daemon_stop` 的 v1 握手回退、v1 `Shutdown = 16` 的空载荷、v2 客户端对 v1 应答的未知/缺失字段处理，均只有静态推断（`handshake.rs:85-101`、`protocol.rs:17-49`、`client.rs:456-482`）。
- 未在 macOS 实机验证 `dirs::home_dir()`、`0700`/`0600`、detached 环境继承与 `CROSSH_CONFIG_DIR` 传播；Linux/Windows 按仓库约束交给 CI，本地未跑。
- 未执行迁移集成测试、故障注入（rename 前后 kill、写失败退避、debounce 窗口内崩溃）、GUI 冒烟与 `crossh ls` 列解析测试；`crossh daemon status` 的三态输出只做了条文核对，未实际运行。
- 未验证同 revision 快照在真实 `watch` 通道上的到达顺序，以及 `RefreshAvailability`/`PersistenceResult` 与并发 mutation 交错时的客户端合并；N2 的 outcome/revision 规则尚未落到 spec/tasks。
- `session-host` delta 仍未合入主规格；`openspec validate --strict` 的 INFO 已确认归档会被拒绝。`session-daemon` 的归档状态未改变，`daemon-workspace` 归档仍受其前置约束。
- E3-1 的 `rg` 结果是针对当前源码实际运行的；实现后的 5.6/7.4 门禁尚未执行，不能替代实现说明中的回填。

### 收尾判定（历史，已由下节取代）

> 本节的 `带条件通过` 结论和上方“收尾复审最终状态总表”是第三轮复审时的记录。E3-1、E4-1、N1、N2 已在第四轮定点修订中关闭，最终结论见文末「收尾确认（2026-11-17）」。

**带条件通过。** 第三轮修订关闭了 E1、E2、E5、E6、E7；E3、E4 各剩一项条文/任务缺口（E3-1、E4-1）；同时发现两项新的实现前缺口（N1、N2）。这四项都不涉及“daemon 持有布局、GUI 投影”的架构选择，属于可定位的条文修补，因此不判 `不通过`；它们会直接导致 apply 返工或 MUST 无法测试，因此也不判 `通过`。E3-1、E4-1、N1、N2 关闭并在本文件标记之前，不得开始实现；归档仍受 `session-daemon` 归档、`session-host` delta 合入、CI 回填与 7.1–7.3 结论的约束。

## 收尾确认（2026-11-17）

**结论：通过。** 第四轮定点修订关闭了 N1、N2、E3-1、E4-1；至此 C1–C12、D1–D8、E1–E7 与 N1–N2 全部关闭，无剩余 must-fix 条件。本节取代上文所有轮次的状态表与判定。

### 关闭依据

| 项 | 结论 | 证据 |
| --- | --- | --- |
| N1 `daemon status` 无副作用 probe | 已关闭 | `design.md:467` 改为复用三态分类、通过无副作用 `probe_daemon()` 实现，`:449-484` 定义其不删除端口文件、不 spawn、不调用 `ensure_daemon`；`spec.md:428`/`:446-447` 与 `tasks.md:67` 同步；6.3/7.6 的测试覆盖无端口、陈旧端口、v1 daemon、v2 daemon 四种情况。 |
| N2 runtime-only op 的 outcome/revision | 已关闭 | `design.md:189-190` 定义 `EnsurePaneSession`/`RefreshAvailability` 的 `Applied`/`Unchanged` 分支，`:246` 把运行时变化列入“revision 不变但重发快照”的例外，`:519-520`/`:532` 补测试；`spec.md:241` 与 `tasks.md:26`/`:28`/`:29`/`:44` 同步。 |
| E3-1 C 组 `rg` 门禁 | 已关闭 | `tasks.md:57` 与 7.4（`:76`）改为 `rg -n 'try_forward\(|resolve_open_path' src crates`，不再命中 `src/infrastructure/tray.rs:199` 的注释提及；允许清单与“其余命中即失败”的判据保持一致。 |
| E4-1 debug/release 旧 settings 语义 | 已关闭 | `spec.md:374`/`:403` 与 `:409-418`、`design.md:294-300`/`:397-405`、`tasks.md:15`/`:19`/`:60` 明确区分：debug 回退读取的旧文件逐字节不变且不被新版写入；release 的迁移源与 `settings_write` 同路径，首次保存会重写并丢旧字段，旧值只能从 `.pre-workspace` 快照恢复；发布说明同步该限定。 |

### 历史条件的最终状态

| 条件 | 状态 | 说明 |
| --- | --- | --- |
| C1–C12 | 已关闭 | C4/C6/C7 依赖的 N2/N1/E4-1 已关闭；C10/C11 依赖的 E3-1 已关闭；其余历史轮次已确认。 |
| D1–D8 | 已关闭 | D1（E2/E6）、D3（E1）、D4（E5）、D6（E4-1）均在本轮或前轮关闭。 |
| E1–E7 | 已关闭 | E3-1、E4-1 为本轮新增缺口，已关闭；其余在第三轮关闭。 |
| N1、N2 | 已关闭 | 见上表。 |

### 未验证事项（保持原样，交给 apply 与 CI）

- 本地未运行任何编译、check、clippy、test、fmt 或架构脚本（磁盘空间约束）；实现后的编译、2000 行限制、协议 v2 互操作与 GUI 冒烟均由任务 7.1–7.6 和 CI 负责。
- `session-host` delta 仍未合入主规格；`openspec validate --strict` 的 INFO 表示归档会被拒绝，这是已记录的前置约束（`session-daemon` 归档后自动解除）。
- 本文件只评审规划产物；实现代码尚未开始，`review.md` 的通过不构成对实现的验证。

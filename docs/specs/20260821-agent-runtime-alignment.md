# Agent Runtime 对齐 pi-agent（不含插件）

> 对齐 pi 的核心架构与能力：树形 Session、Runtime 隔离、EventBus/消息队列、阈值/溢出双阶段 Compaction、多模式复用。插件/主题/扩展机制不在本 spec。

## 元数据

- 状态：`approved` → `in-progress`
- 创建：2026-08-21
- 批准：2026-08-21（用户在对话中批准）
- 相关 ADR：`0002-logic-ui-layering`、`0003-agent-logic-and-view-split`、`0006-executable-testing-contracts`、`0012-spec-driven-development-loop`，新增 ADR 0015 待定（Session 树与 Runtime 边界）
- 相关 issue / 路线图项：本轮对话「让 crossh agent 拥有和 pi-agent 相同的能力（不含插件）和架构」
- CI 平台影响：`无（纯逻辑）` — Rust 逻辑与 ratatui TUI，仅 macOS 本地验证

## 背景

`crossh-agent` 当前以 `crossh-ai-sdk` 为 provider-neutral 适配层已正确，但上层仍是单体：`AgentSession` 为线性 `Vec<Message>` + 截断式 `compact(max_chars)` (`crates/crossh-agent/src/session.rs:866行`)，`src/agent_cli.rs:1713行` 单文件持有 `App {settings, session, messages, scroll, queued_inputs}` 并直接耦合 ratatui 渲染与模型调用。对比 `pi-agent` 的 `SessionManager(树形JSONL) + AgentSessionRuntime(cwd绑定Services) + Agent(EventBus) + compaction(threshold/overflow摘要)`，crossh 缺少分支/回溯、多模式复用（print/json/rpc/SDK）、以及可嵌入的 runtime 抽象。`agent_cli.rs` 已接近 2000 行红线，后续能力无法以小步叠加。

## 目标

1. Session 持久化从线性升级为树形 `parentId`，支持原地分支与 `fork/clone/import` 语义，历史不丢。
2. 引入 `SessionManager` trait + `FsSessionManager` 实现，`AgentSession` 通过它持久化，支持 `inMemory` 供 SDK/测试。
3. 引入 `AgentSessionRuntime` 拥有 `session + services(cwd, settings, resourceLoader, modelRuntime)`，`switch/new/fork` 统一走 `teardown→create→apply`。
4. 引入 `AgentEventBus` 与统一 `AgentSessionEvent`，实现 `steering(Enter)/followUp(Alt+Enter)` 双队列与 `Esc` 恢复。
5. Compaction 从截断升级为 `threshold/overflow` 双阶段摘要，回写 `CompactionEntry{summary, tokensBefore, firstKeptEntryId}`，`buildContext` 时注入而非插入 System 消息。
6. `src/agent_cli.rs` 按 0003 拆为 `app.rs + view.rs + runtime.rs`（保留 `input.rs/render.rs`），每个文件 < 800 行，logic 不依赖 gpui/ratatui。
7. 复用同一 `Agent` 内核提供 `interactive + print(-p) + --mode json + SDK` 四种调用方式（`--mode rpc` 可后置）。

## 非目标

- 不引入 pi 的 Extension/Skill/PromptTemplate/Theme/PackageManager 插件体系；`load_skills/load_prompts` 保留现状，不新增发现/热重载。
- 不引入 `pi` 的远程 model catalog 自动刷新、subscription 登录；仅保留本地 `settings.toml` 的 provider/model。
- 不改变现有 8 个内置工具的审批策略与 3 协议 wire 格式；`SDK ToolDefinition.requires_approval` 仍归 agent 层（已在 20260818 spec 收敛）。
- 不做 GPUI 工作区集成；`crossh-agent` 仍为独立 `ratatui` 二进制，通过 `sibling_executable` 被 `crossh` 拉起。
- 不处理 `crossh-ssh`/`crossh-terminal` 等非 agent 域。

## 行为契约

命名前缀：`spec_20260821_agent_runtime__`

1. 当 `SessionManager` 以 `parentId` 树形追加 `message/thinking_level_change/model_change/compaction/branch_summary/session_info` 条目时，应能通过 `SessionManager::entries(sessionId)` 按树还原分支，观察到 `fork(entryId)` 后新分支仅共享祖先条目且原分支不受影响。
2. 当 `load_session` 读取旧版线性 JSONL（`kind: session + kind: message`）时，应自动迁移为树形且与旧文件内容语义等价，观察到既有会话可原样读取、新写入为树形记录。
3. 当 `AgentSessionRuntime::switchSession(path)` 调用时，应先触发 `session_before_switch` 式的 `teardownCurrent` 再 `createRuntime(cwd)` 并 `apply`，失败时不污染当前 runtime，观察到切换失败后原 session 仍可用。
4. 当 `AgentSessionRuntime::fork(entryId)` 调用时，应以 `entryId` 为锚点创建新分支并将选中文本置于编辑器，观察到新 session 的 `parentSession` 指向源 session。
5. 当用户在 agent 运行中按 `Enter` 提交消息时，应进入 `steering` 队列（当前 turn 结束后投递）；按 `Alt+Enter` 应进入 `followUp` 队列（全部结束后投递），观察到 `queue_update{steering,followUp}` 事件。
6. 当 `Esc` 取消正在执行的 turn 时，应中止当前 tool/model 调用并将 `steering+followUp` 队列恢复至输入框，观察到 `agent_end{willRetry:false}` 后输入框包含被恢复文本。
7. 当上下文接近 `model.context_window` 阈值时，应触发 `compaction_start{reason:threshold}` 并调用模型生成摘要，回写 `CompactionEntry`；溢出时触发 `overflow` 并重试，观察到 `compaction_end{result.tokensBefore}` 且 `buildContext` 包含摘要而非被截断消息。
8. 当 `AgentSession.compact(reason)` 成功后，`/tree` 视图应能回溯到被压缩前的条目（历史不丢），观察到 `entries` 仍含被摘要覆盖的原始 `message` 条目。
9. 当以 `crossh-agent -p "prompt"` (print 模式) 运行时，应复用同一 `Agent` 内核执行单轮并将结果输出至 stdout 后退出，观察到退出码 0 且无 TUI 初始化。
10. 当以 `crossh-agent --mode json` 或 `SDK createAgentSession({sessionManager:inMemory()})` 运行时，应通过 `AgentSessionEvent` 订阅获得相同 `message/tool_call` 事件流，观察到 JSONL 输出与 interactive 的语义一致。
11. 当 `agent_cli` 拆分后，`crates/crossh-agent` 与 `crates/crossh-ai-sdk` 仍零 `gpui/ratatui/crossterm` 依赖，观察到 `cargo metadata` 依赖图无 UI 泄露且 `scripts/check-architecture.sh` 通过。
12. 当 `thinkingLevel` 切换时，应追加 `thinking_level_change` 条目并影响后续请求的 `reasoning` 参数，观察到切换后 `wire body` 的 `reasoning/effort/thinking` 字段变化（同 `providers.rs` 现有逻辑）。

## 边界与错误

- `SessionManager` 读写对 `path` 进行 `workspace_path` 约束外再加 `fs2::File::lock` 或 `proper-lockfile` 互斥；并发追加时以 `parentId` 冲突返回 `Err(Conflict)`，调用方重试。
- `fork/switch` 的 `entryId` 不存在或 `parentId` 悬空时返回 `Err(NotFound)`，不创建新文件。
- `compaction` LLM 调用失败时走 `auto_retry`（指数退避，最多 3 次），最终失败则回退为截断并标记 `fromHook:false`，不阻塞用户输入。
- `steering/followUp` 队列在 `agent_end` 后按 `one-at-a-time` 逐条投递，避免一次注入多条导致上下文突变。
- `print/json` 模式下 `Ctrl-C` 两次退出：第一次 `abort` 当前 turn，第二次强制退出并持久化已落盘条目。
- 旧版 session 迁移失败时返回 `Err(IncompatibleVersion)` 并提示 `crossh-agent --export` 手动导出，不静默丢弃。

## 接口与状态变更

- `crates/crossh-agent/src/session.rs`：新增 `SessionEntry{type,id,parentId,timestamp}` 变体与 `SessionManager` trait；`AgentSession` 改为 `entries: Vec<SessionEntry>` + 视图 `messages()`；保留 `save_session/load_session` 兼容层。
- `crates/crossh-agent/src/runtime.rs` (新增)：`AgentSessionRuntime {session, services, createRuntime}`，方法 `switchSession/newSession/fork/import`；`AgentSessionServices {cwd, settingsManager, resourceLoader, modelRuntime}`。
- `crates/crossh-agent/src/event.rs` (新增)：`AgentSessionEvent {agent_end, agent_settled, queue_update, compaction_start/end, entry_appended, session_info_changed, thinking_level_changed}` + `EventBus`。
- `crates/crossh-agent/src/compaction.rs` (新增)：`shouldCompact(usage) -> {threshold,overflow}` + `summarize()`；`CompactionEntry{summary, tokensBefore, firstKeptEntryId, usage}`。
- `crates/crossh-agent/src/lib.rs`：重导出 `SessionManager, AgentSessionRuntime, AgentSessionEvent`。
- `src/agent_cli.rs`：拆为 `src/agent_cli/{app.rs, view.rs, runtime.rs}` + 现有 `input.rs/render.rs`；`app.rs` 仅组装，逻辑下沉至 `crossh-agent`。
- `src/bin/crossh-agent.rs` (或 `src/main.rs` 的 agent 分支)：新增 `--print/-p --mode json|rpc --session-dir --tools/--no-tools` 参数解析（复用 `pi` 的 `args.ts` 语义子集）。
- 持久化格式：`~/.config/crossh/agent/sessions/<projectKey>/<id>.jsonl` 从 `kind:session + kind:message` 升级为 `type:session(type:session) + type:{message,compaction,branch_summary,...}`，`CURRENT_SESSION_VERSION: 1 -> 3`，旧版读取时迁移。

## 平台影响

- 纯逻辑 + ratatui TUI，无平台差异；本地 `cargo test --workspace` + `scripts/check-architecture.sh` 验证，无非 macOS CI 义务。

## 涉及纪律

- [x] Logic must not depend on UI（层级）：`crossh-agent/crossh-ai-sdk` 禁止 `gpui/ratatui/crossterm`，`src/agent_cli` 仅消费 `crossh-agent` 公开 API。
- [x] Keep the app entry point thin：`main.rs` 仅 `window setup + keybindings + boot`，agent 启动逻辑收敛至 `runtime.rs`。
- [x] Split vertical features, then split logic and view inside each：`agent_cli` 内 `logic(runtime/event/compaction)` 与 `view(render)` 分离。
- [ ] Feature-owned settings：本 spec 不新增 settings 项，沿用 `config.rs` 的 `[agent]` 段。
- [x] 文件规模 < 2000 行：拆分后每个文件 < 800 行，`check-architecture.sh` 白名单无需新增。
- [x] 工程笔记 / ADR 同步义务：新增 ADR 0015（Session 树与 Runtime 边界），`docs/architecture.md` 登记。
- [ ] 响应式 UI：TUI 最小尺寸可用性在 `render.rs` 保持。

## 影响模块

- `crates/crossh-agent/src/{session.rs, runtime.rs(新增), event.rs(新增), compaction.rs(新增), lib.rs, providers.rs, policy.rs, tests.rs}`
- `crates/crossh-ai-sdk/src/lib.rs`（仅消费 `ThinkingLevel/Protocol`，不改）
- `src/agent_cli/{app.rs(新增), view.rs(新增), runtime.rs(新增), input.rs, render.rs, mod.rs}`
- `src/bin/crossh-agent.rs` 或 `src/main.rs` 的 `spawn_agent_process` 分支
- `docs/architecture.md`、`docs/adr/0015-*.md`、`crates/crossh-agent/README.md`

## 验收清单

- [ ] spec 评审通过（AI 评审 + 人批准）
- [ ] 行为契约全部固化为失败测试并确认失败原因正确（Red）
- [ ] 最小实现通过聚焦测试（Green）
- [ ] `cargo fmt --check`
- [ ] `scripts/check-architecture.sh`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] 声明的平台 CI job 通过（无）
- [ ] 结构性决策提炼进 ADR 0015 并登记 `docs/architecture.md`
- [ ] 调试根因合并进 `docs/engineering-notes/`（如有）
- [ ] 新增行为合并进 `docs/testing.md` 关键行为矩阵（如有）
- [ ] 用户可观察效果人工确认：`/tree` 分支、`fork`、`Enter/Alt+Enter` 队列、`Esc` 恢复、`-p` 单轮、`--mode json`

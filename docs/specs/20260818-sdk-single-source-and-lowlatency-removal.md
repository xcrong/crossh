# SDK 单一事实来源收敛与 low-latency 残骸移除

> 本 spec 同时处理审计发现 S-1（SDK 类型镜像）与 S-2（low-latency 残骸），并取代
> `docs/specs/20260818-remove-sdk-tool-approval-field.md`（其目标并入本 spec）。

## 元数据

- 状态：`done`
- 创建：2026-08-18
- 相关 ADR：无新增（不改变 crate 边界）；执行 ADR 0002/0003 的 logic/UI 分层
- 相关 issue / 路线图项：`docs/audit/2026-08-18-architecture-redundancy.md`（S-1、S-2）
- 取代：`docs/specs/20260818-remove-sdk-tool-approval-field.md`
- CI 平台影响：`无（纯逻辑 + 无平台差异的菜单项删除，本地 macOS 验证）`

## 背景

**S-1（SDK 类型镜像）**：`crossh-ai-sdk` 与 `crossh-agent` 之间维护着 10 个逐字段相同的类型
（`AgentRole`↔`Role`、`AgentMessage`↔`Message`、`AgentToolCall`↔`ToolCall`、
`AgentToolResult`↔`ToolResult`、`AgentContentBlock`↔`ContentBlock`、`AgentEvent`↔`Event`、
`AgentResponse`↔`Response`、`AgentToolDefinition`↔`ToolDefinition`、`AgentProtocol`↔`Protocol`、
`AgentThinkingLevel`↔`ThinkingLevel`），并在 `providers.rs` 用 6 个零变换转换函数
（`to_sdk_protocol`/`to_sdk_thinking`/`to_sdk_message`/`from_sdk_response`/`from_sdk_event`/
`sdk_tool_definitions`）逐字段搬运。SDK 的定位是 provider-neutral 通用适配层（canonical 消息、
HTTP/SSE 传输、OpenAI Chat/Responses 与 Anthropic Messages 适配、未来 provider 扩展点），
agent 是它的消费方。镜像状态使 wire 事实（如 serde rename、thinking budget 魔数）需要双处
手动同步，单边漂移会静默破坏 wire 格式。本次将 SDK 确立为唯一事实来源，agent 退化为纯消费者。

前序 spec `20260818-remove-sdk-tool-approval-field.md`（已 done）已删除 SDK
`ToolDefinition.requires_approval` 字段并固化守卫测试（wire 请求不含审批元数据）；
本 spec 将其作为保持性约束继续遵守。

**S-2（low-latency 残骸）**：low-latency shell input 的行为实现已在 fork 重构（`542eb3e`）
中随 `terminal/input.rs` 删除，但 seam 全量残留：`WorkspacePane::toggle_low_latency` trait
方法与三个空实现、`TerminalPaneInfo` 两个恒 false 字段、`tab_strip` 中永远禁用的
"低延迟 Shell 输入"菜单项、`ShellMenuAction::ToggleLowLatencyShellInput` action 变体、
两个 locale key。这违反 AGENTS.md「No backward-compatibility bloat」纪律。

## 目标

1. `crossh-ai-sdk` 成为消息、工具、协议、思考级别的唯一事实来源；`crossh-agent` 不再定义任何镜像类型。
2. 删除 `providers.rs` 全部 `to_sdk_*`/`from_sdk_*` 转换胶水。
3. SDK `ToolDefinition` 不再暴露没有生产消费者的 `requires_approval` 字段；审批标志归属 agent 层不变。
4. 对外行为不变：三个 provider 的 wire 请求逐字节一致、`AgentSession` JSONL 持久化形状一致、工具审批流程不变。
5. 移除 low-latency shell input 的全部残骸（trait 方法、字段、空 impl、菜单、action 变体、i18n key）。

## 非目标

- 不合并 `crossh-ai-sdk` 进 `crossh-agent`：crate 边界保留，SDK 作为通用适配层独立演进。
- 不新增 SDK 的第二消费者。
- 不改变 provider 协议、工具名称、工具执行流程、审批策略或用户提示文本。
- 不处理审计 backlog 中 S-3 至 S-10（重复 helper、settings 读取去重、prompt 工具名单、路径防护注释、crossh-assets / crossh-terminal 边界裁定）。
- 不参与进行中的 `shell.rs` / `agent_cli.rs` 拆分重构（仅保证本变更与其不冲突）。

## 行为契约

命名前缀：`spec_20260818_sdk_`（S-1）与 `spec_20260818_lowlatency_`（S-2）。

1. 当 `crossh-agent` 不再定义 `AgentRole`/`AgentMessage`/`AgentToolCall`/`AgentToolResult`/
   `AgentContentBlock`/`AgentEvent`/`AgentResponse`/`AgentProtocol`/`AgentThinkingLevel`
   镜像类型时，其消费方（`src/agent_cli.rs`、`src/features/settings/window.rs`、
   `src/features/workspace/shell.rs` 等）应经 agent 再导出的 SDK canonical 类型编译通过，
   观察到全仓对上述镜像定义与 `to_sdk_*`/`from_sdk_*` 函数的引用为零。
2. 当 provider 编码改为直接消费 SDK canonical 类型时，对同一组消息、工具与模型配置，
   三个内置 adapter（OpenAI Chat、OpenAI Responses、Anthropic Messages）生成的 wire 请求
   与变更前逐字节一致，观察到协议测试断言字节相等。
3. 当 `AgentSession` 持久化使用 SDK canonical 类型时，JSONL 记录的结构（字段名与嵌套）
   与变更前一致，观察到既有会话可被原样读取、新写入记录结构等价。
4. 当 agent 构造工具定义时，工具名称、描述与参数 schema 单一来源于 SDK `ToolDefinition`；
   需要审批的标记保留在 agent 层结构，观察到 SDK 层不携带该标记、agent 层仍能判断。
   （保持性约束：前序 spec 已删除 SDK 字段并有守卫测试，本 spec 的 Red 阶段直接复用该测试。）
5. 当 agent 执行标记为需要审批的工具时，执行前仍等待批准；拒绝时工具不执行，观察到
   既有审批回归测试保持通过。
6. 当 SDK `ToolDefinition` 构造时，不提供 `requires_approval` 参数；SDK 与 agent 的测试
   不再通过该字段构造，观察到测试套件零引用。（保持性约束，同上。）
7. 当 `WorkspacePane::toggle_low_latency` 从 trait 删除后，terminal/sftp/forwarding 三个
   视图不再提供空实现，观察到 `src` 与 `crossh-ui` 编译通过且无 dead_code 警告。
8. 当 `TerminalPaneInfo` 的 `low_latency_enabled`/`low_latency_available` 字段删除后，
   `tab_strip` 不再构造"低延迟 Shell 输入"菜单项，观察到标签右键菜单不再出现该项
   （用户可观察效果）。
9. 当 `ShellMenuAction::ToggleLowLatencyShellInput` 变体与 `shell.rs` 分发分支删除后，
   全仓 `rg low_latency` 命中为零，观察到编译通过。
10. 当两个 locale key（`en.yml`、`zh-CN.yml`）删除后，i18n 查找不再引用它们，
    观察到无编译或运行警告。

## 边界与错误

- provider 请求构造失败时，仍返回原有错误类型与错误路径，不因类型收敛改变。
- 工具审批回调不可用或用户拒绝时，仍沿用 agent 层当前的取消/拒绝结果。
- SDK 类型收敛后，`AgentResponse::text()`/`reasoning()` 等 agent 层便利能力以同 crate
  `impl` 扩展或等价机制保留，不丢失消费方当前依赖的访问器。
- 类型改名迁移期间的编译错误是预期中间态；以最终 `cargo clippy --all-targets -- -D warnings`
  与 `cargo test --workspace` 全绿为准。
- S-2 删除面均为 no-op 路径，无运行时失败路径；唯一用户可观察变化是禁用菜单项消失。

## 接口与状态变更

- `crates/crossh-ai-sdk/src/lib.rs`：删除 `ToolDefinition.requires_approval` 字段与构造参数；
  canonical 类型、`Client`、`StreamAccumulator`、HTTP/SSE 传输与三个 adapter 保持不变。
- `crates/crossh-agent/src/messages.rs`：镜像类型全部删除，改为经 agent 层再导出 SDK 类型。
- `crates/crossh-agent/src/policy.rs`：`AgentProtocol`/`AgentThinkingLevel` 镜像删除，改用 SDK；
  审批策略与 thinking 决策逻辑留在 agent 层。
- `crates/crossh-agent/src/tools.rs`：工具定义与 SDK `ToolDefinition` 合一；审批标志以
  agent 层结构携带。
- `crates/crossh-agent/src/providers.rs`：删除 6 个转换函数，直接消费 SDK 类型。
- `crates/crossh-agent/src/session.rs`：JSONL 序列化改用 SDK 类型（形状不变）。
- `src/features/workspace/pane.rs`：删除 `toggle_low_latency` 与 `TerminalPaneInfo` 两个字段。
- `src/features/{terminal,sftp,forwarding}/view.rs`：删除三个空实现。
- `src/features/workspace/{shell.rs,tab_strip.rs}`：删除分发逻辑与菜单块。
- `crates/crossh-ui/src/context_menu.rs`：删除 `ToggleLowLatencyShellInput` 变体。
- `locales/{en,zh-CN}.yml`：删除 `low_latency_shell_input` key。
- `src/agent_cli.rs`、`src/features/settings/window.rs`：类型引用随 agent 再导出更新。
- `docs/architecture.md`：SDK 职责描述补充「canonical 单一事实来源 / 通用适配层，agent 为消费方」。

## 平台影响

- 纯 Rust 逻辑与跨平台一致的菜单项删除；由本地 macOS arm64 workspace 测试与
  clippy 验证，无非本机平台 CI 义务。

## 涉及纪律

- [x] Logic must not depend on UI（层级）：SDK/agent 仍为纯逻辑层，无 gpui 依赖新增；
      S-2 仅删除 UI 侧 no-op 路径。
- [x] 文件规模 < 2000 行（scripts/check-architecture.sh）：本变更以删除为主，不增加超限文件。
- [x] 工程笔记 / ADR 同步义务：无新结构性边界，不新增 ADR；`docs/architecture.md` 的
      SDK 职责表述随变更同步。
- [x] No backward-compatibility bloat：S-2 正是一次「无历史包袱」清理，不写兼容 shim。
- [ ] Feature-owned settings / 图标纪律 / 响应式 UI：不涉及。

## 影响模块

- `crates/crossh-ai-sdk/src/lib.rs`
- `crates/crossh-agent/src/{lib.rs, messages.rs, policy.rs, tools.rs, providers.rs, session.rs, tests.rs}`
- `src/agent_cli.rs`、`src/features/settings/window.rs`
- `src/features/workspace/{pane.rs, shell.rs, tab_strip.rs}`
- `src/features/{terminal,sftp,forwarding}/view.rs`
- `crates/crossh-ui/src/context_menu.rs`
- `locales/{en,zh-CN}.yml`
- `docs/architecture.md`、`crates/crossh-agent/README.md`、`crates/crossh-ai-sdk/README.md`

## 验收清单

- [x] spec 评审通过（AI 评审 + 人批准）
- [x] 行为契约全部固化为失败测试并确认失败原因正确（Red）——契约 4/6 复用前序 spec 守卫测试；其余为行为不变快照测试 + 编译/符号验证
- [x] 最小实现通过聚焦测试（Green）
- [x] `cargo fmt --check`
- [x] `scripts/check-architecture.sh`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `cargo test --workspace`（干净环境：`env -u CROSSH_UPDATE_SIGNING_KEY`，本地 shell 注入该变量会误伤 manifest 签名测试）
- [x] 全仓 `rg "AgentMessage|AgentRole|to_sdk_|from_sdk_|low_latency"` 零命中（白名单外）
- [x] 右键菜单不再出现「低延迟 Shell 输入」项（用户可观察效果人工确认）——菜单构造代码已全部删除，`rg` 零命中；GUI 运行验证待用户
- [x] `docs/architecture.md` 与两个 crate README 的职责表述已同步

> 新 ADR：无（crate 边界未变，SDK 职责表述已同步进 `docs/architecture.md`）。
> 行为矩阵：无新增行为，守卫测试已随 Spec 命名固化在代码中。

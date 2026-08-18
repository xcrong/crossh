# 收敛 SDK ToolDefinition 的审批字段

## 元数据

- 状态：`done`
- 创建：2026-08-18
- 相关 ADR：无
- 相关 issue / 路线图项：`docs/audit/2026-08-18-simplification-audit.md`（S-1）
- CI 平台影响：`无（纯逻辑）`

## 背景

`crossh-ai-sdk::ToolDefinition` 当前携带 `requires_approval` 字段，但 provider adapter 编码请求时不读取它。工具审批实际由 `crossh-agent` 的工具定义和 agent CLI 负责，SDK 字段没有生产行为消费者，形成跨 crate 的无效状态。

## 目标

1. SDK 的工具定义不再暴露没有生产消费者的审批字段。
2. Agent 层现有工具审批行为保持不变：需要审批的工具仍然在执行前进入审批流程，不需要审批的工具仍然直接执行。
3. OpenAI Chat、OpenAI Responses 和 Anthropic Messages 的请求编码结果保持不变。

## 非目标

- 不改变工具审批策略、工具名称、工具执行流程或用户提示文本。
- 不新增 provider 对审批字段的 wire 编码。
- 不修改 provider 协议、持久化格式或设置项。
- 不处理 `agent_cli.rs` 和 workspace shell 的文件拆分；它们属于本轮另外两个行为不变重构任务。

## 行为契约

1. 当 agent 构造 provider 工具定义时，SDK 工具定义仍包含工具名称、描述和参数 schema，并能被三个内置 provider adapter 编码。
2. 当 agent 执行一个标记为需要审批的内置工具时，执行前仍等待批准；拒绝时工具不执行。
3. 当 agent 执行一个不需要审批的内置工具时，工具仍直接执行。
4. 对同一组消息、工具和模型配置，三个 provider adapter 生成的 wire 请求与变更前一致。
5. SDK 和 agent 的测试不再通过已经删除的审批字段构造 SDK `ToolDefinition`。

## 边界与错误

- provider 请求构造失败时，仍返回原有错误类型和错误路径。
- 工具审批回调不可用或用户拒绝时，仍沿用 agent 层当前的取消/拒绝结果。
- SDK 工具定义缺少审批字段不应导致审批逻辑下沉到 SDK 或 provider adapter。

## 接口与状态变更

- 删除 `crossh-ai-sdk::ToolDefinition.requires_approval` 及其构造参数。
- 不改变 `crossh_agent::AgentToolDefinition.requires_approval`。
- 不改变 provider wire 格式。

## 平台影响

- 仅涉及平台无关 Rust 逻辑；由本地 macOS arm64 workspace 测试验证。

## 涉及纪律

- [x] Logic must not depend on UI（层级）：只修改 SDK/agent 纯逻辑层。
- [x] 文件规模 < 2000 行：不增加超限文件。
- [ ] 工程笔记 / ADR 同步义务：无新结构性边界。

## 影响模块

- `crates/crossh-ai-sdk/src/lib.rs`
- `crates/crossh-agent/src/providers.rs`
- `crates/crossh-agent/src/tests.rs`
- 相关 provider/SDK 测试

## 验收清单

- [x] spec 评审通过（AI 评审 + 人批准）
- [x] 行为契约全部固化为失败测试并确认失败原因正确（Red）
- [x] 最小实现通过聚焦测试（Green）
- [x] `cargo fmt --check`
- [x] `scripts/check-architecture.sh`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `cargo test --workspace`
- [ ] 新增行为合并进 `docs/testing.md` 关键行为矩阵（如有）

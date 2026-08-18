# Crossh 简化扫描报告（2026-08-18）

触发原因：用户要求评估仓库是否符合 KISS 原则。

扫描方式：读取 `AGENTS.md`、`docs/architecture.md`、全部 ADR、工程经验索引和上一轮简化报告；检查 workspace 结构、生产引用、`#[allow(dead_code)]`、超长源文件、重复 helper/API、Cargo 依赖和已有简化 backlog。尝试运行 `cargo clippy --workspace --all-targets -- -D warnings`，但提权审批服务因当前模型不受支持而拒绝，未能重新执行该检查。

## 总体结论

仓库整体符合 KISS，且比上一轮扫描时更接近：上一轮识别的死契约、重复文本编辑实现、无消费者组件、冗余入口和发布校验重复项均已有删除、合并或明确保留理由。当前没有发现应立即删除的生产模块。

本轮唯一的中等强度候选已完成处理：SDK 的审批字段已删除，审批策略仍保留在 agent 层；另外有 2 个低风险维护信号，不足以单独发起重构。

## 发现

| 编号 | 问题 | 严重度 | 证据与消费者结论 |
| --- | --- | --- | --- |
| S-1 | `crossh-ai-sdk::ToolDefinition.requires_approval` 当前没有 SDK 层行为 | 中，已完成 | 原字段定义于 `crates/crossh-ai-sdk/src/lib.rs:133-151`，由 `crates/crossh-agent/src/providers.rs:200-210` 传入；现已删除 SDK 字段和构造参数。审批判断仍在 `src/agent_cli.rs:689,1002,1821-1825`，消费 agent 层的 `AgentToolDefinition.requires_approval`。spec 与契约测试见 `docs/specs/20260818-remove-sdk-tool-approval-field.md`。 |
| S-2 | `src/agent_cli.rs`、`src/features/workspace/shell.rs` 接近文件大小上限 | 低 | 当前分别约 1862、1906 行，尚未违反 `scripts/check-architecture.sh` 的 2000 行规则；两者都包含输入、状态、异步任务和渲染相关逻辑。文件长度本身不是删除理由，但继续扩展会增加局部复杂度，应在下一次相关 feature 变更时按垂直职责拆分。 |
| S-3 | 跨平台打包脚本存在资产拷贝逻辑重复 | 信息 | `scripts/package-windows.ps1` 与 `scripts/copy-shared-assets.sh` 有相似流程，但这是 PowerShell 与 shell 之间的跨平台边界。上一轮已裁定保持现状；将其统一为预生成资产包会增加发布流程状态和故障面，当前不符合 KISS 的净收益标准。 |

## 处置 Backlog

| 优先级 | 编号 | 建议处置 | 说明 |
| --- | --- | --- | --- |
| 已完成 | S-1 | 删除 SDK 字段及构造参数 | agent 层审批策略保持不变；三个 provider adapter 的 wire 请求新增测试确认不含审批元数据。 |
| P3 | S-2 | 随功能变更拆分 | 不建议为降低行数单独重构；拆分前先定义状态、输入处理和渲染的所有权，避免把逻辑搬到更多薄包装中。 |
| 保留 | S-3 | 不处理 | 跨平台脚本的少量重复低于引入新的打包中间产物的复杂度。 |

## 已确认的有意复杂度

- logic/UI 分层、独立 `crossh-git`/`crossh-agent` 入口和 feature-owned settings 由 ADR 明确规定，不能按表面复杂度删除。
- SSH 连接生命周期、known_hosts 决策和路径逃逸防护有工程经验记录与生产消费者，应保留。
- `crossh-theme` 的 renderer-independent token 与 `crossh-ui` 的 GPUI 颜色适配是跨渲染层的两种表示，不是可直接折叠的重复状态。
- `#[allow(dead_code)]` 当前命中项均有跨编译单元、测试 feature 或独立 binary 的解释；没有新增可确认的死代码命中。

## 与 SDD 工作流的衔接

S-1 如果要删除公共 SDK 字段，应先创建并人工批准 spec，再用 SDK/provider 的协议测试证明字段移除或正式编码后的行为。S-2 属维护性建议，暂不创建 spec。S-3 已有裁定，不创建新的提案。

## 本轮未决项

未能重新运行 Clippy，因为提权审批服务拒绝了执行请求；本报告的死代码结论基于静态引用核验和上一轮已记录的 workspace Clippy 结果，不宣称本轮 Clippy 通过。

# crossh-agent

职责：提供与供应商无关的 agent 消息、模型配置、工具执行、流式响应和会话文件管理。

边界：

- 不依赖 GPUI；agent loop、工具权限和会话编排由应用层调用。
- canonical 消息、协议与思考级别由 `crossh-ai-sdk` 单一事实来源提供（本 crate 直接消费并再导出，不维护镜像类型）；provider wire protocol 由 `crossh-ai-sdk` 负责，本 crate 不拥有视图状态。
- 审批策略与工具审批标志归属本 crate，不进入 wire。

公开入口：`Message`、`Role`、`Event`、`Response`、`ToolCall`、`ToolResult`、`Protocol`、`ThinkingLevel`（自 SDK 再导出）、`AgentSettings`、`builtin_tools`、`execute_tool_with_cancel`、`session::AgentSession`。

快速验证：`cargo test -p crossh-agent`
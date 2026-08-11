# crossh-agent

职责：提供与供应商无关的 agent 消息、模型配置、工具执行、流式响应和会话文件管理。

边界：

- 不依赖 GPUI；agent loop、工具权限和会话编排由应用层调用。
- provider wire protocol 由 `crossh-ai-sdk` 负责，本 crate 不拥有视图状态。

公开入口：`AgentMessage`、`AgentSettings`、`complete_stream`、`builtin_tools`、`execute_tool`、`session::AgentSession`。

快速验证：`cargo test -p crossh-agent`

# crossh-ai-sdk

职责：canonical 单一事实来源（消息、工具、协议、思考级别）加上统一 OpenAI Chat、OpenAI Responses 和 Anthropic Messages 的请求、响应与 SSE 流模型。

边界：

- 只负责 provider wire boundary、HTTP 和事件归一化，不负责 agent loop、工具权限或 UI。
- 上层通过 adapter 接口接入协议，避免把供应商格式泄漏到业务逻辑；`crossh-agent` 是消费方，直接消费本 crate 的 canonical 类型，不维护镜像。

公开入口：`Protocol`、`Message`、`CompletionRequest`、`Client`、`ProviderAdapter`、`builtin_adapter`。

快速验证：`cargo test -p crossh-ai-sdk`

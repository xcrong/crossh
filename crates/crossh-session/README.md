# crossh-session

职责：常驻进程 `crossh-daemon` 的属主实现，拥有 PTY 与输出环形缓冲，持有项目/标签页/递归分屏工作区并持久化到 `daemon/workspace.json`，对外提供协议 v2 会话与工作区服务。

边界：

- 零 `gpui` / `gpui_platform` / `crossh-terminal` 依赖，不负责任何视图状态。
- GUI 只消费快照，布局与会话生命周期归本 crate；纯逻辑可被 CLI/GUI 共用。

公开入口：`client`、`session`、`workspace`、`workspace_store`、`workspace_actor`、`protocol`、`handshake`、`feed`、`instance`、`spawn`，以及二进制 `crossh-daemon`。

快速验证：`cargo test -p crossh-session`

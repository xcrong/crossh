# crossh-terminal

职责：定义终端 feature 可跨 UI 边界传递的事件、连接状态、时间戳和设置。

边界：

- 依赖 `crossh-core` 的连接状态，不依赖 GPUI，也不负责终端渲染或 PTY 生命周期。
- 应用层负责把这些 contracts 连接到具体的终端 view。

公开入口：`TerminalSettings`、`TerminalEvent`、`ConnState`、`events`、`settings`、`timestamps`。

快速验证：`cargo test -p crossh-terminal`

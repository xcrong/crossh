# crossh-terminal-view

职责：终端的 GPUI 视图宿主，拥有 `TerminalView`、按键绑定、daemon 链接与链接打开器，把 `crossh-session` 的会话输出投影为 `PaneId` 终端视图。

边界：

- 模型层事件与设置在 `crossh-terminal`，PTY 与工作区快照在 `crossh-session`；本 crate 不持有布局，只做渲染、交互与宿主生命周期。
- 本地裁剪的 Zed `terminal_view` 基础（渲染与交互）只在本 crate内使用，不向应用层透出。

公开入口：`TerminalView`、`TerminalItem`、`init`、`daemon_link`、`LinkOpener`（`open_link`、`set_link_opener`）、`set_text_resolver`，以及复出的 `ConnState`、`TerminalEvent`。

快速验证：`cargo test -p crossh-terminal-view`

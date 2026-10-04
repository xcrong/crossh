# crossh-note-view

职责：GPUI Note Viewer 功能：窗口、列表/搜索/标签、`crossh-editor` 输入态与 Markdown 预览。

边界：

- 只被 `src/bin/crossh-note.rs` 消费；笔记存储层在 `crossh-note`，文本编辑语义来自 `crossh-core`。
- 不初始化终端、工作区与设置功能。

模块划分：

- `window.rs` — 窗口状态、脏保护、列表与编辑区渲染、快捷键分发。
- `find_panel.rs` — 笔记内查找浮层（面板 UI + 查询输入 + 上下一个匹配）。
  匹配引擎与命中高亮由 `crossh-editor` 的 `TextareaState::search_session` 提供。
- `format.rs` — 纯格式化逻辑（标题截断、相对时间、错误码文案），零 `gpui` 依赖。
- `markdown.rs` — Markdown 预览，直接复用 `crossh_editor::TextView`。

快速验证：`cargo test -p crossh-note-view`

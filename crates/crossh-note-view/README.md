# crossh-note-view

职责：GPUI Note Viewer 功能：窗口、列表/搜索/标签、`crossh-editor` 输入态与 Markdown 预览。

边界：

- 只被 `src/bin/crossh-note.rs` 消费；笔记存储层在 `crossh-note`，文本编辑语义来自 `crossh-core`。
- 不初始化终端、工作区与设置功能。

公开入口：`init`、`open_note_window`。

快速验证：`cargo test -p crossh-note-view`

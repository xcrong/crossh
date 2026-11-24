# crossh-note

职责：本地 SQLite（WAL + FTS5 + 触发器同步）笔记存储，全文检索把 FTS 与正文子串两条路径取并集，再按置顶 → 命中次数 → 首次命中位置 → 更新时间排序，支持置顶与软删恢复。

边界：

- 零 `gpui` 依赖的纯逻辑层，不负责窗口、主题与文本编辑语义。
- 视图层在 `crossh-note-view`，文本编辑语义与启动器在 `crossh-core`（`note_launcher`）。

公开入口：`NoteStore`、`Note`、`MAX_CONTENT_BYTES`、`note_db_path`，以及 `open`、`open_default`、`list`、`search`、`create`、`update`、`delete`、`restore`、`set_pinned`、`purge_deleted`、`clear_all`、`get`。

快速验证：`cargo test -p crossh-note`

# 0016-Standalone Note Viewer binary

## 状态

已接受

## 背景

原 `bank` 将笔记以 `AppShell` 弹窗（`ModalDialog 760px`）嵌入主工作区，导致焦点/IME/滚动与 Zed Editor 样式耦合，且布局受 `AppShell` 约束。笔记需要长期 SQLite 持久化、FTS5 搜索、标签与 Markdown 预览等简化 Obsidian 能力，应与主工作区的终端/SSH/SFTP 生命周期解耦。参考 `0008 Standalone Git Viewer` 的独立二进制经验，笔记更适合独立进程。

## 决策

Cargo package 同时产出 `crossh`、`crossh-git`、`crossh-note` 三个二进制。`crossh` 仅保留轻量笔记启动器 `note_launcher.rs`（`spawn_note_process`）与底部状态栏 `FileText` 入口；`crossh note` 与状态栏入口均负责启动同目录的 `crossh-note`，找不到时回退到 `PATH` 并 `log::warn`。

新增纯逻辑 crate `crates/crossh-note`（`rusqlite 0.32 bundled` + FTS5 + `parking_lot::Mutex`），零 `gpui` 依赖，提供 `NoteStore`（`WAL`、`user_version=1`、`notes` + `notes_fts` + 触发器，损坏备份 `note.db.corrupt.<ts>`，`MAX_CONTENT_BYTES=8*1024`、`MAX_TAGS_BYTES=1024`）。`crossh-note` 仅初始化 GPUI、主题、资源与 `src/features/note`（`window`/`markdown`/`render`），使用 `editor::Editor::for_buffer(Buffer::local, None)` 纯文本无 Project，`pulldown-cmark` 预览；`src/features/note` 源码仅由 `crossh-note` 入口通过 `#[path]` 装配，不进主 `crossh` 编译单元。

`note.db` 与 `settings.toml` 同目录（`dirs::config_dir()/crossh/note.db`），`pinned` 置顶再按 `updated_at` 倒序，搜索命中 `content` 与 `tags`（`FTS5` 失败回退 `LIKE` 并转义 `%/_`）。启动为单向 fire-and-forget，不建 IPC，主进程无需感知子进程变更。

## 结果/代价

笔记 UI 获得独立窗口布局（`900x600`/`640x400` 最小）、原生 Zed Editor 焦点/撤销/IME 与主题一致性，主程序不携带笔记 UI 与 `rusqlite`，两类二进制可独立优化。代价是发布包需携带第三个可执行文件，状态栏重复点击可能启动多个 Note 窗口，且主程序与 `crossh-note` 需保持兼容版本；本地仍仅验证 macOS arm64，Linux/Windows/x86_64 由 Actions 验证。

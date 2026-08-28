# Crossh Note 独立笔记

> 复制本文件到 `docs/specs/YYYYMMDD-<slug>.md`，填写后进入评审。
> 只描述行为与验收，不写实现方案。语言与项目文档保持一致。

## 元数据

- 状态：`approved`
- 创建：2026-08-27
- 相关 ADR：`docs/adr/0008-standalone-git-viewer.md`、`docs/adr/0016-standalone-note-viewer.md`（待建）
- 相关 issue / 路线图项：bank 原型回退后重设计，对标简化 Obsidian
- CI 平台影响：`仅 macOS`（本地 arm64 验证，Linux/Windows 由 Actions 验证二进制装配）

## 背景

原 `bank` 方案将笔记以 `AppShell bank_visible + ModalDialog 760px` 嵌入主工作区，导致焦点/IME/滚动与 Zed Editor 样式耦合，布局与回退路径不可靠。参考 `crossh-git` 独立二进制经验，需要将笔记拆为独立进程 `crossh-note`，主应用仅保留底部状态栏图标入口（fire-and-forget 启动），数据持久化为 SQLite，提供列表/搜索/标签/ Markdown 预览等简化 Obsidian 能力。

## 目标

1. 提供独立二进制 `crossh-note`（`src/bin/crossh-note.rs`），与 `crossh`、`crossh-git` 同包产出，共享 `crossh-assets` 资源目录。
2. 主应用底部状态栏新增笔记入口（`Archive` 图标），点击拉起 `crossh-note` 进程，找不到二进制时回退到 `PATH` 查找并失败提示。
3. 笔记数据长期持久化到 SQLite（`note.db` 与 `settings.toml` 同目录，`WAL` + `user_version` 迁移，损坏时备份为 `note.db.corrupt.<ts>`）。
4. 笔记左侧列表支持搜索（`FTS5 unicode61` 命中 `content/tags`，无 FTS 时回退 `LIKE`），`pinned` 置顶再按 `updated_at` 倒序。
5. 中心编辑器基于 `editor::Editor + language::Buffer::local` 纯文本（无 Project），支持多行编辑、撤销、IME；右侧 Markdown 预览（`pulldown-cmark`）可切换。
6. 支持标签（`tags TEXT`，逗号分隔，搜索与过滤覆盖标签）。
7. 支持固定/取消固定、复制内容、删除、清空全部（需二次确认）。

## 非目标

- 图谱/关系可视化、双向链接图、Canvas 等 Obsidian 高阶能力。
- 多端同步、协作、加密、附件/图片托管。
- Vault 文件夹直接映射或外部 Markdown 文件监听（仅 DB）。
- 主应用内嵌入笔记弹窗或侧栏（本 spec 明确排除，统一走独立窗口）。

## 行为契约

1. 当 `crossh-note` 首次启动且 `note.db` 不存在，应该 创建 `notes(id, content, tags, pinned, created_at, updated_at)` + `notes_fts(content, tags)` + 触发器并置 `PRAGMA journal_mode=WAL`，观察到 `user_version=1`。
2. 当 `note.db` 损坏无法打开，应该 将原文件重命名为 `note.db.corrupt.<ts>` 并新建空库，观察到旧库备份存在且新库可用。
3. 当 创建笔记 `content="hello"` `tags="work,idea"`，应该 `list()` 返回该笔记且 `pinned=0`，观察到 `created_at == updated_at` 且搜索 `hello` 与 `work` 均可命中。
4. 当 更新笔记内容或标签，应该 `updated_at` 递增且 `search()` 的 `FTS5` 索引同步更新，观察到旧关键词不再命中、新关键词可命中。
5. 当 `search("关键词")` 且 `FTS5` 可用，应该 返回 `content LIKE %关键词% OR tags LIKE %关键词%` 的并集按 `pinned DESC, updated_at DESC` 排序，观察到大小写不敏感。
6. 当 切换 `pinned`，应该 `list()` 置顶固定笔记，观察到 `pinned=1` 排在所有 `pinned=0` 之前。
7. 当 删除笔记，应该 `list()` 不再含该 `id` 且 `search` 不再命中，观察到 `delete(nonexistent)` 不报错。
8. 当 `content.trim().is_empty()` 创建或更新，应该 拒绝并提示 `empty_content`，观察到 DB 行数不变。
9. 当 `content` 超过 `MAX_CONTENT_BYTES=8*1024`，应该 截断或拒绝并提示，观察到存储字节数 ≤ 上限。
10. 当 主应用点击状态栏笔记入口，应该 `spawn_note_process()` 以 `current_dir` 为 `cwd` 拉起 `crossh-note`，观察到独立窗口出现（本地 arm64 可视验证）。
11. 当 `crossh-note` 窗口中选中左侧笔记，应该 中心 `Editor` 文本同步为该笔记 `content` 且标签输入同步为 `tags`，观察到光标位于末尾且可编辑。
12. 当 在 `Editor` 中输入多行 `a\nb` 并切换预览，应该 右侧渲染 `pulldown-cmark` 后的 HTML 片段且空内容显示 `预览为空`，观察到 `H1-3/bold/italic/code/list/blockquote/link/rule` 正确。
13. 当 `content` 或 `tags` 包含标签 `work` 且过滤器为 `work`，应该 列表仅含含该标签的笔记，观察到标签点击可触发过滤。
14. 当 清空全部，应该 需二次确认且确认后 `list()` 为空，观察到取消时数据不变。

## 边界与错误

- DB 损坏、磁盘满、权限不足时备份并 toast，不崩溃。
- `FTS5` 不可用（如编译缺失）自动回退 `LIKE` 搜索。
- `crossh-note` 二进制缺失时主应用 `spawn` 失败应 `log::warn` 并 `ToastTone::Error`。
- 窗口关闭时若有未保存变更（`editor.text != store.content`），应提示丢弃或自动保存（首版丢弃并 `Info` toast，与 bank 一致）。
- `content/tags` 含 `'`、`%`、`_` 等特殊字符时搜索需转义，不注入 SQL。

## 接口与状态变更

- 新增 crate `crates/crossh-note`（纯逻辑，无 `gpui`）：`Note {id, content, tags, pinned, created_at, updated_at}`、`NoteStore` API 同契约 3-9。
- 新增二进制 `src/bin/crossh-note.rs`：`AppIdentity "io.github.xcrong.crossh.note"`、`open_note_window(cwd)`。
- 新增 `src/features/note_launcher.rs`：`parse_cli/spawn_note_process/note_process_command`（对标 `git_launcher.rs`）。
- 新增 `src/features/note/{window,render,input,model,markdown}.rs`：仅 `crossh-note` 挂载，不进主 `crossh` 编译单元。
- 状态栏：`render_workspace_status_bar` 右侧新增 `status-note` 按钮（`IconName::Archive 13px`，`tooltip.note`）。
- 持久化：`note.db` 路径 `dirs::config_dir()/crossh/note.db`（`~/.config/crossh/note.db`），与 `settings.toml` 同级；`i18n` 新增 `note.*` 22 keys。
- 无 wire 协议变更，主与子进程无 IPC，仅文件共享。

## 平台影响

- 本地仅验证 macOS arm64 窗口与 `spawn`；Linux/Windows/x86_64 的二进制装配与资源目录由 GitHub Actions 验证。
- `bundled` SQLite 在各平台编译需 `cc`，CI 已覆盖，无本地交叉编译需求。

## 涉及纪律

- [x] Logic must not depend on UI（`crates/crossh-note` 零 `gpui`；`note/window` 依赖 logic，logic 不依赖 view）
- [x] Feature-owned settings（笔记库路径与窗口状态归 `note` feature，不集中到全局 settings）
- [x] 图标纪律（`Archive/Copy/Tag/X` 等均取 Lucide 1.27.0 官方 SVG，`define_icons!` 同步）
- [x] 文件规模 < 2000 行（`scripts/check-architecture.sh` 白名单外强制）
- [x] 工程笔记 / ADR 同步义务（新增 ADR 0016 与 `docs/engineering-notes/note-window.md` 如有 IME/滚动坑）
- [x] 响应式 UI（`note` 窗口自 `WindowOptions` 最小尺寸起可用，左侧 `180-280px` + 右侧 `min 320px` 自适应）

## 影响模块

- `Cargo.toml`、`crates/crossh-note/*`
- `src/bin/crossh-note.rs`、`src/features/note_launcher.rs`、`src/features/note/*`
- `src/features/workspace/view.rs`（状态栏入口）、`locales/en|zh-CN.yml`、`crates/crossh-assets/src/lib.rs`
- `docs/architecture.md`、`docs/adr/0016-standalone-note-viewer.md`、`docs/testing.md`

## 验收清单

- [ ] spec 评审通过（AI 评审 + 人批准）
- [ ] 行为契约全部固化为失败测试并确认失败原因正确（Red）
- [ ] 最小实现通过聚焦测试（Green）
- [ ] `cargo fmt --check`
- [ ] `scripts/check-architecture.sh`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] 声明的平台 CI job 通过（非本机平台：提交后由 Actions 验证，spec 状态保持 in-progress 直到通过）
- [ ] 结构性决策提炼进 ADR（如有）并登记 docs/architecture.md
- [ ] 调试根因合并进 docs/engineering-notes/（如有）
- [ ] 新增行为合并进 docs/testing.md 关键行为矩阵（如有）
- [ ] 用户可观察效果人工确认（独立窗口、状态栏入口、搜索/标签/预览）

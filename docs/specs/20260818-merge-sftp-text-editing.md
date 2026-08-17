# 合并 sftp 字符编辑与 TextEditingState 重复实现（S-A1 + S-A10 联动）

## 元数据

- 状态：`done`
- 创建：2026-08-18
- 相关 ADR：无（共享逻辑层边界不变；`text_editing.rs` 已是既有纯逻辑模块，本次仅收拢实现）
- 相关 issue / 路线图项：`docs/plans/2026-08-17-simplification-backlog.md` S-A1（S-A10 联动，不单独行动）
- CI 平台影响：`无（纯逻辑 + 本地 GPUI 测试）`

## 背景

审计发现 SFTP 编辑器（`src/features/sftp/logic.rs` 的 `backspace_char`/`delete_char`/
`move_cursor_horizontal` 自由函数）与共享状态机 `shared/text_editing.rs`
（`TextEditingState` 方法）实现了逐行相同的无选区退格/删除/横向移动语义，
存在两份字符边界运算。同时 S-A10 指出 `QuickCommandEditor`/`CommitEditor` 的
8+1 个单行转发委托方法与 `state` 直接 pub 字段访问（`shell.rs`、`git/input.rs`
等大量直连 `editor.state.*`）形成两种封装风格并存。

审计建议：给 `TextEditingState` 增加无选区便捷语义或让 `RemoteEditor` 复用状态
机；bool 返回值语义、调试断言是行为契约。S-A10 裁定"随本 spec 一并设计，不单独
行动"。

## 目标

1. SFTP `RemoteEditor` 复用 `TextEditingState` 状态机，删除 `logic.rs` 中 4 个
   重复编辑函数（`insert_text`/`backspace_char`/`delete_char`/
   `move_cursor_horizontal`）与其重复测试，sftp 编辑行为（退格/删除/移动/脏标记）
   逐字节不变。
2. 共享状态机的"是否实际发生修改"语义成为一等契约：`backspace`/`delete`/
   `move_horizontal`/`replace_selection` 返回 bool，供 sftp 脏标记使用；现有
   消费点忽略返回值，行为不变。
3. 调试断言契约保留：状态机编辑方法对光标施加字符边界断言（原 sftp 版持有
   `debug_assert!`）。
4. S-A10 联动：删除 `QuickCommandEditor`/`CommitEditor` 薄委托方法，封装风格
   统一为"调用方直接操作 `state` 字段/方法"，行为不变。

## 非目标

- 不合并 `move_cursor_vertical`/`line_bounds`（sftp 独有的多行编辑能力，shared
  版无对应物，保持原位）。
- 不动 `EndCaretInput`（路径/上传输入，非本 spec 范围）。
- 不统一 settings 的 `with_agent_text_state` 瞬时读写风格（不同机制，非委托层）。
- 不把 `TextEditingState` 字段改为私有（渲染与 IME 处理合法直连字段）。
- 不改 `move_to_boundary`/`select_all`/`clear_composition` 签名（无 bool 消费）。

## 行为契约

1. 当 `TextEditingState` 上执行 `backspace()` 时，返回 `true` 当且仅当文本实际
   发生删除（光标不在开头/选区非空）；否则返回 `false` 且文本、光标、锚点全部
   不变；文本变化时锚点被清除，光标落在合法字符边界。
2. 当 `TextEditingState` 上执行 `delete()` 时，返回 `true` 当且仅当文本实际
   发生删除（光标不在结尾/选区非空）；否则返回 `false` 且文本、光标、锚点全部
   不变；文本变化时锚点被清除，光标落在合法字符边界。
3. 当 `TextEditingState` 上执行 `move_horizontal(direction, extend)` 时，返回
   `true` 当且仅当光标实际移动（或选区端跳跃发生）；在边界（无选区 + 文本开头
   向左/文本结尾向右）返回 `false` 且光标不变（`extend=true` 时锚点建立仍发生，
   bool 只承诺光标位移）；`extend=false` 且存在选区时无条件跳到选区端并清除
   选区（返回 `true`，与光标是否已位于选区端无关）。
4. 当 `TextEditingState` 上执行 `replace_selection(text)` 时，返回 `true` 当且
   仅当文本实际发生变化（选区非空被替换/空文本插入）；空文本 + 无选区返回
   `false`，文本与光标不变，但陈旧锚点被清除（与现状一致）；插入后光标落在
   插入文本末尾、锚点清除。
5. 当向状态机编辑方法传入落在 UTF-8 字符内部的 `cursor` 时（debug 构建），
   触发断言（原 sftp `debug_assert!(content.is_char_boundary(*cursor))` 契约
   上移到状态机）；`cursor` 始终在字符边界时断言不触发。
6. 当 SFTP `RemoteEditor` 执行退格/删除/插入/横向移动时，文本与光标的字节级
   结果与改造前一致（对 `"a✓b"`、`"ab你好\nxyz"` 等 UTF-8 混合文本逐例断言）；
   `dirty` 仅在编辑实际发生修改时置位（空退格、边界移动、空文本插入不置位）。
7. 当 SFTP `RemoteEditor` 的 IME 组合生效/取消/提交时（`ime_marked_text`/
   `ime_replacement` 的清除、`unmark_text` 光标回退、`replace_and_mark` 记录
   替换区间），行为与改造前一致（改由 `state` 字段承载，语义不变）。
8. 当调用方对 `QuickCommandEditor`/`CommitEditor` 执行编辑与查询时，`state`
   直连调用的结果与改造前经委托方法的结果完全一致（委托方法删除后由既有
   git/workspace 消费点测试覆盖）。

## 边界与错误

- sftp 编辑器恒无选区/锚点：`RemoteEditor` 无 anchor 字段，`view_input.rs`
  `selected_text_range` 恒返回 `position..position`。合并后 `TextEditingState`
  的锚点清除分支（退格/删除/替换时 `anchor = None`）对 sftp 不可达，不是行为
  变化；对 git/settings 消费点该分支与现状一致。
- `replace_selection("")` 在无选区时清除陈旧锚点（`anchor == cursor` 的幽灵
  锚点），与现状一致，仅返回 bool 附加信息。
- dirty 语义按输入路径划界：契约 6 约束键输入路径（`insert`/`backspace`/
  `delete`/`move_horizontal` 仅在实际修改时置位 dirty）；IME 组合提交路径
  （`view_input.rs` `replace_text_in_range`）保持现状的**无条件** `dirty = true`
  （契约 7 优先于契约 6）。
- 调试断言在 release 构建下被编译掉，release 行为与现状一致；非边界输入在
  debug 构建下任何实现（含现状切片）都会 panic，断言上移只是显式化，非新风险。

## 接口与状态变更

- `shared/text_editing.rs` 公共 API：`backspace`/`delete`/`move_horizontal`/
  `replace_selection` 返回值从 `()` 改为 `bool`（消费点忽略返回值，源码兼容；
  破坏性仅针对显式断言返回值 `()` 的调用——全仓库不存在）。
- `sftp/logic.rs`：删除 `insert_text`/`backspace_char`/`delete_char`/
  `move_cursor_horizontal` 及对应测试（`pub(crate)`，非公共 API）。
- `sftp/view.rs` `RemoteEditor`：`content`/`cursor`/`ime_marked_text`/
  `ime_replacement` 四个字段合并为 `state: TextEditingState`（视图内部结构）；
  `insert`/`backspace`/`delete`/`move_horizontal` 四个 sftp 侧薄方法保留，
  内部改调 `state` 并按 bool 结果置位 dirty；`move_vertical` 不变。
- `command_editor.rs`/`git/editor.rs`：删除薄委托方法，调用点直连 `state`；
  `git/editor.rs` 的 `editor_keeps_unicode_cursor_on_character_boundaries`
  测试同步改写为 `state` 直连。
- 无 wire / 持久化 / 设置项变化。

## 平台影响

- 无。全部为纯逻辑与 GPUI 视图层代码；本地 macOS 测试全量可验证，CI 通用
  check/test job 覆盖即可，无需声明专门平台 job。

## 涉及纪律

- [x] Logic must not depend on UI（层级）：改动在 `shared/text_editing.rs`
      （纯逻辑，已有架构红线）与 sftp/git/workspace 视图层（依赖纯逻辑）；
      不新增反向依赖；`check-architecture.sh` 持续拦截。
- [x] 文件规模 < 2000 行：`sftp/view.rs` 约 1279 行、`shell.rs` 约 1966 行，
      本 spec 只替换等价行数或略减，不越线。
- [x] 其余纪律（图标、设置、响应式 UI）不适用，标注无。

## 影响模块

- `src/shared/text_editing.rs`（bool 返回 + debug_assert + 契约测试）
- `src/features/sftp/logic.rs`（删 4 函数与测试）
- `src/features/sftp/view.rs`、`view_input.rs`（RemoteEditor 字段与消费点）
- `src/features/workspace/command_editor.rs`（删委托方法 + 模块注释更新）
- `src/features/git/editor.rs`（删委托方法 + 既有测试改写）
- `src/features/workspace/shell.rs`、`src/features/workspace/view.rs`、
  `src/features/git/input.rs`、`src/features/git/render.rs`（调用点改直连
  `state`；其中 workspace/view.rs:1141 与 git/render.rs:1218 的
  `editor.selection()` 委托调用必须改为直连）
- `docs/plans/2026-08-17-simplification-backlog.md`（完成后标注 S-A1）

## 验收清单

- [x] spec 评审通过（AI 评审 + 人批准）
- [x] 行为契约全部固化为失败测试并确认失败原因正确（Red；测试命名前缀
       `spec_20260818_merge_sftp_text_editing_`）
- [x] 最小实现通过聚焦测试（Green）
- [x] `cargo fmt --check`
- [x] `scripts/check-architecture.sh`
- [x] `cargo clippy --workspace --all-targets -D warnings`
- [x] `cargo test --workspace`
- [x] 声明的平台 CI job 通过（无平台影响，不适用）
- [x] 结构性决策提炼进 ADR（如有）并登记 docs/architecture.md（无新边界变化，
       不适用；`text_editing.rs` 共享逻辑归属已有记录）
- [x] 调试根因合并进 docs/engineering-notes/（如有，不适用）
- [x] 新增行为合并进 docs/testing.md 关键行为矩阵（如有：状态机 bool 语义为
       可执行契约，随测试固化即可）
- [x] 用户可观察效果人工确认（针对 UI/交互变更：sftp 编辑器退格/删除/移动——
       契约测试覆盖 UTF-8 字节语义与脏标记；真实主机上的端到端确认同 S-B1
       由 loopback spec 承接）

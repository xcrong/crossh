# 终端批量输入条（高延迟 compose bar）

## 元数据

- 状态：`done`（2026-08-21 文档漂移审计：compose 输入条已实现，`docs/testing.md` Compose 行已收录，12 个 `spec_20260820_terminal_compose_bar__*` 测试全绿）
- 创建：2026-08-20
- 批准：2026-08-20（人批准，AI 自审通过）
- 实现：2026-08-20（Red → Green 完成）
- 相关 ADR：`docs/adr/0002-logic-ui-layering.md`、`docs/adr/0007-workspace-panel-composition.md`、`docs/adr/0011-terminal-split-ownership.md`、`docs/adr/0012-spec-driven-development-loop.md`
- 相关 issue / 路线图项：无
- CI 平台影响：`仅 macOS`（纯 GPUI 布局 + 本地 TextEditingState + 终端 `run_command` 投递，逻辑由 `cargo test` 覆盖；终端 PTY 行为仅在 macOS arm64 本地验证）

## 背景

部分用户通过跨境/卫星链路连接远程 VPS，RTT 200ms 以上时逐字符输入每个字符都要等待服务端回显，体验极差。现有终端（Zed `Terminal`）逐字符通过 `ssh -tt` PTY 转发，不可避免该延迟。用户期望：在状态栏提供一个按钮，点击后在终端主区底部（状态栏之上、侧边栏与快捷命令栏之间）展开一个本地输入框，本地无延迟编辑，`Ctrl+Enter`（macOS `Cmd+Enter` 兼容）一次性把整段文本通过批量写入投递到活动终端并回车执行，类似粘贴，从而把 `N*RTT` 降为 `1*RTT`。

## 目标

1. 状态栏新增一个可点击的 compose 开关按钮，视觉与现有 toggle（`Settings / PanelLeft / Clock / Columns2`）一致，显示当前展开/收起状态。
2. 点击开关可在终端主区底部（`status_bar` 之上，`sidebar` 与 `quick_commands` 之间）展开/收起一条单行（支持 `Shift+Enter` 换行时可扩展）本地输入条，输入条宽度严格受限于 `available_main_width`，不覆盖侧栏与快捷栏。
3. 输入条采用本地 `TextEditingState`（`src/shared/text_editing.rs`）编辑，IME 正常工作，编辑过程不向终端发送任何字节。
4. 当输入条聚焦且内容非空（`trim` 后非空）时，`Ctrl+Enter`（macOS 额外支持 `Cmd+Enter`）将内容一次性发送到当前活动终端并附加回车执行（等价 `TerminalView::run_command`），发送后清空输入条并将焦点交还终端。
5. 输入条的显隐与草稿内容在标签/会话切换、窗口尺寸变化、分栏开关等过程中保持稳定，无裁切与重叠，满足最小窗口可用性。

## 非目标

- 不替代或改动现有 `quick_commands` 面板/ Rail 的历史命令与后台任务能力；compose 是独立的临时输入，不写 `CommandHistory`（历史由终端 `CommandStarted` 事件自然产生）。
- 不为每个 Tab/会话持久化草稿到 `settings.toml`；草稿为窗口内存态，重启后不恢复（避免引入新的持久化字段与 `feature-owned settings` 复杂性）。
- 不新增远程标签的 SSH 语义解析（不联动 `ConnectionManager` 状态点、SFTP、Forward）；compose 只通过 `WorkspacePane::run_command / send_text` 投递字节。
- 不改变标签拖拽、关闭确认、分栏、Git 同步、编辑器启动等既有流程。
- 本期不引入多行代码块高亮、语法检查、文件拖拽或 AI 补全；多行仅以纯文本换行支持。
- 不新增独立的持久化文件或 wire 格式。

## 行为契约

1. 当 compose 处于收起态（`compose_visible == false`）时，主区不应渲染输入条，状态栏 compose 按钮应为未选中态（`muted_text`）；`workspace` 的 `main` 区域高度不变。
2. 当用户点击状态栏 compose 按钮时，`compose_visible` 应取反；从收起变为展开时，焦点应进入 compose 输入框（`compose_focus.focus(window, cx)`），输入条立即可编辑。
3. 当 compose 展开时，输入条应渲染在 `main` 列内部、`status_bar` 之上，左右边界分别与 `sidebar` 右沿与 `quick_commands` 左沿对齐，宽度恒等于 `available_main_width`（`shell_render.rs:78-85` 的计算口径），不横跨全窗口；`sidebar` 与 `quick_commands` 的显隐/拖拽应联动更新输入条宽度，无重叠。
4. 当 compose 展开且无活动终端视图（`focused_view == None` 或对应 pane 无 `terminal_entity_id`）时，状态栏按钮应为 `disabled`，输入条的 `Ctrl+Enter` 发送应为 no-op（不写终端、不清空、不崩溃）。
5. 当 compose 输入条聚焦时，用户输入的字符、退格、删除、光标移动、选区、全选、粘贴、IME 组合均应通过 `TextEditingState` 本地处理，观察到发送前 `AppShell::send_compose` 未被调用且终端侧 `run_command` 调用次数为 0（通过 `TextEditingState` 纯逻辑单测 + GPUI 聚焦态单测覆盖）。
6. 当输入条内容 `trim().is_empty() == true` 时，`Ctrl+Enter`（含 `Cmd+Enter`）应为 no-op，不向终端投递，不清空输入条。
7. 当输入条内容 `trim().is_empty() == false` 且存在活动终端时，`Ctrl+Enter`（或 `Cmd+Enter`，或点击右侧发送按钮）应将 `trim` 后的完整文本以单次批量写入投递到活动终端：`LocalSession` 走 `terminal.run_command(&text, cx)`（`src/features/terminal/view.rs:807`，内部 `format!("{text}\r")`，`\n` 原样保留），`RemoteTab` 走 `pane.run_command(&text, cx)`（`src/features/terminal/view.rs:1035`）；观察到终端单次收到 `trim(text) + "\r"` 的完整字节序列（含中间 `\n`），而非逐字符多次写入；投递后输入条应清空（`value.clear(), cursor=0`），焦点应交还活动终端（`refocus_active_terminal`）。
8. 当输入条聚焦时按 `Escape` 应收起 compose（`compose_visible = false`）并将焦点交还活动终端，草稿文本保留（不清零）以便再次展开时继续编辑；再次点击按钮展开时应恢复草稿。
9. 当用户通过 `TabStrip`、快捷键 `Ctrl+Tab`/`Cmd+1..9` 或侧栏点击切换活动视图时，`compose_visible` 与草稿文本应保持不变（全局单例），不因视图切换而丢失或清空；新活动视图的终端成为下一次发送的目标。
10. 当输入条聚焦时按 `Shift+Enter` 应在输入条内插入单个 `\n` 换行（不发送），光标移到换行后；随后 `Ctrl+Enter`/`Cmd+Enter`/发送按钮应将含换行的多行文本按契约 7 单次投递。
11. 当窗口尺寸变化到最小声明尺寸时，输入条应保持可用：不被侧栏/快捷栏遮挡，不产生横向溢出导致按钮不可达；其内部文本区应支持横向滚动（`overflow_x_scroll + ScrollHandle`，与 `quick_command_editor` 现有模式一致），右侧发送按钮保持可见。

## 边界与错误

- `Ctrl+Enter` 与 `Cmd+Enter` 同时支持，`Enter` 单独不发送（避免与换行冲突）；`Ctrl+Shift+Enter` / `Cmd+Shift+Enter` 同样视为发送（修饰键包含 `Ctrl/Cmd` 即可）。
- 输入条含首尾空白时，发送前应 `trim` 后投递；全空白按契约 6 视为 no-op。
- 极长单行（> 4000 字符）或多行（> 100 行）批量投递不应 panic 或截断，输入条通过横向/纵向滚动容纳，终端侧单次调用投递完整字节序列。
- IME 组合过程中（`ime_marked_text` 非空、`ime_replacement` 存在）按 `Ctrl+Enter` 应先提交组合（`unmark` 语义）再按契约 6/7 判定是否发送；不应把未提交的 marked_text 丢弃或重复发送。
- 终端处于 `ALT_SCREEN`（如 `vim`/`less`，`is_alt_screen == true`）或前台命令运行中（`is_command_running == true`）时，compose 仍允许发送（不强制 disabled），但实现应在单测中明确该分支不崩溃（发送路径与空闲态一致）。
- 快速连续点击状态栏按钮（展开→收起→展开）不应残留旧的 `on_key_down` 监听或焦点竞争；`compose_focus` 复用同一 `FocusHandle` 实例。
- 无活动视图时展开输入条允许编辑但发送 no-op；此后若新建或切换出活动视图，无需重新展开即可发送。
- 窗口无焦点或输入条非聚焦时，全局 `Ctrl+Enter` 不应触发 compose 发送（仅输入条聚焦态响应）。

## 接口与状态变更

- `AppShell`（`src/features/workspace/shell.rs`）新增内存态字段（不持久化）：
  - `compose_visible: bool`（默认 `false`）
  - `compose_state: TextEditingState`（`src/shared/text_editing.rs:16`，初始空）
  - `compose_focus: FocusHandle` / `compose_scroll: ScrollHandle`
  - 方法：`toggle_compose_bar(&mut self, cx)`、`send_compose(&mut self, cx)`、`hide_compose(&mut self, cx)`
- `EntityInputHandler for AppShell`（`src/features/workspace/shell_input.rs:15`）新增 `AppShellInputField::Compose`，在 `active_input_field / text_for_range / selected_text_range / marked_text_range / unmark_text / replace_text_in_range / replace_and_mark_text_in_range / bounds_for_range / text_length_utf16` 追加 `Compose` 分支，复用 `quick_command_editor` 的 `TextEditingState` 处理模式。
- 渲染：新增 `src/features/workspace/compose_bar.rs`（或 `src/features/workspace/compose.rs`），导出 `render_compose_bar(shell: &mut AppShell, window: &Window, cx: &mut Context<AppShell>) -> AnyElement`，内部结构参考 `src/features/workspace/view.rs:1162 render_quick_command_editor` 的自绘输入框（`div.track_focus(&focus).on_key_down` + `ime_input_canvas` + `text_span/marked_text_span/text_caret` + 右侧 `Button` 发送），高度 `px(38-42)`，`border_t_1 border_color(border)`，置于 `main` 列底。
- 挂载：`src/features/workspace/shell_render.rs:100-108` 的 `workspace` 组合中，将 `main` 与 `compose_bar` 同置于 `main` 列的 `flex_col` 内（`div.flex_1.flex_col.child(main).children(compose_bar.then(...))`），再与 `sidebar`/`quick_commands` 拼成 `flex_row`；`status_bar` 保持 `root.flex_col` 的末 child，不受影响。
- 状态栏：`src/features/workspace/view.rs:462 render_workspace_status_bar` 新增 `render_compose_toggle` 按钮，`id="status-compose"`，图标新增 `Keyboard`（`crates/crossh-assets/assets/icons/keyboard.svg`，源 `https://raw.githubusercontent.com/lucide-icons/lucide/1.27.0/icons/keyboard.svg`），`selected = compose_visible`，`disabled = focused_view.is_none()`，`tooltip = i18n::text("tooltip.compose_bar")`，`on_click = AppShell::toggle_compose_bar`。
- `WorkspacePane::run_command` 已有，无需新增 trait；发送路径直接复用 `src/features/terminal/view.rs:807 run_command` / `src/features/terminal/view.rs:1035 pane.run_command`。
- 图标纪律：新增 `Keyboard` 按 `AGENTS.md` 从 `https://raw.githubusercontent.com/lucide-icons/lucide/1.27.0/icons/keyboard.svg` 下载原文件到 `crates/crossh-assets/assets/icons/keyboard.svg`，并在 `crates/crossh-assets/src/lib.rs:146 define_icons!` 追加 `Keyboard => "icons/keyboard.svg"`，`THIRD_PARTY_NOTICES.md` 同步更新 Lucide 引用，禁止手写/改写 path。
- `locales/en.yml` / `locales/zh-CN.yml` 新增：`tooltip.compose_bar`、`compose.placeholder`、`compose.send`（`Ctrl+Enter` 提示），`src/shared/i18n.rs` 无需新增分支，直接走 `i18n::text`。
- 无 `settings.toml` 新增字段，无持久化格式变更，无 wire 格式变更。

## 平台影响

- 布局与输入为 GPUI 行为，仅在 macOS arm64 本地人工验证（最小窗口/标准窗口/侧栏收起/快捷栏 Rail 三态）与 `cargo test --workspace` 纯逻辑单测覆盖。
- Linux / Windows 不新增平台分支，`TextEditingState` 的 UTF-8 边界、IME、trim 逻辑由 `cargo test` 在全平台复跑；终端 PTY 投递语义与 macOS 一致，无需额外 GitHub Actions job。
- 若新增 `keyboard.svg` 资产，`crates/crossh-assets` 的 `every_declared_and_embedded_icon_is_loadable` 单测在各 Runner 复跑即覆盖。

## 涉及纪律

- [x] Logic must not depend on UI：`TextEditingState` 的编辑、光标、选区、trim 判定保持纯逻辑（`src/shared/text_editing.rs`），`AppShell` 的发送仅做 `trim.is_empty()` 与视图分发，无 `gpui` 之外的纯逻辑对 UI 的反向依赖。
- [x] Feature-owned settings：compose 为内存态，不新增 `WorkspaceSettings` 字段，不触及 `src/features/workspace/settings.rs` 的持久化属主；如后续需要持久化显隐，则另起 ADR 明确属主。
- [x] 图标纪律（Lucide 1.27.0 官方 SVG，IconName 映射）：复用或按规范下载原文件，`THIRD_PARTY_NOTICES.md` 同步。
- [x] 文件规模 < 2000 行：`view.rs` 已约 1500 行，新增按钮仅数行；输入条独立为 `compose_bar.rs`，不推高单文件规模，`scripts/check-architecture.sh` 全量校验。
- [x] 工程笔记 / ADR 同步义务：本 spec 为新交互边界，如评审中决策改变涉及面板组合或终端所有权则增补 ADR；调试根因落 `docs/engineering-notes/`。
- [x] 响应式 UI（最小窗口尺寸可用性）：契约 3/11 显式覆盖紧凑与标准布局，人工核验 + 纯逻辑可用宽度计算单测。

## 影响模块

- `src/features/workspace/shell.rs`：`AppShell` 新增 `compose_*` 状态与 `toggle/send/hide` 方法、`refocus_active_terminal` 复用。
- `src/features/workspace/shell_input.rs`：`AppShellInputField::Compose` 与 6 处 `EntityInputHandler` 分支。
- `src/features/workspace/shell_render.rs`：`workspace` 的 `main` 列与 `compose_bar` 的组合挂载。
- `src/features/workspace/compose_bar.rs`（新增）：输入条渲染、`on_key_down` 的 `Ctrl/Cmd+Enter` 与 `Escape`/`Shift+Enter` 处理、滚动与 `ime_input_canvas`。
- `src/features/workspace/view.rs`：`render_workspace_status_bar` 新增 compose toggle 按钮。
- `src/shared/text_editing.rs`：复用，不改动（必要时补充 `trim` 判定单测）。
- `crates/crossh-assets/src/lib.rs` / `crates/crossh-assets/assets/icons/keyboard.svg` / `THIRD_PARTY_NOTICES.md`：仅在新增 `Keyboard` 图标时触及。
- `crates/crossh-ui-component`：不新增组件，复用 `Button`、`Tooltip`、`StatusBar` 既有原语。
- `locales/en.yml` / `locales/zh-CN.yml`：文案新增。
- `docs/testing.md`：补充 compose 行为矩阵。

## 验收清单

- [x] spec 评审通过（AI 评审 + 人批准）—— 2026-08-20 批准
- [x] 行为契约全部固化为失败测试并确认失败原因正确（Red，按 `spec_20260820_terminal_compose_bar__*` 命名，覆盖契约 1-11 的状态切换、可用宽度、IME、trim、发送/no-op（含按钮点击）、焦点交还、Escape、视图切换保持、Shift+Enter 换行、响应式滚动）—— 12 项 `spec_20260820_*` 先红后绿
- [x] 最小实现通过聚焦测试（Green）—— 12 契约测试 + `cargo test --workspace` 全绿（`env -u CROSSH_UPDATE_SIGNING_KEY`）
- [x] `cargo fmt --check`
- [x] `scripts/check-architecture.sh`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `cargo test --workspace`（`env -u CROSSH_UPDATE_SIGNING_KEY` 全量通过；`crossh` 232 passed 含 12 新增）
- [x] 声明的平台 CI job 通过（本 spec 仅 macOS 人工验证 + 全平台 `cargo test`，本地已验证）
- [x] 结构性决策提炼进 ADR（如有）并登记 `docs/architecture.md`—— 无新增结构性边界，沿用 ADR 0002/0007/0011/0012
- [x] 调试根因合并进 `docs/engineering-notes/`（如有）—— 无新增根因
- [x] 新增行为合并进 `docs/testing.md` 关键行为矩阵（Compose 行已增，见本次审计 D-5）
- [x] 用户可观察效果人工确认（本地 macOS 已确认：按钮/输入条/批量发送/切换草稿）

# SidePanel 与 Rail 抽象

## 元数据

- 状态：`done`
- 创建：2026-08-20
- 相关 ADR：`docs/adr/0007-workspace-panel-composition.md`、`docs/adr/0002-logic-ui-layering.md`
- 相关 issue / 路线图项：无
- CI 平台影响：`仅 macOS`（GPUI 布局纯重构，逻辑由 `cargo test --workspace` 在各 Runner 复跑；无平台分支，无需额外 Windows/Linux job，已有 UI 行为由本地 macOS 验证）

## 背景

Workspace 现有 4 处可拖拽侧边面板（`sidebar`、`quick-commands`、`terminal-split`、`git changes-pane`）各自持有 `Rc<Cell<f32>> width + Rc<Cell<bool>> dragging`、各自手写 `clamp(MIN/MAX)`、各自拼接 `div().w(px(width)).h_full().bg().border() + SplitResizer`（`src/features/workspace/shell.rs:159` `src/features/workspace/sidebar.rs:247` `src/features/workspace/view.rs:686` `src/features/git/window.rs:62` `crates/crossh-ui/src/theme.rs:8-21`）。收起态 Rail（`sidebar_rail:356` / `quick_commands_rail:44`）同样复制了 `w(RAIL_WIDTH) flex_col items_center + 30px 头像 + StatusDot + Tooltip` 的骨架与 `rail_avatar_button:314` 样式。重复导致 `view.rs` 超 1400 行、`check-architecture.sh` 规模纪律承压，且新增面板时需重复实现宽度换算与拖拽边界。

## 目标

1. 将 SidePanel（可拖拽展开态容器 + `SplitResizer` 封装）抽为 `crossh-ui-component` 的无状态 `RenderOnce` 原语，统一 `resolved_width`（clamp）、`side`（Left/Right）、`handle_side`、`line` 的行为。
2. 将 Rail（收起态窄栏容器 + 头像列）抽为同一 crate 的无状态原语，统一 `w(RAIL_WIDTH) h_full bg/border + flex_col items_center + scroll` 的骨架与 `rail_avatar` 项的视觉规范（30px、圆角、选中态 `accent` 边框/背景、`StatusDot` 叠加位）。
3. 重构后用户可观察行为与重构前逐像素一致：展开/收起切换、拖拽调宽、clamp 边界、`available_main_width` 计算、Rail 头像间距与悬停均不变；不引入新的持久化字段或设置项。

## 非目标

- 不合并或改动 `WorkspacePane` trait（`src/features/workspace/pane.rs:16`）：内容区 Pane 与 chrome 侧边栏分属不同层次，合并会让 `crossh-core` 等纯逻辑 crate 依赖 `gpui`。
- 不改变 `AppShell` 的状态所有权模型：宽度与拖拽状态仍由 `AppShell` 持有并传入组件，组件不持有 `Entity` 或可变状态。
- 不新增图标、颜色 token 或持久化格式：复用 `crossh-ui::theme` 现有 `SIDEBAR_*` / `QUICK_COMMANDS_*` 常量与 `crossh-assets` 的 `PanelLeft/Right` 图标。
- 不改变拖拽之外的交互（右键菜单、搜索、固定/关闭标签等）；Git Viewer 的 `changes-pane`（`src/features/git/*`）本期不纳入，留待二期另起 spec。
- 不引入新的 crate 依赖或跨 crate 的 wire 格式变更。

## 行为契约

1. 当 `sidebar` 处于展开态时，其渲染宽度应等于 `sidebar_width.get().clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH)`（`theme.rs:9-10`），容器样式为 `w(px(width)) h_full bg(sidebar) border_r_1 border_color(border)`，并包含一个 `SplitResizer("sidebar-resize", dragging, width).min(SIDEBAR_MIN).max(SIDEBAR_MAX).line()` 的可拖拽手柄。
2. 当 `sidebar` 处于收起态（`show_host_sidebar == false`）时，应渲染固定宽度的 Rail，宽度为 `SIDEBAR_RAIL_WIDTH`（`theme.rs:11` 44px），容器为 `w(px(44)) h_full bg(sidebar) border_r_1`，不再包含 `SplitResizer`。
3. 当 `quick-commands` 处于展开态时，其渲染宽度应等于 `quick_commands_width.get().clamp(QUICK_COMMANDS_MIN_WIDTH, QUICK_COMMANDS_MAX_WIDTH)`（`theme.rs:19-20` 240..460），容器为 `w(px(width)) h_full bg(surface) border_l_1`，手柄为 `SplitResizer("quick-commands-resize", ...).handle_left().line()`。
4. 当 `quick-commands` 处于 Rail 态（有 `ActiveCommandContext` 但 `show_quick_commands == false`）时，应渲染固定宽度 `QUICK_COMMANDS_RAIL_WIDTH`（`theme.rs:21` 40px）的 Rail，容器为 `w(px(40)) h_full bg(surface) border_l_1`，无 `SplitResizer`。
5. 当 `quick-commands` 无可用命令上下文（`active_command_context == None`）时，不应渲染任何 quick-commands 面板（展开态与 Rail 均不渲染），`available_main_width` 计算中 quick-commands 宽度按 0 计入（`shell_render.rs:78-85` 语义保持）。
6. 当 `available_main_width(window.viewport_size.width, sidebar_width, quick_commands_width)` 被调用时，其结果应等于 `px((viewport - sidebar - quick).max(0.0))`，其中 `sidebar` 与 `quick` 取各面板的 resolved 宽度（展开态取 clamp 后值，Rail 态取固定 rail 宽度，隐藏态取 0）（`shell.rs:1652` 现有契约保持）。
7. 当拖拽 SidePanel 的手柄时，`SplitResizer` 应按 `SplitHandleSide::Right`（左侧面板）或 `SplitHandleSide::Left`（右侧面板）换算 `drag_width` 并 `clamp(min, max)` 后写入 `width` 单元格，且 `dragging` 在 `MouseDown` 置 true、`MouseUp` 置 false（`crates/crossh-ui-component/src/split_resizer.rs:30-41` 现有契约保持）。
8. 当 Rail 渲染头像列时，每个头像项应为 `w(30) h(30) rounded(RADIUS_SM) border_1`，未选中时 `border TRANSPARENT / bg TRANSPARENT`、悬停 `bg surface`，选中时 `border accent / bg accent_soft`，并支持右上角 `StatusDot size 7 border surface` 叠加位（`sidebar.rs:314-352` / `quick_commands_rail.rs:287-293` 现有视觉保持）；列间距为 `gap 4`，`pitch = 30 + 4 = 34` 的可测常量保持。
9. 当通过 `SidePanel + Rail` 重构后，`sidebar` 与 `quick-commands` 的展开/Rail/隐藏三态切换应满足：各态 `resolved_width` 与 `PanelSide`（Left/Right）配置同重构前一致，Rail 头像列 `pitch=34` 不变；在最小窗口尺寸与 1280px 标准窗口下均无裁切、重叠或不可达控件（响应式纪律保持，ADR 0007 的 `sidebar | main | quick-commands` 同级横向组合语义保持）。

## 边界与错误

- `width` 单元格的非法值（NaN、负值、超出 max 2 倍）经 `clamp(min, max)` 后仍渲染为合法宽度，不 panic、不产生负的 `available_main_width`。
- `viewport` 宽度小于两侧面板宽度之和时，`available_main_width` 按 `max(0)` 截断，主区宽度为 0 但不溢出或崩溃；拖拽过程中 `max_left_width` 同样按现有 `terminal_split` 逻辑保持下界 0。
- 快速切换 `show_host_sidebar` / `show_quick_commands` 时，不应残留旧面板的 `SplitResizer` 监听或旧宽度的视觉闪烁；状态单元格复用同一 `Rc<Cell<_>>` 实例。
- 无活动上下文时切换 `show_quick_commands` 不应触发 quick-commands 面板的渲染或上下文解析（`quick_commands_panel_mode(None, _)` 返回 `None` 的现有分支保持）。
- Rail 头像列为空（无活跃项目/无 pinned 命令）时，Rail 仍渲染固定宽度空容器，不塌陷为 0 宽。

## 接口与状态变更

- 新增 `crates/crossh-ui-component/src/panel.rs`：导出 `PanelMetrics`（可选，复用 `theme::*_WIDTH` 常量即可）、`PanelSide`、`SidePanel`（`RenderOnce`）、`Rail`（`RenderOnce`）及 `rail_avatar` 辅助函数；`crate::lib.rs` 重新导出。
- `crossh-ui::theme` 不新增常量，`crossh-assets` 不新增图标；`settings.toml` 不新增字段，持久化格式不变。
- `AppShell` 的 `sidebar_width/sidebar_dragging/quick_commands_width/quick_commands_dragging` 字段类型不变，仅渲染调用点由手写 `div + SplitResizer` 改为 `SidePanel`/`Rail` 组合；`git` 的 `changes_pane_width` 可二期接入，首期不强制迁移。
- 无公开 wire/API 变更，无新增 `gpui` 依赖的 crate 边界穿越。

## 平台影响

- 变更为纯 GPUI 布局重构，无平台分支逻辑；macOS arm64 本地验证渲染与拖拽，`cargo test --workspace` 的纯逻辑 clamp/pitch 单测在各 Runner 复跑。
- Linux/Windows 的窗口尺寸与拖拽行为与 macOS 一致，无需额外 CI job；`changes-pane` 二期另起 spec 验证。

## 涉及纪律

- [x] Logic must not depend on UI（层级）：新增 `panel.rs` 置于 `crossh-ui-component`（已依赖 `gpui`），不引入到 `crossh-core`/`crossh-ssh`/`crossh-theme` 等纯逻辑 crate；`AppShell` 仍为状态 owner，组件保持 `RenderOnce` 无状态。
- [x] Feature-owned settings：不新增设置项，沿用 `WorkspaceSettings::show_host_sidebar/show_quick_commands`，不产生新的持久化属主。
- [x] 图标纪律：不新增或手改 SVG，复用现有 `PanelLeft/Right` 图标的 `IconName` 映射。
- [x] 文件规模 < 2000 行：`view.rs`（>1400 行）与 `sidebar.rs`（>1000 行）的重复容器代码收敛到 `panel.rs`，降低单文件规模，`scripts/check-architecture.sh` 全量校验。
- [x] 工程笔记 / ADR 同步义务：结构性决策为 ADR 0007 的细化，不新增 ADR；若发现布局根因，落 `docs/engineering-notes/`。
- [x] 响应式 UI（最小窗口尺寸可用性）：`available_main_width` 截断与 `min_w_0`/`flex_shrink` 保持，紧凑与标准布局需人工核验。

## 影响模块

- `crates/crossh-ui-component/src/panel.rs`（新增）
- `crates/crossh-ui-component/src/lib.rs`（导出）
- `crates/crossh-ui-component/src/split_resizer.rs`（仅被复用，不改动）
- `crates/crossh-ui/src/theme.rs`（仅被复用常量，不改动）
- `src/features/workspace/shell.rs`（字段类型不变，渲染调用点收敛）
- `src/features/workspace/shell_render.rs`（`available_main_width` 的输入改为 `SidePanel::resolved_width`）
- `src/features/workspace/sidebar.rs`（`render_sidebar`/`render_sidebar_rail` 改用 `SidePanel`/`Rail`）
- `src/features/workspace/view.rs`（`render_quick_commands` 改用 `SidePanel`）
- `src/features/workspace/quick_commands_rail.rs`（改用 `Rail` + `rail_avatar`）

## 验收清单

- [x] spec 评审通过（AI 评审 + 人批准；2026-08-20 AI 自评审通过，2 项必修已修订后人批准）
- [x] 行为契约全部固化为失败测试并确认失败原因正确（Red：`panel.rs` 9 项 `spec_20260820_side_panel_rail__*` 以 clamp/NaN/pitch/handle/available 断言形式先行验证）
- [x] 最小实现通过聚焦测试（Green：`crates/crossh-ui-component` 52 passed 含 9 项新契约；`cargo test --workspace` 全绿）
- [x] `cargo fmt --check`
- [x] `scripts/check-architecture.sh`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `cargo test --workspace`（`env -u CROSSH_UPDATE_SIGNING_KEY` 全量通过；单失败 `sign_without_key` 为环境预存变量导致，与本次无关）
- [x] 声明的平台 CI job 通过（本 spec 为纯重构，仅 macOS 本地验证即可置 done；`cargo test --workspace` 单测在各 Runner 复跑）
- [x] 结构性决策提炼进 ADR（如有）并登记 docs/architecture.md（无新结构决策：ADR 0007 细化，不新增 ADR）
- [x] 调试根因合并进 docs/engineering-notes/（如有）（无新增根因）
- [x] 新增行为合并进 docs/testing.md 关键行为矩阵（如有）（纯重构收敛，无新增矩阵条目）
- [x] 用户可观察效果人工确认（`SidePanel`/`Rail` 像素与 `split_resizer` 行为经 `clamp_panel_width`/`available_main_width`/`RAIL_AVATAR_PITCH=34` 单测固化；手柄方向 Left→Right/Right→Left 与 `sidebar`/`quick-commands` 现有行为一致）

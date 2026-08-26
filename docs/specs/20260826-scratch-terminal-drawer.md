# 底部抽屉式临时终端（Scratch Terminal Drawer）

## 元数据

- 状态：`draft`
- 创建：2026-08-26
- 相关 ADR：`docs/adr/0002-logic-ui-layering.md`（逻辑零 gpui）、`docs/adr/0007-workspace-panel-composition.md`（面板组合）、`docs/adr/0011-terminal-split-ownership.md`（分栏属主独立）
- 相关 issue / 路线图项：无
- CI 平台影响：`仅 macOS`（纯 GPUI 布局 + 本地 PTY，逻辑由 `cargo test` 覆盖；终端行为仅 macOS arm64 验证）

## 背景

用户在主终端工作时，经常需要一个临时本地 shell 查看系统信息（`htop/df`）、升级依赖（`brew upgrade / cargo update`）等。这类任务生命周期短、不属于任何项目，若用现有 `LocalSession`（`project_dir + recent_dirs + TabStrip`）会污染侧栏计数、最近项目和标签条。需要一个不进项目模型、随手呼出/收起、位于主区底部的临时终端抽屉。

## 目标

1. 状态栏新增一个 Scratch 开关，点击或快捷键可在主区底部（`sidebar` 与 `quick_commands` 之间、`status_bar` 之上）展开/收起一条临时终端抽屉。
2. 抽屉内是单个本地 `TerminalView`（Zed PTY），临时任务完成后可一键收起，主视图与侧栏完全不受影响。
3. 抽屉的显隐与高度为窗口内存态，不污染 `local_dirs / recent_dirs / pinned_local_tabs`，关闭即不留痕。
4. 布局在最小窗口下仍可用，高度可拖拽且有明确上下界。

## 非目标

- 不做多实例 Scratch（首版单例，复用同一 PTY）。
- 不做持久化（重启后不恢复，草稿与高度不写 `settings.toml`）。
- 不做远程 SSH Scratch（仅本地 shell）。
- 不做与 `CommandHistory / quick_commands` 的历史联动。
- 不新增通用 `crossh-ui-component` 组件；抽屉为 workspace 私有视图。
- 不改变现有 `LocalSession`、`TerminalSplit`、`ComposeBar` 的持久化与属主语义。

## 行为契约

1. 当 Scratch 处于收起态时，主区不应渲染抽屉，状态栏 Scratch 按钮为未选中态；`available_main_width` 计算不变。
2. 当用户点击状态栏 Scratch 按钮或触发快捷键 `cmd+``（macOS）/`ctrl+``（非 macOS）时，应该切换 Scratch 显隐，观察到按钮 `selected` 与抽屉可见性一致；从收起变为展开时，若尚无 Scratch 终端则新建一个本地终端，焦点进入该终端。
3. 当 Scratch 展开时，应该渲染在 `main` 列内部、`status_bar` 之上，左右边界与 `sidebar` 右沿与 `quick_commands` 左沿对齐，宽度恒等于 `available_main_width`，不挤压侧栏与快捷栏；高度在 `[120, 400]`（默认 `220`）范围内，拖拽条可调整，超出边界被 clamp。
4. 当 Scratch 终端存在时，其 `cwd` 初始为当前活动 `LocalSession` 的 `cwd`（若无活动会话则为 `HOME`），且创建过程不应写入 `recent_dirs / local_dirs / pinned_local_tabs`，观察到侧栏项目计数与 `recent_dirs` 长度不变。
5. 当 Scratch 处于展开态再次被隐藏（按钮/快捷键/`Esc`），应该仅隐藏抽屉，不销毁终端，观察到再次展开时终端内容与运行中的命令保持不变（PTY 复用）。
6. 当 Scratch 终端触发 `TerminalEvent::Closed`（PTY 退出）时，应该清空 Scratch 终端并自动收起抽屉，观察到 `scratch_terminal.is_none() && scratch_visible == false`。
7. 当窗口收缩到最小声明尺寸时，抽屉应保持完整可用，观察到不超出视口、不遮挡侧栏、无横向溢出，内部终端可横向/纵向滚动。
8. 当 Scratch 展开时按 `Esc`，应该收起抽屉并将焦点交还之前的活动终端（若存在），观察到 `scratch_visible == false` 且 `focused_view` 恢复。
9. 当 Scratch 展开且无任何活动视图时，仍应可正常展开/编辑/收起，观察到不 panic，显隐切换与高度拖拽正常。
10. 当 Scratch 可见时，`TabStrip` 与侧栏的会话计数不应包含 Scratch，观察到 `local_sessions.len()` 与 `remote_tabs.len()` 不变。

## 边界与错误

- 快速连续切换（展开→收起→展开）不应创建多个 PTY，仅复用同一 `Entity<TerminalView>`；重复创建被去重。
- 高度拖拽值 `<=0` 视为哨兵，回退到默认高度；`SplitResizer` 的 `min/max` clamp 保证不越界。
- 终端创建失败（`TerminalBuilder` 错误）应显示错误态而非 panic，抽屉仍可关闭。
- 应用退出时 Scratch 终端随窗口销毁，无泄漏日志。
- `Esc` 在 Scratch 输入中被终端消费（如 `vim`）时，优先由终端处理，不强制收起。

## 接口与状态变更

- `AppShell` 新增内存态字段（不持久化）：
  - `scratch_visible: bool`（默认 `false`）
  - `scratch_terminal: Option<Entity<TerminalView>>`
  - `scratch_height: Rc<Cell<f32>>`（默认 `220.`，哨兵 `0.` 回退默认）
  - `scratch_dragging: Rc<Cell<bool>>`
  - `scratch_subscription: Option<Subscription>`
  - 方法：`toggle_scratch_terminal(&mut self, &mut Window, &mut Context<Self>)`、`hide_scratch_terminal(&mut self, &mut Context<Self>)`
- 新增 `src/features/workspace/scratch.rs`（状态与交互）与 `src/features/workspace/scratch_bar.rs`（渲染），复用 `src/features/workspace/compose_bar.rs` 的布局模式与 `SplitResizer`。
- 状态栏：`src/features/workspace/view.rs:render_workspace_status_bar` 新增 Scratch toggle 按钮，`id="status-scratch"`，图标 `Terminal`，`selected = scratch_visible`，`tooltip = "tooltip.scratch_terminal"`。
- 快捷键：`src/features/terminal/mod.rs` 或 `src/features/workspace/shell.rs` 绑定 `cmd+`` / `ctrl+`` 到 `ToggleScratchTerminal`（`AppShell` key_context）。
- `locales/en.yml` / `locales/zh-CN.yml` 新增 `tooltip.scratch_terminal`。
- 无 `settings.toml` 新增字段，无持久化格式变更。

## 平台影响

- 布局与终端为 GPUI 行为，仅 macOS arm64 本地人工验证（最小窗口/标准窗口/侧栏收起/快捷栏 Rail 三态）与 `cargo test --workspace` 覆盖。
- Linux / Windows 无新增分支，纯逻辑由 `cargo test` 复跑；PTY 行为与 macOS 一致，无需额外 CI job。

## 涉及纪律

- [x] Logic must not depend on UI —— Scratch 显隐与高度为 UI 态，不渗入 `crossh-core`。
- [x] Feature-owned settings —— 无持久化设置，状态为 `AppShell` 内存态。
- [x] 图标纪律（Lucide 1.27.0）—— 复用现有 `Terminal` 图标，不新增资产。
- [x] 文件规模 < 2000 行 —— 独立 `scratch.rs / scratch_bar.rs`，不推高 `shell.rs / view.rs`。
- [x] 工程笔记 / ADR 同步义务 —— 无新增结构性边界，沿用 ADR 0002/0007/0011。
- [x] 响应式 UI —— 契约 3/7 显式覆盖紧凑与标准布局。

## 影响模块

- `src/features/workspace/scratch.rs`（新增，交互逻辑）
- `src/features/workspace/scratch_bar.rs`（新增，渲染与拖拽）
- `src/features/workspace/shell.rs`（`AppShell` 字段与方法）
- `src/features/workspace/shell_render.rs`（`main` 列与抽屉组合挂载）
- `src/features/workspace/view.rs`（状态栏按钮）
- `src/features/workspace/mod.rs`（模块导出）
- `locales/en.yml` / `locales/zh-CN.yml`（文案）

## 验收清单

- [ ] spec 评审通过（AI 评审 + 人批准）
- [ ] 行为契约全部固化为失败测试并确认失败原因正确（Red）
- [ ] 最小实现通过聚焦测试（Green）
- [ ] `cargo fmt --check`
- [ ] `scripts/check-architecture.sh`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] 声明的平台 CI job 通过
- [ ] 结构性决策提炼进 ADR（如有）并登记 `docs/architecture.md`
- [ ] 调试根因合并进 `docs/engineering-notes/`（如有）
- [ ] 新增行为合并进 `docs/testing.md` 关键行为矩阵（如有）
- [ ] 用户可观察效果人工确认（抽屉推拉、高度拖拽、PTY 复用、最小窗口）

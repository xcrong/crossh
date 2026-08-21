# 终端分栏宽度按属主独立记忆

> 每个分栏（属主 Tab）拥有自己的左窗格宽度，互不共享；关闭分栏即丢弃其宽度。

## 元数据

- 状态：`approved` → `in-progress`（2026-08-21 用户在对话中批准）
- 创建：2026-08-21
- 相关 ADR：`0011`（分栏与属主 Tab 绑定、多分栏独立共存）
- 相关 issue / 路线图项：本轮对话「终端分栏宽度是全局共享的，改为各分栏独立」
- CI 平台影响：`无（纯逻辑）` — 布局状态为纯 Rust 逻辑，仅 macOS 本地验证

## 背景

当前所有终端分栏共用 AppShell 顶层的一个 `terminal_split_width: Rc<Cell<f32>>`
（`src/features/workspace/shell.rs`）。用户在 Tab A 把分栏拖成 1:2 后切到
Tab B，Tab B 的分栏会沿用这个宽度；关掉全部分栏再重开又回到均分。而分栏本身
已经是按属主 Tab 独立共存的（ADR 0011，`WorkspaceState.terminal_splits`），
宽度却落在全局单值上，与「每个 Tab 的分栏是独立临时布局」的既有语义不一致。

## 目标

1. 每个属主 Tab 的分栏各自记忆自己的左窗格宽度，切换 Tab 时互不影响。
2. 新建分栏仍从左右均分开始（保持现状默认）。
3. 关闭某个分栏后，它的宽度偏好随之丢弃；同 Tab 再开分栏回到均分。
4. 远程 Tab 索引重映射、批量清扫等既有生命周期路径正确携带/清理宽度状态。

## 非目标

- 不改变 `SplitResizer` 通用组件的 API 与行为（仍是调用方持有单元格的无状态组件）。
- 不把宽度持久化到磁盘设置（分栏是临时布局，重启不恢复，与现状一致）。
- 不改 clamp 规则（min 160px、手柄 8px、可用性门槛 328px 均保持不变）。
- 不引入比例存储——继续存绝对像素值，窗口 resize 时仍按现有 clamp 收敛。

## 行为契约

命名前缀：`spec_20260821_split_width__`

1. 当属主 A 的分栏宽度被设为 W₁、属主 B 的分栏宽度被设为 W₂（W₁ ≠ W₂）时，
   应能分别通过各自的宽度查询读到 W₁ 和 W₂，观察到两个分栏宽度互不覆盖。
2. 当读取一个从未拖拽过的分栏的宽度时，应得到哨兵值 `0.0`，观察到渲染层
   以均分作为默认值（与现状语义一致）。
3. 当创建分栏成功时，应自动为其分配独立的宽度槽位（创建失败/早退不分配），
   观察到无需渲染层介入即可读到该分栏自己的宽度单元格。
4. 当某分栏被拆除（关闭属主/右窗格、批量清扫命中）时，应同时移除其宽度
   槽位，观察到之后重新查询不到该属主的宽度条目。
5. 当远程 Tab 索引重映射发生且某分栏属主索引前移时，其宽度槽位应跟随迁移；
   属主被删除的分栏的宽度槽位应被清除，观察到 key 与 `terminal_splits` 一致。
6. 当所有分栏清空后，宽度槽位集合与 `terminal_splits` 同步为空、不留孤儿
   条目（拖拽标志复位是实现细节，不在纯测试契约内）。

## 边界与错误

- 宽度槽位的生命周期严格跟随 `terminal_splits` 的增删路径，任何一条拆分栏
  路径漏清理都会造成孤儿条目——契约 4/5/6 分别覆盖三条路径。
- 渲染层查不到活动分栏时不读宽度；查不到宽度槽位（理论不可达，防御）时
  回退哨兵值 `0.0` 走均分，不得 panic。
- 拖拽进行中切换 Tab：拖拽标志是全局单值（同一时刻只有一个活动分栏在渲染
  手柄），保持现状即可，不在本 spec 范围内改造。

## 接口与状态变更

- `WorkspaceState` 新增 `split_widths: BTreeMap<ActiveView, Rc<Cell<f32>>>`
  （与既有 `compose` 平行的 per-view map，纯逻辑层，无 gpui 依赖）。
- `AppShell.terminal_split_width` 单值字段删除；`terminal_split_dragging` 保留。
- 无持久化格式、wire 格式、设置项变化。

## 平台影响

- 无平台差异：布局状态为平台无关纯逻辑。本地跑全部测试即可，无点名 CI job。

## 涉及纪律

- [x] Logic must not depend on UI — 宽度槽位放在 registry（逻辑层），仅用
      `std::rc::Rc` + `std::cell::Cell`，不引入 gpui 类型。
- [ ] Feature-owned settings — 不涉及新设置。
- [ ] 图标纪律 — 不涉及图标。
- [x] 文件规模 < 2000 行 — 改动均为小增量，不触碰红线文件。
- [ ] 工程笔记 / ADR 同步义务 — 无结构性决策变化（ADR 0011 语义的自然延伸）。
- [x] 响应式 UI — clamp 与最小窗格宽度规则原样保留，紧凑布局行为不变。

## 影响模块

- `src/features/workspace/registry.rs` — `split_widths` 槽位表及其增删/重映射。
- `src/features/workspace/shell.rs` — 删除全局 `terminal_split_width` 字段。
- `src/features/workspace/view.rs` — 渲染与 resizer 接线改读活动属主的槽位。
- `src/features/workspace/split.rs` — `reset_split_ui_if_idle` 适配。

## 验收清单

- [x] spec 评审通过（AI 评审 + 人批准）
- [x] 行为契约全部固化为失败测试并确认失败原因正确（Red）
- [x] 最小实现通过聚焦测试（Green）
- [x] `cargo fmt --check`
- [x] `scripts/check-architecture.sh`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `cargo test --workspace`（注：`crossh-update` 的
      `sign_without_key` 测试在本机因环境变量 `CROSSH_UPDATE_SIGNING_KEY`
      已设置而失败，属预存环境问题；unset 后全绿，与本次改动无关）
- [x] 声明的平台 CI job 通过（本 spec 无非本机平台 job）
- [ ] 结构性决策提炼进 ADR（如有）并登记 docs/architecture.md —— 无，ADR 0011 语义的自然延伸
- [ ] 调试根因合并进 docs/engineering-notes/（如有）—— 无
- [ ] 新增行为合并进 docs/testing.md 关键行为矩阵（如有）—— 无
- [ ] 用户可观察效果人工确认（针对 UI/交互变更）

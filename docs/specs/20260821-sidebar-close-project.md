# 侧栏项目一键关闭（Stop Project）

## 元数据

- 状态：`in-progress`
- 创建：2026-08-21
- 批准：2026-08-21（人批准，AI 自审通过，已修正 Square 图标缺失）
- 相关 ADR：`docs/adr/0002-logic-ui-layering.md`、`docs/adr/0007-workspace-panel-composition.md`、`docs/adr/0012-spec-driven-development-loop.md`
- 相关 issue / 路线图项：无
- CI 平台影响：`仅 macOS`（纯 GPUI 侧栏 + WorkspaceState 内存态，无平台分支）

## 背景

当前侧栏项目行右侧对活跃项目（`count>0`）仅提供 `+` 新建终端，无关闭入口；仅空记录（`count==0`）提供 `×` 从最近记录移除。用户无法一键释放某个项目的全部本地会话内存，需逐个关闭 Tab 或退出整个应用。与“完全关闭软件再打开该项目”相比，缺少一个轻量的“暂停项目、可恢复”操作。固定标签（pinned tab）的语义是持久化的，重启后 `activate_local_dir` 会幂等恢复并派发 `default_command`，该语义应同样适用于“关闭项目后再打开”。

## 目标

1. 为 `count>0` 的活跃项目在侧栏提供一键“停止项目”按钮，点击后关闭该项目下的全部本地会话，释放对应终端与后台任务，但保留项目在 `recent_dirs` 与固定标签记录中。
2. 关闭后的项目变为 `count==0` 空记录态，可再次点击整行或 `+` 按“重启后重建”同一路径恢复：`restore_pinned_tabs_for_project` 重建全部 pinned tab 并派发各自 `default_command`，无 pinned 时创建单个普通会话。
3. 与 `count==0` 的 `× forget` 视觉与语义明确区分，不误删记忆。

## 非目标

- 不改变 `×` 的 `forget_local_dir` 语义（从 `recent_dirs` 移除记忆，空项目从侧栏消失）。
- 不为关闭项目新增持久化字段或 wire 格式；复用现有 `WorkspaceSettings::recent_dirs` 与 `pinned_local_tabs`。
- 不改动标签条单会话的 `Close/Close Others` 逻辑与确认流程。
- 不处理远程主机（`remote_tabs`）的批量关闭；本期仅本地项目。
- 本期不引入二次确认以外的批量确认 UI 定制。

## 行为契约

1a. 当 `dir.sessions.is_empty() == false`（`count>0`）时，侧栏项目行右侧应在 `count` 文本与 `+` 按钮之间渲染一个“停止项目”按钮（`IconName::Square`，`size 14`，`id="local-stop-{idx}"`，`tooltip=tooltip.stop_project`），同时不渲染 `× forget` 按钮。
1b. 当 `count==0` 时，侧栏项目行右侧应仅渲染 `×`（`forget`）不渲染 `■`。
2. 当用户点击活跃行的 `■` 时，应该关闭该 `project_dir` 下的全部 `LocalSession`，观察到 `workspace.sessions.local_sessions` 中该目录的会话数归零、`local_dirs[project_dir].sessions` 为空、对应的 `Connection` 资源与后台任务已停止；`recent_dirs` 仍包含该 `project_dir` 且 `pinned_local_tabs` 中该目录的固定记录保持不变（不被 `retain` 清除）。
3. 当 `■` 关闭导致原活跃视图属于被关闭项目时，`workspace.active_view` 应按现有 `close_local_session` 的焦点回退策略转移（`dir.active_session → first_local_view() → remote_tabs.last()`），并 `refocus_active_terminal`，不产生空悬的 `active_view`。
4. 当项目已通过 `■` 变为空记录后，再次点击该项目行（`activate_local_dir`）时，应该按“重启后打开”同一路径重建：若 `pinned_local_tabs` 中存在该 `project_dir` 的记录则 `restore_pinned_tabs_for_project` 幂等地重建全部固定标签（含 `custom_name/default_command`，并按现有逻辑延迟派发 `default_command`），否则创建一个普通 `open_local_session`；重建后 `count` 恢复为 pinned 数量或 1。
5. 当 `■` 触发的批量关闭中任一会话满足 `local_session_close_risk / is_command_running == true` 时，应该对该会话走现有单会话关闭确认路径（`request_close_local_session` 的弹框语义），用户取消则该会话保留（`local_sessions`/`local_dirs`/`pinned` 均不变），其余可关闭会话按契约 2 继续关闭。
6. 当用户在活跃项目行右键时，上下文菜单应在现有两项（`打开本地终端/在 Finder 中显示`）之后追加 `Separator + 停止项目` 项（`id="stop-project"`，`label=context_menu.stop_project`，`action=ShellMenuAction::StopLocalProject(project_dir)`），点击效果与 `■` 完全一致；空记录的右键保持不变（`忘记目录`）。

## 边界与错误

- 批量关闭应聚合风险判定：有风险会话逐个经 `request_close_local_session` 确认，取消的会话保留，其余已关闭会话的回收与 `active_view` 回退不受影响。
- 若 `project_dir` 在关闭瞬间已被 `normalize_local_cwd` 判定为不存在或已被 `forget`，`StopLocalProject` 应为 no-op，不 panic。
- 快速连续点击 `■`（或 `■` 与 `+` 竞争）不应产生重复关闭或会话 ID 漂移；实现应快照 `project_dir` 对应的 `Vec<LocalSessionId>` 再迭代，避免二次 `get` 漂移。
- 关闭过程中若 `split` 涉及被关闭视图，应复用现有 `detach_splits_for` 避免分栏状态错乱。
- `count==0` 的行点击 `×` 后项目从 `recent_dirs` 移除，若该项目已被停止过，`pinned` 记录在首次真正的单会话 `close` 时已被保留，`forget` 不额外清理 `pinned`（与现状一致，`prune_missing_pinned_tabs` 仅在目录丢失时清理；本期有意保留孤儿 pinned，后续清理另起 spec）。
- 不改变 `prune_missing_pinned_tabs` 触发时机与 `persist_settings` 回写点。

## 接口与状态变更

- `AppShell` 新增方法 `stop_local_project(project_dir: PathBuf, cx)`（或 `close_local_project`），语义为“关闭项目全部会话但保留 pinned”；内部抽取 `close_local_session_internal(session_id, keep_pinned: bool)` 供复用，`stop` 路径以 `keep_pinned=true` 逐个经 `request_close_local_session` 风险确认后关闭，未通过确认的会话保留。
- `ShellMenuAction` 新增 `StopLocalProject(PathBuf)`。
- `src/features/workspace/sidebar.rs` 的 `render_local_dir` 按 `count` 条件分支渲染 `■` vs `×`，并新增 `■` 的 `on_click -> stop_local_project`；右键菜单同分支追加 `StopLocalProject`。
- 图标：新增 `Square`（`https://raw.githubusercontent.com/lucide-icons/lucide/1.27.0/icons/square.svg`）到 `crates/crossh-assets/assets/icons/square.svg`，并在 `crates/crossh-assets/src/lib.rs: define_icons!` 追加 `Square => "icons/square.svg"`，`THIRD_PARTY_NOTICES.md` 沿用 1.27.0 声明。
- `locales/en.yml`/`zh-CN.yml` 新增 `tooltip.stop_project` 与 `context_menu.stop_project`。
- 无 `settings.toml` 新增字段，无持久化格式变更。

## 平台影响

- 仅 GPUI 侧栏与 `WorkspaceState` 纯逻辑，`cargo test` 覆盖全部契约；macOS arm64 本地人工验证侧栏三态（`0/1/N`）与焦点回退，无新增 Linux/Windows 分支，复用现有 `cargo test --workspace` 在各 Runner 的覆盖。

## 涉及纪律

- [x] Logic must not depend on UI：批量关闭的“保留 pinned”判定为纯逻辑（`WorkspaceState`/`SessionRegistry`），不反向依赖 `gpui`。
- [x] Feature-owned settings：不新增设置项，复用 `recent_dirs` 与 `pinned_local_tabs` 的现有属主与持久化路径。
- [x] 图标纪律（Lucide 1.27.0 官方 SVG，IconName 映射）：复用现有 `Square`（`square.svg` 已在 `crates/crossh-assets/assets/icons/` 与 `define_icons!` 中声明），不新增资产，若需新图标则按规范下载原文件。
- [x] 文件规模 < 2000 行：改动分散在 `sidebar.rs` 数行、`shell.rs` 抽取方法、`tabs.rs` 复用，不推高单文件规模，需 `scripts/check-architecture.sh` 校验。
- [x] 工程笔记 / ADR 同步义务：如关闭语义改变固定标签生命周期则增补 ADR；否则仅本 spec。
- [x] 响应式 UI（最小窗口尺寸可用性）：侧栏行高与按钮尺寸沿用现有 `ROW_HEIGHT / 24px`，不新增溢出风险。

## 影响模块

- `src/features/workspace/sidebar.rs`：`render_local_dir` 条件渲染与右键菜单。
- `src/features/workspace/shell.rs`：`stop_local_project` 与 `close_local_session_internal(keep_pinned)` 抽取。
- `src/features/workspace/tabs.rs`：`close_other_local_sessions` 的参照与 `detach_splits_for` 复用。
- `src/features/workspace/view.rs` / `registry.rs`：`preferred_state`、`local_dirs` 重建的联动（只读）。
- `crates/crossh-ui/src/context_menu.rs`：`ShellMenuAction::StopLocalProject`。
- `locales/en.yml` / `zh-CN.yml`：文案。
- `docs/testing.md`：补充侧栏项目停止行为矩阵。

## 验收清单

- [x] spec 评审通过（AI 评审 + 人批准）—— 2026-08-21 Square 图标已补
- [x] 行为契约全部固化为失败测试并确认失败原因正确（Red，按 `spec_20260821_sidebar_close_project__*` 命名，覆盖契约 1-6：`count` 分支渲染、批量关闭保留 recent/pinned、焦点回退、重建恢复、风险确认、右键菜单）—— 14 项先红后绿
- [x] 最小实现通过聚焦测试（Green）—— 14 契约测试 + `cargo test --workspace` 全绿（`env -u CROSSH_UPDATE_SIGNING_KEY`）
- [x] `cargo fmt --check`
- [x] `scripts/check-architecture.sh`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `cargo test --workspace`（`env -u CROSSH_UPDATE_SIGNING_KEY` 全量通过；`crossh` 246 passed 含 14 新增）
- [ ] 声明的平台 CI job 通过（仅 macOS 人工验证 + 全平台 `cargo test`，本地已验证，Actions 待跑）
- [x] 结构性决策提炼进 ADR（如有）并登记 `docs/architecture.md`—— 无新增结构性边界，沿用 ADR 0002/0007/0012
- [x] 调试根因合并进 `docs/engineering-notes/`（如有）—— 无新增根因
- [ ] 新增行为合并进 `docs/testing.md` 关键行为矩阵
- [ ] 用户可观察效果人工确认（侧栏 `2/1/0` 三态按钮区分、停止后重建与重启一致、`default_command` 派发）

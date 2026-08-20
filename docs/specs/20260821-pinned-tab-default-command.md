# 固定标签默认命令与重载

## 元数据

- 状态：`in-progress`
- 创建：2026-08-21
- 相关 ADR：`docs/adr/0004-feature-owned-settings.md`、`docs/adr/0012-spec-driven-development-loop.md`
- 相关 issue / 路线图项：无
- CI 平台影响：`仅 macOS`（逻辑与设置持久化跨平台一致，终端执行语义仅在 macOS 本地验证）

## 背景

固定标签（`PinnedLocalTab`）目前只持久化 `project_dir / cwd / custom_name`，每次恢复都是裸 `zsh`。用户常见需求是：把某个固定标签命名为 `Agent` 后希望它自动进入 `opencode`，或把某项目标签绑定到 `ssh xxx -t 'cd /srv/app && exec zsh'` 以实现项目→主机的绑定。现状只能手动输入命令，切换项目后重复操作心智负担高。

同时用户提出侧边栏 `活跃/主机` 主机归属模型与项目归属心智不符，希望后续隐藏；本 spec 不处理侧边栏重构，只用一条通用 `default_command` 解决“固定标签自动进入指定命令”的可复用原语，SSH 绑定只是该原语的一个实例（`ssh ...`），避免为 SSH 单开一套 `Project.hosts` 模型。

## 目标

1. 固定本地会话标签可配置一条 `default_command`，写入 `settings.toml` 并随固定记录持久化，重启后保留。
2. 固定标签右键菜单提供 `编辑默认命令 / 重载默认命令 / 清除默认命令` 能力，菜单可用性与当前状态一致。
3. 配置了默认命令的固定标签在打开/恢复时自动执行该命令；未配置时保持现状裸 `zsh`。
4. 已在 `zsh 空闲` 状态的固定标签可通过 `重载默认命令` 一键进入该命令，无需手动粘贴。
5. 默认命令的编辑、清除、取消固定、关闭标签等路径均同步持久化且无残留。

## 非目标

- 不改变侧边栏 `活跃 / 主机库` 的展示与隐藏（由 `show_host_sidebar` 开关另行控制，本期不删代码）。
- 不为非固定（普通）本地会话提供默认命令持久化入口（避免产生无法持久化的临时状态）；普通标签右键不出现编辑/重载项。
- 不新增远程标签（`RemoteTab` / SFTP / Forward）的默认命令。
- 不解析 `ssh` 语义去联动 `ConnectionManager` 的结构化状态点、SFTP、端口转发；本期只做终端层命令投递，`ssh xxx` 仅作为普通命令在终端内执行。
- 不改变标签拖拽排序、标签条布局、最近目录规则与关闭确认流程。
- 不引入新的持久化文件；继续使用现有 `settings.toml`。

## 行为契约

1. 当用户对已固定标签选择 `编辑默认命令` 并提交非空字符串 `cmd` 时，该标签的持久化记录应写入 `default_command = Some(trimmed(cmd))`，标签上应出现默认命令标识（如 `Play` 小图标或等价视觉），且重启后该记录仍为 `Some(cmd)`。
2. 当提交的文本全空白（`trim.is_empty()`）时，应视为清除，持久化记录归一为 `None`，标签的默认命令标识消失；空白提交不产生 `Some("")`。
3. 当用户在 `编辑默认命令` 弹窗中取消时，持久化记录与标签显示应保持不变，不产生写入。
4. 当固定记录的 `default_command` 为 `Some(cmd)` 且该标签的会话被创建或随项目激活恢复时，终端应自动执行 `cmd`（观察到终端收到该命令的执行请求；重启恢复路径同理）。当 `default_command` 为 `None` 时，不自动执行任何命令，保持裸 `zsh`。
5. 当用户对 `default_command == Some` 且终端处于空闲（无前台命令运行）的固定标签选择 `重载默认命令` 时，终端应再次执行该命令；当 `default_command == None` 时该菜单项应为 `disabled` 且点击无效。
6. 当终端正在运行前台命令（`is_command_running == true`）时，`重载默认命令` 应为 `disabled`，防止叠加执行。
7. 当用户对 `default_command == Some` 的固定标签选择 `清除默认命令` 时，持久化记录应归一为 `None`，标签标识消失；当 `default_command == None` 时该项为 `disabled`。
8. 当固定标签被 `取消固定` 或被关闭（关闭按钮 / 关闭其他 / 进程退出）时，其 `default_command` 随所属固定记录一并移除，重启后不再恢复该命令。
9. 当用户对未固定标签打开右键菜单时，不应出现 `编辑默认命令 / 重载默认命令 / 清除默认命令` 三项；仅已固定标签出现。
10. 当应用启动时某固定记录的 `project_dir` 或 `cwd` 已失效（删除、改名、普通文件、不可访问）时，该记录（含 `default_command`）应被跳过并从持久化中清理，启动不崩溃。
11. 当 `default_command` 包含的文本含首尾空白时，持久化应存 `trim` 后的值，且多次 `normalized()` 保持幂等。

## 边界与错误

- `default_command` 以 `Option<String>` 存储，`None` 与 `Some("")` 等价归一为 `None`，与 `custom_name` 的空白归一规则一致；`normalized()` 负责去空白与幂等。
- 固定记录的 `pin_id` 仍是唯一身份键，`default_command` 不参与身份推导；新增/删除记录不影响其他 `pin_id` 的命令。
- 编辑弹窗打开期间会话被关闭：提交时若会话已不存在，静默忽略，不写持久化，不崩溃。
- 设置保存失败（写盘失败）：内存态保持本次编辑前的 `default_command`，不崩溃，按现有设置持久化错误路径记日志（契约同 `pin-rename` 的契约 10）。
- 重载执行失败（终端实体不存在、命令投递失败）：不改持久化记录，可选 `Toast` 提示，不崩溃；失败不影响后续重载可用性（空闲后可再次重载）。
- 恢复路径自动执行：若终端尚未就绪，命令应在终端可接收输入后投递；不得因时序丢失命令，也不得重复投递两次。
- 旧版本 `settings.toml` 无 `default_command` 字段时，以 `None` 解码，行为等价于未配置，保证向前兼容。

## 接口与状态变更

- `WorkspaceSettings::PinnedLocalTab`（`src/features/workspace/settings.rs`）新增 `#[serde(default, skip_serializing_if = "Option::is_none")] pub default_command: Option<String>`，`normalized()` 中 `trim` 并空白归一；旧文件缺字段默认为 `None`。
- `LocalSession`（`src/features/workspace/view.rs`）新增运行时镜像 `default_command: Option<String>`，供标签标识与菜单可用性判断、重载投递使用；随 `pin_id/custom_name` 同步生命周期。
- `ShellMenuAction`（`crates/crossh-ui/src/context_menu.rs`）新增固定标签动作：`EditDefaultCommand(LocalSessionId)` / `ReloadDefaultCommand(LocalSessionId)` / `ClearDefaultCommand(LocalSessionId)`。
- `tab_strip.rs::local_session_menu_entries` 已固定分支新增三项（编辑/重载/清除），可用性按契约 5-7 控制；未固定分支不新增。
- 弹窗：新增 `default_command_editor` 模态（复用 `view.rs:1287 render_rename_editor` 的 `ModalDialog + ime_input_canvas` 模式），标题/占位符/按钮文案走 `i18n`。
- 图标：复用现有 Lucide 资产 `Play` / `RefreshCw` / `Pencil`（1.27.0 固定版本），不新增 `assets/icons/*.svg`。
- 终端执行：复用既有 `TerminalView::run_command` 或等价重建 PTY 方式投递命令，不新增 `gpui` 依赖到纯逻辑层。

## 平台影响

- 设置读写与 `normalized()` 为纯逻辑，三平台行为一致，`cargo test` 可覆盖。
- 终端命令投递与 `is_command_running` 判断依赖本地 PTY，仅在 macOS arm64 本地验证；Linux / Windows 的设置持久化由对应 GitHub Actions job 验证，终端执行不声明跨平台契约。

## 涉及纪律

- [x] Logic must not depend on UI：`default_command` 的 `trim/空白归一/幂等` 做成纯函数或 `normalized()` 内聚，无 `gpui` 依赖。
- [x] Feature-owned settings：字段与归一规则归 `workspace` feature，`persistence.rs` 只做快照读写。
- [x] 图标纪律（Lucide 1.27.0 官方 SVG，IconName 映射）：仅复用已登记图标，不手写 path。
- [x] 文件规模 < 2000 行：`view.rs` 若因新增弹窗超限则拆分 `modal_editor.rs` / `tab_strip.rs` 按既有拆分纪律执行。
- [x] 工程笔记 / ADR 同步义务：本 spec 不新增结构性边界，无需新 ADR；如评审中决策改变则增补。
- [x] 响应式 UI：编辑弹窗复用现有模态宽度（`px(420)` 级别），最小窗口下可用；标签标识不改变标签尺寸与滚动。

## 影响模块

- `src/features/workspace/settings.rs`：`PinnedLocalTab` 新字段、归一化、单测。
- `src/features/workspace/pinned.rs`：如需 `default_command` 的纯逻辑归一/过滤，补充单测。
- `src/features/workspace/view.rs` / `src/features/workspace/modal_editor.rs`：`LocalSession.default_command` 镜像、编辑弹窗渲染与提交/取消、重载投递。
- `src/features/workspace/shell.rs` / `tabs.rs`：固定/取消固定/重载/清除的动作处理、项目激活恢复时的自动执行、关闭路径清理与持久化保存。
- `src/features/workspace/tab_strip.rs`：右键菜单新增三项与可用性控制、标签默认命令标识。
- `crates/crossh-ui/src/context_menu.rs`：`ShellMenuAction` 新变体。
- `locales/en.yml`、`locales/zh-CN.yml`、`src/shared/i18n.rs`：`context_menu.edit_default_command / reload_default_command / clear_default_command`、`default_command_editor.title/placeholder/save` 文案。
- `docs/testing.md`：补充固定标签默认命令行为矩阵。

## 验收清单

- [x] spec 评审通过（AI 评审 + 人批准）—— 2026-08-21 用户批准
- [x] 行为契约全部固化为失败测试并确认失败原因正确（Red，按 `spec_20260821_pinned_tab_default_command__*` 命名）—— 10 个 `spec_20260821_*` 测试先红后绿，覆盖契约 1-11（含 trim/空白归一、取消、提交后关闭忽略、清除、菜单禁用态、未固定无入口、恢复应用、幂等、序列化 skip）
- [x] 最小实现通过聚焦测试（Green）—— `cargo test -p crossh` 220 passed（新增 10）
- [x] `cargo fmt --check`
- [x] `scripts/check-architecture.sh`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `cargo test --workspace`
- [ ] 声明的平台 CI job 通过（Linux / Windows 设置持久化由 Actions 验证，spec 保持 `in-progress` 直到通过）
- [x] 结构性决策提炼进 ADR（如有）并登记 `docs/architecture.md`—— 无新增结构性边界，沿用 ADR 0004/0012
- [x] 调试根因合并进 `docs/engineering-notes/`（如有）—— 无新增根因
- [ ] 新增行为合并进 `docs/testing.md` 关键行为矩阵
- [ ] 用户可观察效果人工确认（固定标签编辑/重载/清除、标签标识、重启保留、项目激活自动执行、空闲/运行中禁用态）

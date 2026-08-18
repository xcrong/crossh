# P1/P2 简化项任务清单（2026-08-17 审计 backlog）

来源：`docs/audit/2026-08-17-simplification-audit.md`（P0 与 P3 已完成：P0 见 `a90b465`/`bed7e92`，P3 见 `4b12e56`）。
用途：供后续会话的 Agent 逐项执行。**P1 是 spec 类（必须走 SDD：先写 spec → AI 评审 → 人批准 → 才实现）；P2 是决策类（必须由人先裁定选项，或 Agent 给出裁定建议后由人拍板）。**

---

## P1 — spec 类（5 项）

### S-B1 删除 `InputCmd`/`SessionEvent` 死终端契约（联动 loopback spec）

> **状态：已完成（2026-08-17，spec: `docs/specs/20260817-remove-dead-terminal-contract.md`）**
> 删除清单全部落地：session.rs 两符号、mod.rs re-export、connection.rs cfg(test)
> 终端设施（OpenTerminal 变体、open_terminal_channel、detect_remote_shell、
> relay_terminal、drive_input、cfg(test) bootstrap 连带死代码）+ 3 个 `#[ignore]`
> 测试。`TerminalProcessInfo` 保留。人工验证定位由 loopback spec 承接。

- **位置**：
  - `crates/crossh-core/src/terminal/session.rs:4,23` —— `#[allow(dead_code)] pub enum InputCmd` / `SessionEvent`（生产零消费）
  - `crates/crossh-core/src/terminal/mod.rs:7` —— 对应 re-export
  - `crates/crossh-ssh/src/connection.rs` cfg(test) 终端基础设施：`ConnCmd::OpenTerminal`(59-65)、`run_connection` match 分支(296)、`open_terminal_channel`(667-716)、`detect_remote_shell`(719-738)、`relay_terminal`(861-902)、`drive_input`(905-927)
  - 3 个 `#[ignore]` 真实主机测试：`connect_real_host`、`connect_and_run_remote_command`、`connect_and_run_ls`(1084-1343)
- **消费者证据**：生产零消费；全部使用点在 connection.rs 的 cfg(test) 范围。生产远程终端走 `tabs.rs:571-599` 的 `ssh -tt` + `remote_shell_bootstrap_command()`，不经过该契约。
- **硬性约束**：`TerminalProcessInfo`（session.rs:17-20）是生产符号（被 `terminal/title.rs:3,27,226` 消费），**不能删**。
- **联动**：`docs/specs/20260817-ssh-hermetic-loopback.md`（draft）契约 5 声明"重建后可以再次打开终端或执行命令"。3 个 `#[ignore]` 测试的处置归 loopback spec 负责；本 spec 负责删除老 cfg(test) 通道设施，建议**实施顺序排在 loopback 之后**，spec 中写明依赖。
- **反方论证（保留理由）**：删除会移除"真实主机人工验证"路径；但与 ADR 0006（hermetic 测试）冲突且 CI 永不执行。
- **验收**：rg 全仓库零引用；`cargo clippy --workspace --all-targets -D warnings` 绿；全 workspace 测试绿。

### S-B4 统一两份 shell bootstrap 生成逻辑（联动 loopback spec）

> **状态：已完成（2026-08-17）**
> S-B1 已删掉 cfg(test) 版，无第二份逻辑可统一；剩余工作为生产版
> shell.rs 内部 per-shell 提取重构（行为不变）：拆出
> `remote_bash_startup_script`/`remote_zsh_launcher_script`/`remote_fish_startup_script`
> 三个 per-shell payload 生成函数，`remote_shell_bootstrap_selector()` 组装运行时
> selector，`remote_shell_bootstrap_command()` 仅做 base64 包装。重构前后
> 生产字节流经临时 dump 比对 **逐字节一致**。新增两条单元测试：
> `remote_bootstrap_payloads_cover_each_shell`（各 shell 命令形态）与
> `remote_bootstrap_selector_embeds_unchanged_shell_payloads`（selector 内嵌
> 字节与生成函数一致，防漂移回归）。若 loopback spec 日后需要测试版命令
> 形态，直接在 loopback 侧依赖生产版即可。

- **位置**：
  - 生产版：`crates/crossh-core/src/terminal/shell.rs:332-404`（`remote_shell_bootstrap_command()`，无参，base64 编码 selector，运行时按 `$SHELL` 分派）
  - 测试版：`crates/crossh-ssh/src/connection.rs:740-762`（`#[cfg(test)] fn remote_shell_bootstrap_command(shell: RemoteShell)`）
- **消费者证据**：生产版唯一消费点 `src/features/workspace/tabs.rs:592`（`zed_ssh_shell`）；测试版唯一消费点 connection.rs:698（cfg(test)）。
- **语义差异（必须保留）**：bash 测试版用进程替换 `<(printf ...)`，生产版写 temp 文件 + `--rcfile`——shell.rs:392-399 注释明确论证进程替换式嵌套引号会被远端 /bin/sh 变体误解析；zsh 测试版内联进 `.zshrc`，生产版用 `.zshenv` + ZDOTDIR + deferred precmd；fish 类似。
- **建议做法**：提取 per-shell 生成函数 `remote_shell_bootstrap_command_for(shell)` 到 shell.rs，生产 selector 分分支复用、测试版改调该函数。这是行为变化（测试路径语义升级为生产版）。
- **联动**：3 个 `#[ignore]` 测试 CI 不执行、无法本地验证等价性，须与 loopback 落地联动或明确接受"测试路径不再独立验证生产路径"。
  **更新（2026-08-17）：** S-B1 已删除 connection.rs 的 cfg(test) 版
  `remote_shell_bootstrap_command`，"测试版统一"对象不复存在；本项剩余工作仅为
  生产版（shell.rs）内部 per-shell 提取 refactor，属行为不变重构（按 AGENTS
  豁免）或可关闭——除非 loopback spec 需要测试版命令形态，届时在 loopback 侧
  直接依赖生产版即可。
- **验收**：单元测试覆盖各 shell 生成的命令形态；生产消费点字节流不变。

### S-B2 删除 `AuthChoice::Password` 零构造变体

> **状态：已完成（2026-08-17，spec: `docs/specs/20260817-remove-auth-choice-password.md`）**
> 删除 `AuthChoice::Password` 变体 + `#[allow(dead_code)]`（session.rs）+ 不可达
> match 分支（connection.rs）；lib.rs re-export 不变。契约测试 3 条：
> `default_auth_never_yields_password`（候选穷尽 Key/Agent + 顺序）与
> `password_fallback_roundtrip`/`password_fallback_none_when_ui_unreachable`
> （UI 凭据兜底往返与取消）。`rg` 全仓库零引用；fmt/architecture/clippy
> 全绿；全 workspace 测试通过（164+ 用例）。testing.md Connection 矩阵追加
> 认证候选与密码兜底契约。

### S-A1 合并 sftp 字符编辑与 `text_editing.rs` 重复实现

> **状态：已完成（2026-08-18，spec: `docs/specs/20260818-merge-sftp-text-editing.md`）**
> `RemoteEditor` 合并为持有 `TextEditingState`（content/cursor/ime 四字段收敛）；
> `TextEditingState::backspace/delete/move_horizontal/replace_selection` 返回
> bool（"是否实际发生修改"，供脏标记使用）+ 字符边界 debug_assert 上移；
> 删除 logic.rs 的 `insert_text`/`backspace_char`/`delete_char`/
> `move_cursor_horizontal` 及其测试。S-A10 联动一并落地：删除
> QuickCommandEditor/CommitEditor 薄委托方法，调用点（shell.rs、git/input.rs、
> workspace/view.rs、git/render.rs、shell_input.rs）全部直连 `state`。
> 契约测试 7 条（spec 前缀命名）覆盖 bool 语义、选区端跳跃、幽灵锚点清除、
> sftp UTF-8 编辑与脏标记、IME 状态承载。rg 零残留；fmt/architecture/clippy
> 全绿；全 workspace 测试通过。

- **位置**：`src/features/sftp/logic.rs:72-111`（`backspace_char`/`delete_char`/`move_cursor_horizontal`，无选区自由函数 `&mut String, &mut usize -> bool`）vs `src/shared/text_editing.rs:71-120`（`TextEditingState` 结构体方法，含选区/IME）。
- **消费者证据**：sftp 版生产消费 `src/features/sftp/view.rs:584-585,946`、`view_input.rs` 的 EntityInputHandler(114/150/190)；shared 版消费 `features/git/input.rs:57-68`、`settings/agent.rs:552-556`、`workspace/shell.rs:870-908`。无选区退格/删除/横向移动语义逐行相同；`move_cursor_vertical`/`line_bounds` 是 sftp 独有。
- **建议做法**：给 `TextEditingState` 增加无选区便捷语义或让 RemoteEditor 复用状态机；bool 返回值语义、调试断言是行为契约。
- **联动**：S-A10（薄委托层封装风格不一致）随本 spec 一并设计，不单独行动。
- **反方论证**：两实现"逐行相同"但合并涉跨模块改动面，sftp 独有功能必须保留。
- **验收**：sftp 编辑行为（退格/删除/移动/选区）由既有测试 + 新增契约测试覆盖。

### S-D1 收缩 providers 非流式入口链

> **状态：已完成（2026-08-18）**
> 删除 `complete`/`complete_with_tools`/`complete_target` 三个零消费者函数
> （providers.rs），lib.rs 导出同步收紧为 `complete_stream_with_options`；
> 链尾 `complete_target_with_timeout`（policy.rs:293 消费）保留。属行为不变
> 死代码删除（豁免 spec）：rg 全仓库零引用；fmt/architecture/clippy
> （workspace --all-targets -D warnings）全绿；crossh-agent 40 条测试通过。

- **位置**：`crates/crossh-agent/src/providers.rs:29,37,49` —— `complete`/`complete_with_tools`/`complete_target`（pub，lib.rs 导出）。
- **消费者证据**：无生产调用点；链尾 `complete_target_with_timeout`（pub(super)，`policy.rs:293` 消费）**保留**。
- **建议做法**：删除 3 个函数 + lib.rs 导出更新（当前导出为 `complete, complete_stream_with_options`）。
- **反方论证**：`complete` 曾是公共 API；无生产者时属投机性接口。
- **验收**：rg 零引用；全 workspace 编译无警告。

---

## P2 — 决策类（13 项）

每项给出**决策选项 + 倾向建议**。人裁定后按选项实施。Agent 可先输出裁定建议供人拍板。

### S-D10 `UpdateManifest.published_at` / `UpdateArtifact.signature` 字段从未被生产读取

> **状态：已完成（2026-08-18，按裁定"只删 `published_at`"）**
> 删除 `UpdateManifest.published_at` 字段（model.rs 定义 + 测试置 None 处），
> 生产/测试零残留；`UpdateArtifact.signature` **保留**并补注释：它是
> `docs/remote-update-plan.md` 安全边界中已声明的 Ed25519 更新协议签名预留
> （SHA-256 不能抵抗发布源被恶意改写），不等同于 macOS Developer ID 代码签名。
> serde 对未知字段默认忽略，删除后旧 manifest 仍可解析。rg 零引用；
> fmt/architecture/clippy（workspace --all-targets -D warnings）全绿；
> 全 workspace 测试通过。

- **位置**：`crates/crossh-update/src/model.rs:25,37`；只在测试中置 `None`(291,301)；`generate-update-manifest.sh` 不输出。
- **选项**：A. 删除两字段；B. 真正启用签名验证（范围扩大，需产品决策）。
- **建议**：A（`signature` 若确为未来签名预留可只删 `published_at`，spec 或注释说明）。

### S-D13 `release.yml` validate job 与 `ci.yml` check job 重复

> **状态：已完成（2026-08-18，按裁定 B）**
> 删除 validate job 与 ci.yml check job 重复的 step（fmt/architecture/clippy/
> 全量测试/Cargo.lock 同步校验及其支撑的 toolchain/cache step），仅保留发布
> 专属的 tag 版本一致性校验；job 注释说明意图：通用门禁由 ci.yml 在 main
> push 与 PR 上执行，tag 均从已验证的 main 提交打出，发布时不重复消耗
> macOS runner 时间。裁定依据：main 分支未开 branch protection，validate
> 仅是冗余保险而非强制门禁。yaml 语法验证通过。

- **位置**：`.github/workflows/release.yml:19-69` vs `.github/workflows/ci.yml:11-41`（fmt/check-architecture/clippy/全量测试/Cargo.lock 校验无差异化 step）。
- **选项**：A. 保留（发布前强制验证属刻意冗余）+ 注释说明；B. 删除 validate 中重复项，仅保留发布专属 step。
- **建议**：倾向 A 的保守变体——先加注释说明意图；若确认 CI 已强制 check 通过才能发布则做 B。**默认不要动 release 关键路径**。

### S-D15 `package-windows.ps1` 内嵌 assets 拷贝逻辑

- **位置**：`scripts/package-windows.ps1:37-62` vs `scripts/copy-shared-assets.sh:10-34`（Zed checkout 前缀查找 + 字体/图标拷贝 + manifest.json）。
- **选项**：A. 保持（跨语言必然重复，维护成本可接受）；B. CI 预生成资产包再消费。
- **建议**：A。B 改动大、收益小，不值得单独立项。

### S-A5 `git/window.rs` 的 `_refresh_task` 永不置回 None、从不读结果

> **状态：已完成（2026-08-18，按裁定 A）**
> `_refresh_task` 重命名为 `refresh_task` 并补注释：字段是"刷新循环已启动"
> 标记（仅防重入 is_some 检查），从不置回 None，持有 Task 句柄使循环随
> 实体 drop 自动取消。属行为不变重构（豁免 spec）：fmt/architecture/
> clippy（workspace --all-targets -D warnings）全绿；全 workspace 测试通过。

- **位置**：`src/features/git/window.rs:52,647-658`（字段仅 init/is_some 防重入/赋值三处使用；Task 作用为随实体 drop 取消）。
- **选项**：A. 保持设计 + 去掉下划线前缀命名 + 补注释（"刷新循环已启动"标记 + drop 取消语义）；B. 简化为 bool + 明确取消策略（需 verify Task drop 取消）。
- **建议**：A（最小改动，行为不变）。

### S-A7 `app/mod.rs` 的 `LaunchTarget` 单变体枚举 + `open_launch_target` 薄包装

> **状态：已完成（2026-08-18，按裁定 A）**
> 保留 `LaunchTarget` 枚举与 `open_launch_target` 转发函数并补注释：
> 启动目标是 CLI 参数解析的产物，`on_reopen` 回调复用同一目标恢复启动
> 入口；未来 CLI 新增直接打开项目/SSH 会话等入口时扩展变体即可，main 与
> on_reopen 闭包无需感知具体目标。属行为不变文档改动（豁免 spec）：
> fmt/architecture/clippy（workspace --all-targets）全绿；全 workspace
> 测试通过。

- **位置**：`src/app/mod.rs:7-16`；消费点 `main.rs:91,99-107,125`。
- **选项**：A. 保留（未来多启动目标扩展点）+ 注释说明；B. 塌缩为 `open_main_window` 直接调用。
- **建议**：倾向 A（reopen 分支有真实用途，枚举成本极低）；若坚持 YAGNI 选 B。

### S-A9 `TerminalViewEvent` 单变体枚举

> **状态：已完成（2026-08-18，按裁定 A）**
> 保留 `TerminalViewEvent` 枚举（事件类型扩展点）并补注释：此类事件以枚举
> 形式承载而非直接方法调用，便于未来新增 UI-only 请求（如 copy-all、
> reopen session）而不必扩展 workspace match 分支或让终端视图耦合具体
> 消费者。属行为不变文档改动（豁免 spec）：fmt/architecture/clippy
> （workspace --all-targets -D warnings）全绿；全 workspace 测试通过。

- **位置**：`src/features/terminal/view.rs:149`（唯一变体 `SendSelectionToAdjacent`；emit view.rs:932；订阅 tabs.rs:145、shell.rs:421 —— 有监听者，非死代码）。
- **选项**：A. 保留（事件类型扩展点）+ 注释；B. 塌缩为 `send_to_adjacent` 方法调用。
- **建议**：A。

### S-A10 QuickCommandEditor / CommitEditor 薄委托层

> **状态：已完成（2026-08-18，联动 S-A1 spec：`docs/specs/20260818-merge-sftp-text-editing.md`）**
> 随 S-A1 一并落地（commit `3113257`）：删除 QuickCommandEditor/CommitEditor 全部
> 薄委托方法（原 command_editor.rs:30-60、git/editor.rs:18-54 的 8+1 个单行转发），
> 两类编辑器仅保留 UI 侧字段（scope/original/focus/scroll 与 focus），调用方
> （shell.rs、git/input.rs、workspace/view.rs、git/render.rs、shell_input.rs、
> sftp/view.rs）全部直连 `editor.state`，与 `TextEditingState` 直接 pub 字段风格统一；
> 测试随状态机迁移至 shared/text_editing.rs。验证结果同 S-A1 关闭记录
> （契约测试 7 条、rg 零残留、fmt/architecture/clippy 全绿、全 workspace 测试通过）。

- **位置**：`src/features/workspace/command_editor.rs:30-60`、`src/features/git/editor.rs:18-54`（8+1 个单行转发方法，全部有生产调用点）；与 `TextEditingState` 直接 pub 字段形成两种封装风格。
- **选项**：A. 随 S-A1 spec 一并设计（让调用方直接操作 `editor.state` 或收紧字段可见性）；B. 保持现状。
- **建议**：A。**不要单独行动**，等 S-A1。

### S-C3 `Separator` 组件整个模块零消费者

- **位置**：`crates/crossh-ui-component/src/separator.rs`（`Separator`/`SeparatorOrientation`/`horizontal`/`vertical`/`orientation`；仅 lib.rs:40,60 re-export + 自身 cfg(test)）；注意 `MenuEntry::Separator`（crossh-ui context_menu.rs:131）是无关独立变体。
- **选项**：A. 删除模块 + 同步 `docs/architecture.md:56` 描述；B. 为它安排真实消费者。
- **建议**：A。
- **注意**：architecture.md:56 将 separators 列为组件库内容，删除需同步文档（或更新 ADR 0009 相关描述）。

### S-C5 `ButtonVariant::Info/Warning/Success` 零构造点

- **位置**：`crates/crossh-ui-component/src/button.rs:22-23,80-100`（style() 完备 match 分支故无编译警告）；生产只用 Default/Primary/Secondary/Ghost/Danger/Link。
- **选项**：A. 删除 3 变体 + style()/主题色映射同步；B. 保留（组件 API 预留，风格同 ADR 0013 的 ToastTone 契约论证）。
- **建议**：B（记录预留理由）；若坚持最小 API 选 A（改动面：style() match + 主题色）。

### S-C7 `_toast_task` 字段只写不读

- **位置**：`src/features/workspace/registry.rs:88,100`；唯一写入 `toaster_view.rs:19`；下划线表示保存 Task 句柄防 drop 取消。
- **选项**：A. 改 `cx.spawn(...).detach()` 删字段（需确认 GPUI detach 语义与保存句柄等价）；B. 保留 + 补注释说明"生命周期锚"意图。
- **建议**：先查 GPUI 版本 detach 语义，等价则 A，否则 B。

### S-C8 `SplitHandleSide` 及 `handle_side()/handle_left()/line()` 无外部消费者

- **位置**：`crates/crossh-ui-component/src/split_resizer.rs:16-22,70-84`；`SplitResizer` 本身 4 处生产使用但全用默认 Right + min/max_width。
- **选项**：A. 收紧 API 固定右侧手柄；B. 保留（组件 API 完整性）。
- **建议**：B。

### S-D7 sdk `StreamAccumulator::set_protocol` 仅测试消费者

- **位置**：`crates/crossh-ai-sdk/src/lib.rs:558`；生产路径经 `StreamAccumulator::new(adapter.protocol())` 初始化。
- **选项**：A. 删除（连同 providers.rs cfg(test) wrapper 里的调用）；B. 保留为 SDK 扩展 API。
- **建议**：A（零消费者，SDK 面越窄越好；S-D3 已把兼容 wrapper 删掉，删除后无残留）。

### S-D11 `update_result_path` pub 无外部消费者

- **位置**：`crates/crossh-update/src/model.rs:210`；内部实现走 `pub(crate) update_result_path_in`；lib.rs:17 导出。
- **选项**：A. 改 `pub(crate)` + 删导出；B. 删除（若内部也无调用）。
- **建议**：A（最小改动；先确认 pub(crate) 覆盖全部内部调用）。

---

## 执行规则提醒（给实施 Agent）

1. P1 必须先写 spec（`docs/specs/YYYYMMDD-<slug>.md`，模板 `docs/specs/template.md`），状态改 `approved` 后按 TDD 实现（测试名前缀 `spec_20260817_<slug>__`）。
2. P2 未经人裁定不得直接实现；裁定后按选项执行。
3. 所有改动过 pre-commit 门禁：`scripts/check-architecture.sh`、`cargo fmt --check`、`cargo clippy --workspace --all-targets -D warnings`，外加 `cargo test --workspace`。
4. 受保护表面禁止触碰：logic/UI 分层、ADR 裁决边界、check-architecture.sh 白名单、固定 Zed/GPUI/Lucide revision、engineering-notes 防御模式。
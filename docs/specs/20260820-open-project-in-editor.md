# 状态栏「在外部编辑器中打开」按钮

## 元数据

- 状态：`in-progress`
- 创建：2026-08-20
- 相关 ADR：`docs/adr/0002-logic-ui-layering.md`、`docs/adr/0004-feature-owned-settings.md`、`docs/adr/0012-spec-driven-development-loop.md`
- 相关 issue / 路线图项：无
- CI 平台影响：`全部`

## 背景

本地终端会话的工作目录显示在底部状态栏，但用户要把当前目录交给本机编辑器（VS Code、Zed、Cursor 等）时，只能回终端里敲一条命令。Crossh 需要一个状态栏入口，用本机已安装的编辑器打开当前 `cwd`，并通过自动检测与用户配置降低「装了很多编辑器但每次都敲错命令」的摩擦。

## 目标

1. 在状态栏为当前本地会话提供一个显式的「在外部编辑器中打开」按钮，点击后以 `{editor} {cwd}` 形式启动本机编辑器。
2. 自动检测 PATH 中的常用编辑器命令；检测候选列表是程序内写死的常量，不作为可配置项暴露。
3. 设置中提供一个下拉选择框，选项为自动检测后得到的本机已安装编辑器；用户从中选定一个作为高于自动检测的覆盖项，不提供自由文本输入。
4. 不覆盖设置的用户的体验尽可能顺畅：装了什么编辑器就打开哪个，一个都不装时给出明确错误提示。

## 非目标

- 不改变远程终端、SFTP、端口转发视图的状态栏内容；远程会话不提供该按钮。
- 不检测、不默认尝试终端内编辑器（vim/nvim/emacs）：它们在没有 tty 的后台进程中无法交互。
- 不解析 shell alias（`alias code=…` 在 PATH 中不可见）：检测只针对 PATH 中的可执行文件。
- 不暴露检测候选列表或检测顺序作为用户可配置项：候选列表写死在代码常量中，用户不可修改。
- 不提供自由文本编辑器命令输入；设置中编辑器选择只能来自自动检测结果（外加兼容展示旧配置值的只读项）。
- 不新增键盘快捷键、右键菜单入口或拖拽排序 UI。
- 不实现「记忆最近使用的编辑器」或跨机器同步检测结果。

## 行为契约

1. 当活动视图是本地会话时，状态栏应显示「在外部编辑器中打开」按钮，且按钮对该会话的 `cwd` 生效；当没有活动本地会话或活动视图为远程视图时，该按钮不应被渲染。
2. 当 `editor_command` 设置非空时，点击按钮应使用该值作为编辑器命令启动 `{editor} {cwd}`；空白/未设置的 `editor_command` 应被视为未配置并回退到自动检测，不得产生「以空命令启动」的行为。
3. 当未配置 `editor_command` 时，点击按钮应按代码内默认候选列表的顺序在 PATH 目录中查找第一个存在且可执行的编辑器命令，并使用该命令启动 `{editor} {cwd}`。
4. 检测候选列表是代码内常量（`DEFAULT_EDITOR_PRIORITY`），第一项为 `zed`，并包含 VS Code（`code`）及 Insiders、Cursor、JetBrains 系、Sublime、TextMate 的常用命令名；该列表不可被设置覆盖或部分修改。
5. 设置中的编辑器选择器是一个下拉选择框，其选项为点击展开时自动检测得到的本机编辑器列表（按默认候选顺序、去重、取每个候选首个命中路径），并恒定带有「自动检测」选项；「自动检测」清除 `editor_command` 覆盖，其余选项把对应命令写入 `editor_command`。当检测结果为空时下拉框只含「自动检测」一项。
6. 当已配置的 `editor_command` 值不在当前检测结果中时（例如旧配置迁移），下拉框应把该值作为只读选项展示，保证当前配置可见、可重新选中，不被静默丢弃；选择该选项不改变配置。
7. 编辑器主命令解析以候选顺序优先：按默认候选顺序逐候选在所有 PATH 目录中查找，命中即采用；PATH 目录顺序在单个候选内部决定同名命令的解析结果。解析结果对 UTF-8 命令名和带空格/引号的路径值保持正确。
8. 当未配置 `editor_command` 且默认候选列表中没有命令在 PATH 中可执行时，点击按钮应显示错误 Toast，提示未检测到编辑器并在设置中可配置，应用不得崩溃。
9. 当解析出的编辑器命令启动失败（命令不存在、权限不足、路径无效）时，应显示错误 Toast，提示启动失败，应用不得崩溃。
10. 视觉上按钮应保持状态栏在最小窗口尺寸下的布局：按钮为固定尺寸图标按钮，不随可用宽度变化，不挤压或隐藏状态栏中的已有控件。
11. 按钮的 tooltip 应说明该动作（在外部编辑器中打开当前目录）；当编辑器来源可确定（配置或已检测）时，tooltip 应包含具体编辑器命令名。

## 边界与错误

- 点击入口与本地会话的 `cwd` 绑定：每次点击使用点击发生时活动会话的当前 `cwd`，不缓存旧值，不因上一次点击留下旧闭包而打开旧目录。
- `editor_command` 设置为空白字符串时归一化为未配置；含首尾空白的命令名按 trim 后的值使用。
- 设置下拉框的检测结果在使用时实时计算，不缓存跨会话的检测结果；PATH 变化后重新展开下拉框得到新结果。
- Windows 上 `code` 等命令实际对应批处理文件（`.cmd`）时，解析与启动必须能处理该扩展名，不能因 `Command::new` 无法直接执行批处理而报「找不到程序」；该行为由 CI Windows runner 验证。
- macOS/Linux 上可执行判断以 Unix 可执行位为准；Unix 上不因文件不存在或不可执行而 panic。
- 启动外部编辑器使用分离进程组与非阻塞 stdio，不阻塞 Crossh、不在终端内挂起；编辑器进程由用户自行关闭。
- 并发或快速重复点击不产生共享可变状态，每次点击独立解析并启动，不出现重复启动以外的额外状态污染。

## 接口与状态变更

- `settings.toml` workspace 域字段：
  - `editor_command: Option<String>`（保留）：显式编辑器命令（或命令路径），为空/缺省时回退自动检测；只能通过设置中的下拉选择框写入。
  - `editor_priority` 字段移除：不再序列化，读取旧配置文件时该字段被忽略；检测候选列表由代码常量 `DEFAULT_EDITOR_PRIORITY` 承载。
- 不引入新的外部 crate 或 wire 格式；自有检测与启动逻辑保持平台无关实现 + 平台边界分支。

## 平台影响

- macOS/Linux/Windows 的本地会话状态栏都获得该按钮；启动与检测逻辑在 macOS 本地全量验证。
- Windows 的 `.cmd` 解析与启动分支、Linux 的可执行位与路径行为点名为 CI `terminal-compat` job（ubuntu-22.04、windows-latest）验证，本地不交叉编译、不声称已验证这些平台。
- 各平台的可执行位/PATHEXT 差异集中在检测核心的单个可注入判定点上，以便纯逻辑测试用注入谓词覆盖全部分支。

## 涉及纪律

- [ ] Logic must not depend on UI（层级）：新增的编辑器检测与启动模块为零 `gpui` 依赖的纯逻辑模块，与 `git_launcher.rs` 同级；GPUI 只出现在 workspace 视图层调用点。
- [ ] Feature-owned settings：`editor_command` 归入 workspace feature 的 `WorkspaceSettings`，沿用现有持久化与归一化管道；检测候选常量留在纯逻辑模块。
- [ ] 图标纪律：新增 `square-pen.svg`，从 Lucide 1.27.0 官方源下载并原样引入，`IconName` 映射与资产加载器同步；不手写或改写 path 数据。
- [ ] 文件规模 < 2000 行：新增纯逻辑模块保持轻量，视图层改动为增量函数，`scripts/check-architecture.sh` 全量校验。
- [ ] 工程笔记 / ADR 同步义务：不改变长期边界，无需新 ADR；如实现暴露平台特有根因，落一篇工程笔记。
- [ ] 响应式 UI（最小窗口尺寸可用性）：固定尺寸图标按钮，tooltip 不参与布局，紧凑与标准窗口尺寸下均需人工核验；设置下拉框在紧凑布局下不超出内容区。

## 影响模块

- `docs/specs/20260820-open-project-in-editor.md`（本 spec）
- `src/features/editor_launcher.rs`（新增：检测 + 命令构造纯逻辑，含检测结果列表函数）
- `src/features/workspace/view.rs`（状态栏按钮渲染与点击）
- `src/features/workspace/settings.rs`（`editor_command` 字段与归一化；移除 `editor_priority`）
- `src/features/workspace/shell.rs`（设置 setter 与持久化触发点点缀）
- `src/features/settings/window.rs`（设置窗口下拉选择框）
- `src/features/settings/input.rs`（移除编辑器文本输入分支）
- `src/features/settings/persistence.rs`（设置 round-trip 测试与旧字段忽略测试）
- `crates/crossh-assets/src/lib.rs` + `crates/crossh-assets/assets/icons/square-pen.svg`（图标注册与资产）
- `locales/en.yml`、`locales/zh-CN.yml`（tooltip / settings / toast 文案）
- `docs/testing.md`（关键行为矩阵补充）

## 验收清单

- [x] spec 评审通过（AI 评审 + 人批准；2026-08-20 批准后按用户反馈修订：移除检测顺序配置、编辑器选择改为自动检测结果下拉框，修订稿含用户批准）
- [x] 行为契约全部固化为失败测试并确认失败原因正确（Red：`resolve_editor` 签名变更与 `detect_editors` 新增以编译失败/断言失败形式先行验证）
- [x] 最小实现通过聚焦测试（Green：`cargo test --bin crossh` 全绿，209 通过）
- [x] `cargo fmt --check`
- [x] `scripts/check-architecture.sh`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `cargo test --workspace`（唯一失败 `sign_manifest_cli::sign_without_key…` 为环境相关预存在失败：本机 shell 已设 `CROSSH_UPDATE_SIGNING_KEY` 使子进程签名成功；`env -u` 复跑该 target 全绿，与本次变更无关）
- [ ] 声明的平台 CI job 通过（Windows `.cmd` 检测/启动分支、Linux 可执行位：提交后由 Actions `terminal-compat` 验证，spec 状态保持 in-progress 直到通过）
- [x] 结构性决策提炼进 ADR（无新结构决策：沿用 `git_launcher` 纯逻辑启动器先例与 `0004` feature-owned settings，不登记新 ADR）
- [x] 调试根因合并进 `docs/engineering-notes/`（无新增根因）
- [x] 新增行为合并进 `docs/testing.md` 关键行为矩阵（Workspace 行更新：下拉框选择契约、固定候选列表、旧字段忽略）
- [ ] 用户可观察效果人工确认（状态栏按钮在紧凑与标准窗口下可见可用；已安装编辑器（如 Zed/VS Code）时可打开目录；设置下拉框选择与「自动检测」生效）
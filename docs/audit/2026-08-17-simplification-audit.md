# Crossh 简化扫描报告（2026-08-17）

扫描方式：四个并行只读 subagent（agent/ai-sdk/update + 依赖/脚本/workflows / ssh/core / 根 crate / ui/terminal/theme/assets），先跑死代码锚点（`cargo clippy --workspace --all-targets` 零警告、`rg allow(dead_code)` 全仓库 16 处逐一验证），再按域分片。证据具体到文件:行号。本报告未修改生产代码。

## 总体结论

仓库整体非常干净：clippy 全 workspace 零死代码警告、无未 emit 的订阅、主题 token 与图标映射无一闲置、受保护边界（logic/UI 分层、bin 拆分、共享 chrome）全部有消费者。真正的简化空间集中在三处：

1. **crossh-agent 的 SDK 迁移残留**：一整组"SDK 迁移后保留的兼容层"（auth 类型、wire 编解码、便捷入口）只有测试消费者，是最干净的一刀（S-D1~S-D4）。
2. **两个已知契约的收尾**：`terminal/session.rs` 的 `InputCmd`/`SessionEvent` 死契约（上轮 C-2）与两份 shell bootstrap（上轮 C-3）——均确认属实，但删除/合并是行为变更，需走 spec 并与 loopback spec 联动。
3. **一批过期豁免与死配置**：4 处 `#[allow(dead_code)]` 已过期（符号全部活跃），若干组件 API（Separator、v_flex、ButtonVariant 三色调、SplitHandleSide 方法）零消费者。

## 一、crossh-agent / crossh-ai-sdk / crossh-update / 依赖 / 脚本（审阅 D）

| 编号 | 位置 + 符号 | 消费者证据 | 严重度 | 处置 |
| --- | --- | --- | --- | --- |
| S-D1 | `crates/crossh-agent/src/providers.rs:29,37,49` — `complete`/`complete_with_tools`/`complete_target` | 无生产调用点；调用链前三个函数互相引用、无外部入口，链尾 `complete_target_with_timeout`（pub(super)）有生产消费者（policy.rs:293） | 中 | spec |
| S-D2 | `providers.rs:82` — `complete_stream` | 无任何调用点，仅转发到有消费者的 `complete_stream_with_options`；lib.rs:22 导出 | 中 | 直接修 |
| S-D3 | `providers.rs:19-27,341,413,425` — `AgentAuthStyle`/`AgentWireRequest`/`encode_request`/`decode_response`/`decode_stream_event` | 只有测试消费者（tests.rs:161-586）；注释自称"SDK 迁移后保留的兼容层"；lib.rs:21-24 导出 | 高 | 直接修（整组删除导出与函数，测试改直连 SDK 或删除） |
| S-D4 | `crates/crossh-agent/src/tools.rs:99` — `execute_tool` | 只有测试消费者（tests.rs:438-822 约 10 处）；生产一律用 `execute_tool_with_cancel`（agent_cli.rs:780）；lib.rs:25 导出 | 中 | 直接修 |
| S-D5 | `crates/crossh-ai-sdk/src/lib.rs:28,34,55` — `Protocol::ALL`/`Protocol::label`/`ThinkingLevel::ALL` | 无任何调用点；crossh-agent 有平行的 `AgentProtocol::ALL`/`AgentThinkingLevel::ALL`（生产使用），sdk 这份是重复表示 | 低 | 直接修 |
| S-D6 | `sdk lib.rs:119,129` — `Message::assistant_tool_calls`/`tool_result` | 无任何调用点（sdk 测试只构造 `Message::new`；agent 侧用自有 `AgentMessage` 同名方法） | 低 | 直接修 |
| S-D7 | `sdk lib.rs:558` — `StreamAccumulator::set_protocol` | 只有测试消费者（providers.rs:268-286 cfg(test) wrapper）；生产路径经构造器初始化 | 低 | 直接修（或保留为扩展 API，需裁定） |
| S-D8 | `sdk lib.rs:264,271` — `Response::text()`/`reasoning()` | 只有 sdk 内部测试消费者；生产走 `from_sdk_response` 逐 block 转换；agent 侧有平行 `AgentResponse` 方法 | 低 | 直接修 |
| S-D9 | `sdk lib.rs:182` — `ToolDefinition.requires_approval` | 只写不读：唯一写入点 providers.rs:319；三个 adapter 的 `encode_request` 均不读取；审批判断用 agent 侧 `AgentToolDefinition` | 中 | spec（SDK 边界语义，需确认未来 provider 是否会用） |
| S-D10 | `crates/crossh-update/src/model.rs:25,37` — `UpdateManifest.published_at`/`UpdateArtifact.signature` | 从未被生产读取；两字段只在测试中置 `None`；manifest 生成脚本也不输出 | 中 | 决策（删字段，或真正启用签名验证） |
| S-D11 | `crates/crossh-update/src/model.rs:210` — `update_result_path` | 无外部消费者；内部实现走 `pub(crate) update_result_path_in`；lib.rs:17 导出 | 低 | 直接修（改 pub(crate)） |
| S-D12 | 根 `Cargo.toml:28` — `assets`（zed git）依赖 | 只有 `tests/visual_capture.rs:27`（`visual-tests` feature）引用；src/ 生产代码零引用。即上轮 C-4，仍未修复 | 中 | 直接修（移入 [dev-dependencies]，与 B-6 联动） |
| S-D13 | `release.yml:19-69` validate job vs `ci.yml:11-41` check job | 两 job 重复执行 fmt/check-architecture/clippy/全量测试/锁文件校验，无差异化 step | 低 | 决策（刻意冗余则注释说明，否则删重复项） |
| S-D14 | `release.yml:45-55` 内嵌 awk 版本解析 vs `scripts/release.sh:67-79` | 同一份版本提取逻辑两处独立维护；与 package.sh/package-linux.sh/package-windows.ps1 的 grep 方式又重复 | 低 | 直接修（抽公共脚本） |
| S-D15 | `scripts/package-windows.ps1:37-62` vs `scripts/copy-shared-assets.sh:10-34` | PowerShell 内嵌了 copy 脚本全部逻辑（Zed checkout 前缀查找 + 字体/图标拷贝 + manifest.json 生成），两处独立维护 | 低 | 决策（保持跨语言重复，或 CI 预生成资产包） |

## 二、crossh-ssh / crossh-core（审阅 B）

| 编号 | 位置 + 符号 | 消费者证据 | 严重度 | 处置 |
| --- | --- | --- | --- | --- |
| S-B1 | `crates/crossh-core/src/terminal/session.rs:4,23` — `InputCmd`/`SessionEvent` 死契约（上轮 C-2） | 生产零消费；全部使用点在 crossh-ssh connection.rs 的 cfg(test) 终端基础设施（open_terminal_channel/relay_terminal/drive_input/detect_remote_shell）+ 3 个 `#[ignore]` 真实主机测试。**约束**：同文件 `TerminalProcessInfo` 是生产符号（被 title.rs 消费），不能删；删除范围会移除测试资产，且与 draft spec `20260817-ssh-hermetic-loopback.md` 契约 5 重叠 | 中 | spec/决策（与 loopback spec 联动） |
| S-B2 | `crates/crossh-ssh/src/session.rs:11,22-23` — `AuthChoice::Password` 变体 | 零构造点：仅定义处与 connection.rs:590-598 唯一 match 分支；`default_auth_for` 只产出 Key/Agent；真实密码认证走兜底 `request_credential`，不受删除影响。allow(dead_code) 即为压制 never-constructed 警告 | 低-中 | spec（删变体 + 分支 + re-export 注释） |
| S-B3 | `crates/crossh-ssh/src/sftp.rs:25` — `#[allow(dead_code)]` | 过期豁免：`SftpCmd` 全部 8 个变体均有生产构造点（sftp/view.rs:240-747）；SftpEvent 变体全部被生产匹配 | 低 | 直接修（删 allow，编译验证） |
| S-B4 | `shell.rs:332-404`（生产）vs `connection.rs:740-762`（测试版 bootstrap，上轮 C-3） | 两份独立实现同一意图，但命令语义有实质差异：bash 测试版用进程替换、生产版写 temp 文件 + `--rcfile`（shell.rs:392-399 注释明确论证进程替换式嵌套引号会被远端 /bin/sh 误解析）；zsh/fish 加载策略也不同 | 中 | spec（提取 per-shell 生成函数统一，属行为变化，须与 loopback 落地联动） |

已验证无问题：crossh-ssh 全部公共 re-export 有生产消费者（entity.rs/prompt.rs/manager.rs/forwarding/view.rs）；`run_connection` 的 select! 生命周期、连接池 WeakEntity、known_hosts 决策链路（engineering-note 防御模式）未触碰；crossh-core commands.rs 24 个符号、ssh_config 字段、git 解析器全部活跃。

## 三、根 crate（审阅 A）

| 编号 | 位置 + 符号 | 消费者证据 | 严重度 | 处置 |
| --- | --- | --- | --- | --- |
| S-A1 | `src/features/sftp/logic.rs:72-111`（backspace/delete/move_cursor_horizontal）vs `src/shared/text_editing.rs:71-120` | 同一事实两份表示：无选区退格/删除/横向移动语义逐行相同；sftp 已 use shared 的边界函数；差异在选区/IME 支持 | 中 | spec（合并，行为契约变更） |
| S-A2 | `src/agent_cli.rs:45-49` — `#[allow(unused_imports)]` 覆盖 6 个 import | 6 个符号仅测试使用（agent_cli_tests.rs:152-218 经 `use super::*`）；3 个有生产使用 | 低 | 直接修（import 移到测试文件） |
| S-A3 | `src/features/workspace/toaster.rs:5` — `ToastTone::Info`/`Warning` 无生产构造点 | 生产只构造 Success（view.rs:398、shell.rs:1430）与 Error（shell.rs:1447）；Info/Warning 仅测试构造 + toaster_view.rs 映射。但 ADR 0013 决策 4 已将四语气定为 toaster 契约，变体**保留** | 低 | 直接修（删过期 allow 与"future"注释，注明契约） |
| S-A4 | `src/features/workspace/pane.rs:39,42,43` — `WorkspacePane` 3 个方法无默认实现 | 4 处空 stub（terminal/view.rs:1003、forwarding/view.rs:149、sftp/view.rs:205,207）；trait 中其余 3 个方法已用默认空实现模式 | 低 | 直接修（3 方法补默认空实现，删 4 处空 stub；语义等价） |
| S-A5 | `src/features/git/window.rs:52,647-658` — `_refresh_task` | 字段只用于"已启动"标记（init/is_some 防重入/赋值），永不置回 None、从不读结果；Task 作用仅为随实体 drop 取消 | 低 | 决策（保持设计但补注释，或简化为 bool） |
| S-A6 | `src/features/sftp/logic.rs:10,210-212` — `SFTP_CHANNEL_UNAVAILABLE` const | 生产使用仅 try_send_command 一处 map_err；与 i18n 文本 `sftp_channel_unavailable()` 是同一事实两份表示 | 低 | 直接修（内联字面量，收敛单一事实） |
| S-A7 | `src/app/mod.rs:7-16` — `LaunchTarget` 单变体枚举 + `open_launch_target` 薄包装 | 唯一变体 Main；open_launch_target 只是映射到 open_main_window；消费点 main.rs:91,99-107,125 | 低 | 决策（扩展点则补注释，否则塌缩） |
| S-A8 | `src/features/commands/`、`src/features/projects/`、`src/shared/terminal/`、`src/shared/ui/` | 4 个空目录，无文件无引用（git 不跟踪，仅本地残留） | 信息 | 直接修（删本地残留目录） |
| S-A9 | `src/features/terminal/view.rs:149` — `TerminalViewEvent` 单变体 | 有监听者（emit view.rs:932；订阅 tabs.rs:145、shell.rs:421），非死代码；单臂属预留扩展形态 | 低 | 决策（保留并注释扩展意图，或塌缩为方法调用） |
| S-A10 | `src/features/workspace/command_editor.rs:30-60`、`src/features/git/editor.rs:18-54` — 薄委托层 | 全部方法有生产调用点，无死代码；与 `TextEditingState` 直接 pub 字段形成两种封装风格 | 低 | 决策（随 S-A1 的 spec 一并设计，不单独行动） |

已验证无问题：git/mod.rs 5 处 allow 是 visual-tests feature 的跨编译单元必要豁免（唯一消费者 tests/visual_capture.rs:355-448，生产编译单元无调用者）；git_launcher.rs 3 处 allow 是双 bin 挂载的必要豁免（print_cli_help/spawn_git_process 均有生产消费者，属 ADR 保护边界）；text_editing.rs:41 的 `clear` 有生产消费者但仅在 git bin 编译单元（根 crate 编译单元确实无调用，豁免必要，注释中"设置输入框"措辞不准确，建议修正）；全部 cx.subscribe/observe 有对应 emit；settings 键均有读取方；无悬空 Task。

## 四、crossh-ui / crossh-ui-component / crossh-terminal / crossh-theme / crossh-assets（审阅 C）

| 编号 | 位置 + 符号 | 消费者证据 | 严重度 | 处置 |
| --- | --- | --- | --- | --- |
| S-C1 | `crates/crossh-terminal/src/events.rs:7` — `#[allow(dead_code)]` | 过期豁免：`TerminalEvent` 7 个变体全部有生产 emit（terminal/view.rs:422-464）与消费（tabs.rs:125-142、shell.rs:384-418） | 低 | 直接修（删 allow 与注释） |
| S-C2 | 同 S-A3（toaster.rs:5） | 与 S-A3 合并：allow 过期，ADR 0013 已定四语气契约 | 低 | 直接修 |
| S-C3 | `crates/crossh-ui-component/src/separator.rs` 整个模块 — `Separator`/`SeparatorOrientation`/`horizontal`/`vertical`/`orientation` | 全仓库无任何构造调用（仅 lib.rs re-export + 自身 cfg(test)）；注意 `MenuEntry::Separator` 是无关的独立变体；architecture.md:56 将 separators 列为组件库内容 | 低 | spec/决策（删除模块并同步 architecture.md，或安排真实消费者） |
| S-C4 | `crates/crossh-ui-component/src/layout.rs:11` — `v_flex()` | 无任何消费者（唯一出现是定义 + lib.rs:38,58 re-export）；同模块 h_flex/scroll_y 活跃 | 低 | 直接修（删函数 + re-export） |
| S-C5 | `button.rs:22-23,80-100` — `ButtonVariant::Info/Warning/Success` | 全仓库（生产+测试）零构造；生产只用 Default/Primary/Secondary/Ghost/Danger/Link；是 style() 完备 match 分支所以无警告 | 低 | 决策（删需同步改 style() 与主题色，或明确作为预留记录） |
| S-C6 | `crates/crossh-ui-component/src/lib.rs:32-51` — `pub mod prelude` | 全仓库无 `use crossh_ui_component::prelude`；外部消费方全部从 crate root import | 低 | 直接修（删 prelude）或决策保留（生态惯例） |
| S-C7 | `src/features/workspace/registry.rs:88,100` — `_toast_task` | 只写不读；下划线表明有意保存 Task 句柄防 drop 取消 | 低 | 决策（改 `.detach()` 并删字段，或保留并补注释） |
| S-C8 | `split_resizer.rs:16-22,70-84` — `SplitHandleSide`/`handle_side()`/`handle_left()`/`line()` | SplitResizer 本身 4 处生产使用但全部用默认 Right + min/max_width；该枚举与方法无外部构造 | 低 | 决策（收紧 API 固定右侧手柄，或保留） |

已验证无问题：TabStrip/TabItem/StatusBar/StatusMetric/Toast 全部有生产消费者；IconName 37 变体与 37 个 SVG 一一对应、无孤儿图标；theme 22 token 与 ui 15 尺寸常量全部有消费；TerminalSettings/TerminalTimestampState 活跃；BadgeTone/AvatarKind/ButtonSize/ShellMenuAction/SftpMenuAction 全变体有构造+消费；根 Cargo.toml 除 S-D12 外无其它依赖冗余。

## 处置 Backlog

按优先级排序，处置 = **直接修**（豁免清单内或小型清理）/ **spec**（走 SDD）/ **决策**（需人裁定）。

| 优先级 | 编号 | 问题 | 处置 | 说明 |
| --- | --- | --- | --- | --- |
| P0 | S-D3 | SDK 迁移兼容层整组死代码 | 直接修 | 收益最大的一刀：删 2 类型 + 3 函数 + 导出，测试改直连 SDK |
| P0 | S-D2, S-D4 | 便捷入口无消费者 | 直接修 | 各删一个函数与导出 |
| P0 | S-D12 | 根 assets 依赖冗余（C-4） | 直接修 | 移入 dev-dependencies，与 visual_capture 决策联动 |
| P1 | S-B1, S-B4 | 死终端契约 / 双 bootstrap（C-2/C-3） | spec | 均与 loopback spec 联动，删除/合并是行为变化 |
| P1 | S-B2 | AuthChoice::Password 零构造变体 | spec | 公共 API 变更 + 删不可达分支 |
| P1 | S-A1 | sftp 字符编辑与 text_editing 重复 | spec | 行为契约变更 |
| P1 | S-D1 | providers 非流式入口链 | spec | 收缩非流式入口 |
| P1 | S-D9 | ToolDefinition.requires_approval 只写不读 | spec | SDK 边界语义，需裁定 |
| P2 | S-D10, S-D13, S-D15, S-A5, S-A7, S-A9, S-A10, S-C3, S-C5, S-C7, S-C8, S-D7, S-D11 | 契约/风格类决策 | 决策 | 每项二选一，随缘推进 |
| P3 | S-B3, S-C1, S-A2, S-A3, S-A4, S-A6, S-A8, S-C4, S-C6, S-D5, S-D6, S-D8, S-D14 | 过期豁免与死配置清理 | 直接修 | 低风险批量清理 |

## 与 SDD 工作流的衔接

- **直接修类**（P0 + P3）：不需 spec，遵守既有检查（fmt/architecture/clippy/test）。其中 S-D3 建议独立小 PR（删除 + 测试迁移）。
- **spec 类**（P1）：S-B1/S-B4 建议并入或联动 `docs/specs/20260817-ssh-hermetic-loopback.md`（该 spec 已声明"重建后可以再次打开终端或执行命令"，与本候选重叠）；S-A1 单独一份；S-B2/S-D1/S-D9 各一份短 spec。
- **决策类**（P2）：建议在下一轮评审中逐项裁定，裁定结果补 ADR 或直接修。

## 受保护表面确认

本轮未触碰且确认有消费者的受保护设计：logic/UI 分层（纯逻辑 crate 零 GPUI）、ADR 0005/0008/0009 的三个独立二进制、ADR 0013 toaster 契约、check-architecture.sh 白名单、SSH 生命周期防御模式、固定 Lucide/GPUI revision。各候选的"保留理由"已在上表列明，供后续独立复核。

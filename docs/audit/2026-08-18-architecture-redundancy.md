# Crossh 整体架构冗余分析报告（2026-08-18）

触发原因：用户指出简化审计的核心应是「整体设计能否简化」（300 行能实现的代码是否写了 2000 行），而非只找死代码边角料；随后选择执行整体架构冗余分析。

扫描方式：三个并行 subagent 分片审计（crate 边界与依赖图 / 应用层 feature 与 trait 抽象 / 跨层重复表示），每个分片要求证据精确到文件:行号；主会话对关键发现（low_latency 残骸、crossh-ai-sdk 消费者、action 变体）逐一复核验证。意图基线：`docs/architecture.md`、全部 ADR 0001-0014、AGENTS.md 工程规则、上一轮 `2026-08-18-simplification-audit.md`。

## 总体结论

**整体架构适度划分、接近 KISS，但存在 1 个明确的结构性过度划分点（S-1，三个分片独立指向同一处）和 1 处行为删除后遗留的完整残骸（S-2）。** 其余发现均为中等/低等维护信号。工作树含未提交的 `shell.rs`/`agent_cli.rs` 拆分重构（`shell_render.rs`、`agent_cli_input.rs` 为 untracked），行号以当前工作树为准。

架构健康的证据：10 个 crate 中 8 个有 ≥2 个消费者面或多二进制复用（core 25+ 文件、ssh 12、ui 30+、component 14）；`crossh-theme`（117 行零依赖）是「跨渲染表面共享 token」的正确范例；`crossh-update`/`crossh-agent`/`crossh-ui-component` 受 ADR 0005/0003/0010/0013 保护；无空壳 crate。应用层 traits、事件枚举、订阅、WeakEntity、settings 键经复核几乎全部有真实消费者。

## 发现

| 编号 | 问题 | 严重度 | 证据与消费者结论 |
| --- | --- | --- | --- |
| S-1 | `crossh-ai-sdk` 与 `crossh-agent` 全量类型镜像 + 6 个零变换转换函数，crate 边界疑似过度划分 | 高 | 唯一跨包生产消费者是 `crates/crossh-agent/src/providers.rs:10` 与 `tests.rs:2`，src/ 与根 Cargo.toml 零直接引用。10 个类型逐一镜像：`AgentRole`↔`Role`（messages.rs:4-9 ↔ lib.rs:65-70）、`AgentMessage`↔`Message`（字段含 serde 注释完全一致）、`AgentToolCall`↔`ToolCall`、`AgentToolResult`↔`ToolResult`、`AgentContentBlock`↔`ContentBlock`、`AgentEvent`↔`Event`、`AgentResponse`↔`Response`、`AgentToolDefinition`↔`ToolDefinition`（tools.rs:38-44 ↔ lib.rs:132-154）、`AgentProtocol`↔`Protocol`（policy.rs:22-47 ↔ lib.rs:16-25，含相同 serde rename）、`AgentThinkingLevel`↔`ThinkingLevel`（policy.rs:49-95 ↔ lib.rs:27-63，含相同 budget 魔数）。providers.rs:84-212 维护 6 个逐字段拷贝胶水：`to_sdk_protocol`/`to_sdk_thinking`/`to_sdk_message`/`from_sdk_response`/`from_sdk_event`/`sdk_tool_definitions`。serde rename 双处手动同步（policy.rs:25-29 ↔ lib.rs:19-24），单边漂移会静默破坏 wire 格式。预计净减约 300-400 行。 |
| S-2 | Low-latency shell input：行为已删除、seam 全量残留（永远禁用的用户可见菜单） | 高 | 2026-08-07 `dcec8e5` 实现真实行为，fork 重构 `542eb3e` 删除行为与 `terminal/input.rs` 但未清理 seam。残骸链：`pane.rs:9-10,29-32`（trait 方法 + 两个恒 false 字段）；`terminal/view.rs:1025-1026,1042`、`sftp/view.rs:180`、`forwarding/view.rs:125`（三个空 impl）；`shell.rs:1040,1086-1097`（分发到 no-op）；`tab_strip.rs:170-204`（`disabled: !low_latency_available` → 永远禁用的菜单项）；`crossh-ui/src/context_menu.rs:53`（`ShellMenuAction::ToggleLowLatencyShellInput(usize)`，唯一构造点同上）；`locales/en.yml:189`、`zh-CN.yml:189`（i18n key）。全仓 `rg low_latency` 仅剩上述残骸。属「行为性零消费者」，违反 AGENTS.md「No backward-compatibility bloat」。 |
| S-3 | `host_entry_matches` 逐字重复 | 中 | `sidebar.rs:33-36`（调用点 sidebar.rs:87）与 `shell.rs:1625-1629`（调用点 shell.rs:879）为逐字相同的私有函数。侧栏本就是主机列表 + 搜索的 owner（sidebar.rs:655 持有 `local_dir_matches_query`），可上移收敛。 |
| S-4 | 人类可读字节大小格式化双份 | 中 | `sftp/logic.rs:154-167` `format_size`（含 GB，调用点 render.rs:98,322-323）与 `crossh-ssh/src/sftp.rs:197-207` `format_bytes`（无 GB，调用点 sftp.rs:162-163,183-184）。纯函数，应下沉到逻辑 crate 共享。 |
| S-5 | 当前 Unix 时间戳双份 | 中 | `crossh-agent/src/session.rs:619-622` `unix_millis()`（调用点 35,50,55,105）与 `crossh-core/src/commands.rs:377-382` `unix_timestamp()`（命令历史 `last_used`）。单位不同但可在同一 crate 提供两个 5 行函数。 |
| S-6 | settings.toml 读取回退逻辑与常量双份 | 中 | `src/features/settings/persistence.rs:14,25-37,69-89` 与 `crates/crossh-agent/src/config.rs:14,16-20,26-44`：`SETTINGS_FILE_NAME` 常量双定义 + 完全相同的「文件缺失→默认、解析错误→默认+warn」回退语义。CLI 独立读盘受 ADR 0009 保护（独立二进制不能依赖 GPUI 侧 persistence），但读盘逻辑与常量可复用 `crossh_agent::load_agent_settings` 导出的段落加载器。 |
| S-7 | 系统提示硬编码工具名单与 `builtin_tools()` 双源 | 低 | `agent_cli.rs:1394` 内嵌 `"Available tools: read, grep, find, ls, patch, edit, write, bash."`，权威定义是 `crates/crossh-agent/src/tools.rs:46` 的 `builtin_tools()`（CLI 的 `/tools` 展示 agent_cli.rs:1000-1001 已消费真实列表）。漂移风险：新增工具需手改两处。反方：prompt 字节需要精确措辞控制，自动拼接会改变模型输入；当前可接受，报告为维护信号。 |
| S-8 | 路径越界防护双实现（不同强度） | 低 | `crates/crossh-agent/src/tools.rs:1010-1047` 加固版（ParentDir 拒绝 + canonicalize + starts_with + 40 跳符号链接逐跳解析，1049-1074）与 `agent_cli.rs:1430-1445` 弱版（无符号链接逐跳解析）。威胁模型不同（工具路径是批准后写操作边界 vs `@file` 引用内联 prompt 且最大 32 KiB），不建议强制折叠；建议两处注释互相引用说明强度差异，避免后人误判。 |
| S-9 | `crossh-assets` 疑似单消费者 crate | 低 | 跨包生产消费者仅 `crossh-ui`（assets.rs:5,7 / icons.rs:3,10），src/ 零直接引用（main.rs:98,121 与 bin/crossh-git.rs:46,51 经 `crossh_ui::assets::UiAssetSource` 间接消费）。ADR 0008 保护的是共享**发布目录** `crossh-assets/`（文件系统层面），不构成对 crate 边界的保护。反方：rust-embed 注入与 IconName 完整性测试独立于 GPUI 依赖树，独立 crate 是「存储层可被任意二进制按需挂接」的最廉价表达；当前无任何非 GPUI 消费者，属纯防御性保留。裁定：保留，记录。 |
| S-10 | `crossh-terminal` 与 `crossh-core::terminal` 双边界 | 低 | `src/features/terminal/view.rs:32-44` 被迫同时 import 两个 crate 的 terminal 事实（`crossh_core::terminal::{...}` + `crossh_terminal::{settings, timestamps, events}`）；`crossh-terminal/events.rs:4` 穿层 re-export `crossh_core::connection::ConnectionState`。跨包消费者 = 0（6 个消费者全在 app 内）。反方成立：crossh-core::terminal 的 6 个消费者里 5 个是非 terminal feature（title/shell helper），crossh-terminal 是 feature 自身逻辑边界（ADR 0004 精神），且 core 已 5923 行，合并会搅浑 feature/共享界线。裁定：保持现状可辩护，记录。 |

## 处置 Backlog

| 优先级 | 编号 | 建议处置 | 说明 |
| --- | --- | --- | --- |
| P1 | S-1 | 扩展/新增 SDD spec | 已有 `docs/specs/20260818-remove-sdk-tool-approval-field.md`（draft）处理 requires_approval 字段；本报告提供了更强证据：问题不在单个字段，而是整个 sdk/agent 边界的全量类型镜像与转换胶水。建议将 spec 范围扩展为「SDK 边界裁定」：删除镜像类型让 agent 直接消费 sdk canonical 类型（JSONL 形状不变、字段名相同、无迁移成本），或在有真实第二消费者计划前接受折叠。涉及两个 crate 的归属，需同步 `architecture.md:49` 的 canonical messages 归属声明。 |
| P1 | S-2 | 写短 spec 后删除 | 删除面：pane.rs 方法+字段、三处空 impl、tab_strip.rs 菜单块（170-204）、shell.rs 分发分支（1040,1086-1097）、crossh-ui action 变体（context_menu.rs:53）、locales 两个 key。用户可见影响（永远禁用菜单项消失）属行为变更，走 SDD；这符合仓库「无 shim、无 just-in-case 兼容」纪律，与 S-A9 等既有判例一致。 |
| P2 | S-3 | 直接修 | 与进行中的 shell.rs 拆分重构同批做：收敛为 sidebar 的 `pub(super)` 或与 `HostEntry` 同层（connections/host.rs）。 |
| P2 | S-4 | 直接修 | 下沉到任一逻辑 crate（如 crossh-core）共享；引擎版补 GB 分支后由 UI 复用。 |
| P2 | S-5 | 直接修 | 在 crossh-core 提供 `unix_secs`/`unix_millis`，删除 agent 侧副本。 |
| P2 | S-6 | 直接修 | 复用 `crossh_agent::load_agent_settings` 的段落加载器（CLI 独立读盘保留）；或共享常量。 |
| P3 | S-7 | 维护信号 | 若再增工具建议改为由 `builtin_tools()` 生成一行名单；当前保留精确措辞。 |
| P3 | S-8 | 注释互引 | 在两处防护实现互相引用并说明强度差异；不强制折叠（威胁模型不同）。 |
| 保留 | S-9 | 不处理 | 纯防御性独立；若未来确认无无 GPUI 消费者计划，可评估折叠进 crossh-ui。 |
| 保留 | S-10 | 不处理 | 合并反方论证成立；记录双 import 痛点为已知成本。 |

## 与 SDD 工作流的衔接

- S-1：扩展现有 draft spec（`20260818-remove-sdk-tool-approval-field.md`）或另立「SDK 边界折叠」spec，AI 评审 + 人批准后再动。协议测试需证明折叠后 JSONL 序列化形状与 wire 请求不变。
- S-2：新建短 spec（行为删除，用户可见菜单消失），与 S-1 可并行评审。
- S-3 至 S-6 属豁免清单内的小型合并/去重，可直接修（S-3 建议与 shell.rs 拆分重构同批）。

## 已确认干净 / 有意保留（避免重复排查）

- `WorkspacePane` trait（ADR 0007）：除 `toggle_low_latency`（S-2）外 14 个方法均有生产调用点（view.rs:165、tab_strip.rs:172,441、split.rs:142,179,210,216,230、tabs.rs:240,302,304,725、shell.rs:118,311,647,649,705,1111,1203,1642、quit.rs:148,151,191、notifications.rs:38,43,71）。
- 事件枚举、订阅、WeakEntity：`TerminalEvent` 7 变体全部有 emit+消费；`cx.subscribe` 7 处监听均有 emit；WeakEntity upgrade 全部有消费（连接池弱引用是刻意设计，manager.rs:16-18 有注释）。
- settings 键全部有读方（workspace 5 键、terminal 4 键、updates、language、`[agent]` 整段）——没有「无读方的键」。
- 连接状态 `ConnectionState`（crossh-core/src/connection.rs:5）单源直用；Git 类型、更新模型、IconName 均单源。
- SharedString/String：全仓 SharedString 均出现在渲染层边界转换，状态字段一律 String，未发现同字段双层异型。
- `ToastTone::Warning` 预留有 ADR 0013 第 4 条与 spec 契约 40 背书（与 S-C5 裁定一致），非冗余。
- `ForwardTracker` 与引擎注册表弱镜像：引擎只回传启停结果（oneshot），UI 必须自持乐观渲染副本——是「协议缺事件」而非冗余；消除需引擎加状态事件流，属行为变更，保留。
- registry/shell/tabs/sidebar 无镜像状态：`LocalDir.active_session` 与全局 `active_view` 语义不重叠；host 列表唯一源 `ConnectionManager.entries`。

## 本轮未决项

- 未重新运行 Clippy/workspace 测试（提权审批服务此前拒绝过执行请求）；死代码结论基于静态引用核验与三个独立分片交叉验证。
- S-1 的 spec 范围裁定（扩展现有 spec vs 新立 spec）待用户/评审决定。
- 报告行号基于含未提交拆分重构的工作树；若重构合入后行号漂移，以符号搜索为准。

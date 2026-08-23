# Crossh 简化扫描报告（2026-08-23，代码取证版）

触发原因：用户手动触发简化扫描，并明确要求**以当前代码为唯一真相，不采信历史审计文档的结论**。本报告覆盖同日早前一版基于历史文档推演的报告，全部结论重新取证。

扫描方式：意图基线 `AGENTS.md`、`docs/architecture.md`、ADR 目录标题（仅用于识别受保护表面，不作证据）。分片并行扫描四域（根 crate features/shared/bin、crossh-core+crossh-ssh、UI 五 crate + 视图层、agent/ai-sdk/update/terminal/settings/scripts/Cargo.toml），每条候选由主线程回到源码逐项复核消费者计数后才收录。

锚点命令（主线程亲跑）：

- `cargo clippy --workspace --all-targets`：**零 warning、零 error**。
- 全仓 `#[allow(dead_code|unused)]`：仅 9 处（`git_launcher.rs×3` 双 binary 挂载、`features/git/mod.rs×5` visual-tests、`toaster.rs×1` ADR 0013、`crossh-tui/ansi.rs:550` unused_assignments），均为已注释的必要豁免。
- 根 crate 无 `lib.rs`（纯 bin），dead_code lint 全覆盖；库 crate 的盲区（`pub` 跨 crate 表面）由全仓 grep 消费者计数补齐。

## 总体结论

**不是"大量"历史遗留**：编译器锚点干净，主体（shared 逻辑层、connections 连接池、forwarding tracker、git/sftp 输入层、theme token、图标、terminal 事件、update 链路、scripts/CI 引用图）经逐一核验均为活代码。真正的历史遗留集中在**三坨**，均可归因到明确的演进节点：

1. **ADR 0015 引入的 agent 运行时层未接线**（最大一块）：生产路径 `src/agent_cli*` 绕过 `AgentSessionRuntime`/`SessionManager` 抽象直调 `session.rs` 自由函数，导致 runtime.rs + manager.rs 整体仅测试可达；`EventBus` 只有 emit 半边，6 个事件变体从未 emit 也从未 match。
2. **crossh-ui-component 的"P0-2 建议形态"结构化 API 批次**：一批只有定义与自测、无任何 feature 视图调用的投机封装（ListPane/SelectableRow/ModalTextInput/SplitResizer 对称 API 等），部分源码注释自认无外部消费。
3. **少量重复表示与死符号**：git 状态 metric 链双实现、`queued_inputs` 影子队列双记账、根 Cargo.toml 两个零引用死依赖等。

## 发现

| 编号 | 问题 | 严重度 | 证据与消费者结论 |
| --- | --- | --- | --- |
| S-1 | 根 Cargo.toml 死依赖 `ratatui`(:62) 与 `tui-markdown`(:67) | 高 | 主线程复核：`grep -rn "ratatui\|tui_markdown" src crates tests --include='*.rs'` = **0 命中**；仓库无 build.rs。消费者分类：无。反方论点：可能为 agent_cli TUI 迁移预留——但零引用即纯编译成本。 |
| S-2 | `crossh-agent` 运行时抽象整体仅测试可达 | 高 | `AgentSessionRuntime`(runtime.rs:22)、`AgentSessionServices`(runtime.rs:11)、`SessionManager`(manager.rs:18)、`FsSessionManager`(manager.rs:31)、`InMemorySessionManager`(manager.rs:57)：全仓外部消费者=0（主线程复核，仅剩 `lib.rs:41-42` re-export）。生产端 `src/agent_cli.rs:19-25`、`src/agent_cli_session.rs` 直调 `session.rs` 自由函数 create/load/save/list/latest_session。分类：仅测试。**这是 ADR 0015 裁决过的边界，但"生产绕过该层直调自由函数"是结构性新证据，需裁定而非静默清理。** 反方论点：可嵌入 GUI 的 Runtime 隔离层是既定设计，未来 GUI 内嵌 agent 时需要。 |
| S-3 | `EventBus` 只有 emit 半边接线 | 高 | 主线程复核 `event.rs` 全文：`subscribe()` 唯一调用点是同文件 `#[cfg(test)]`(:141,:165)；生产只 emit 不 listen。9 个变体中 `AgentEnd/AgentSettled/EntryAppended/SessionInfoChanged/ThinkingLevelChanged/ModelChanged` 从未 emit 也从未 match；`QueueUpdate/CompactionStart/CompactionEnd` 有生产 emit（src/agent_cli.rs:594,643,687,706,724）但落空。分类：emit 生产 / subscribe 仅测试。与 S-2 同属 ADR 0015 层，一并裁定。 |
| S-4 | Git 状态 metric 七段链双实现 | 中高 | `src/features/git/render.rs:251-285` 与 `src/features/workspace/view.rs:615-639` 为主线程逐行比对确认的同构实现（↑/↓/+/~/?/!/clean → `StatusMetric` + 相同 `BadgeTone` 映射），任一处改 tone/阈值即静默漂移。分类：生产×2。反方论点：两处上下文容器不同，但不影响 metric 链抽成共享纯函数。 |
| S-5 | `queued_inputs` 影子队列与 `MessageQueue` 双记账 | 中 | 主线程复核 38 处触点：`src/agent_cli.rs:147` 字段、入队恒成对双写（agent_cli_input.rs:14,37 ↔ queue.push_* :13,:36）、drain 路径按值字符串反查手工同步（agent_cli.rs 多处、agent_cli_input.rs:65-68）、`agent_cli.rs:583` `.or_else(\|\| app.queued_inputs.pop_front())` 兜底——两份表示已经需要启发式对齐，即漂移证据。分类：生产。建议以 `MessageQueue` 为单源删除影子队列。反方论点：process_prompt 取消时的独立缓冲依赖，可由 `restore_to_input` 覆盖。 |
| S-6 | crossh-ui-component 投机 API 批次（8 项，仅定义+自测） | 中 | 全部经 scout 全仓计数 + 抽样复核：`ModalTextInput/ModalDialogActions`(modal_field.rs:74,104)、`ListPane/PaneFrame`(list_pane.rs:36,45)、`SelectableRow` 结构体形态(selectable_row.rs:33)、`SplitResizer::{handle_side,handle_left}`(split_resizer.rs:76-89) 及 `SidePanel` 同名方法(panel.rs:148-156)——后者源码注释**自认**"除便捷写法外无直接外部消费点"；`danger_banner/warning_banner`(banner.rs:276-283)、`RAIL_AVATAR_PITCH`(panel.rs:41)。分类：非生产（仅模块内测试）。视图实际只走函数式入口（list_pane()/selectable_row()/Banner::new().tone()）。建议整批裁定：删或降 `#[cfg(test)]`。反方论点：P0-2 形态批次若仍规划迁移则保留接口。 |
| S-7 | 枚举死变体 3 组 | 中 | `ButtonVariant::{Info,Warning,Success}`(button.rs:31-33)：全仓构造 0，style() 存在永不可达 match 臂；`AvatarKind::Host`(avatar.rs:11)：唯一"构造点"是 `sidebar.rs:315-330` 整段被注释的主机 rail 代码；`BannerTone::Info`(banner.rs:26)：生产构造 0，仅测试 assert_ne 提及。分类：非生产。反方论点：tone 三分支对称性是组件契约；主机 rail 属规划内功能（见 S-8 同根因）。 |
| S-8 | sidebar 主机活跃头像尸体代码 | 低中 | `sidebar.rs:274-280` 计算 `active_remote_key` 后唯一消费者是被注释的 `:315-330` 代码块，`:333 let _ = &active_remote_key;` 压制 unused。分类：生产函数内的死计算。与 S-7 的 `AvatarKind::Host` 同根因，应一并裁定恢复或删除。 |
| S-9 | `TerminalProcessInfo` seam 生产恒 None | 中低 | `crates/crossh-core/src/terminal/session.rs:6` 定义；唯一生产调用方传 `None`（title.rs:37 local_terminal_title 的 process 参数）；`process_display_name`(title.rs:241) 仅测试激活。整个 session.rs 模块仅含此类型。分类：投机 seam。反方论点：进程信息注入是终端标题的自然扩展点，成本为一个 Option 参数。 |
| S-10 | `compose_bar.rs` line_bounds 死副本 | 低 | 主线程读源确认：`compose_bar.rs:15-22` 私有 fn 与 `sftp/logic.rs` 逐字等价；`:228 let _ = line_bounds; // 保留 helper 供未来扩展` 显式自认死代码。真身在 sftp/logic.rs 且有 3 处生产消费。分类：非生产。直接删。 |
| S-11 | `MessageQueue` 死方法 5 个 | 低 | 主线程复核：`pop_next`(:83)、`pop_steering`(:93)、`has_steering`(:100)、`pending_count`(:103)、`take_all`(:110) 全仓零调用（forwarding/view.rs 的 take_all 是另一类型同名方法）；`clear_queue`(:107) 是 take_all 的纯别名，仅 `agent_cli_input.rs:59` 一处使用。生产实际只用 push_steening/push_follow_up、is_empty、restore_to_input 与字段访问。反方论点：对齐 pi 的 clearQueue/popNext 语义是有意命名。 |
| S-12 | `CURRENT_SESSION_VERSION` 双常量零读取 | 低 | `entry.rs:6` 与 `session.rs:16` 两份定义，`ENTRY_VERSION` 别名再导出（lib.rs:39），全仓读取=0；实际写文件用私有常量（session.rs 写端 :220、读校验 :268）。两份重复常量有漂移风险。 |
| S-13 | 手写 IME/caret 单行渲染残留 ×4 | 低中 | 统一路径 `TextInput`(text_input.rs)/`ModalField`(modal_field.rs) 已存在，仍有手写残留：`compose_bar.rs:80-230`（多行场景，迁移有工作量）、`git/render.rs:1100-1161`、`settings/agent.rs:300-330`、`sftp/view.rs:1070-1095`。`sftp/view.rs:771-772` 注释自证同类迁移可行。分类：生产×4，随各 feature 变更顺带迁移，不单独立项。 |
| S-14 | 测试镜像 SDK 私有 wire 逻辑 | 低 | `crates/crossh-agent/src/providers.rs:71-116` 复制 SDK 内私有 `Utf8StreamDecoder`；`:100-190` 复制 `apply_model_options`/thinking 映射。全部 #[cfg(test)]。SDK wire 行为变更时测试可静默背离实现。反方论点：SDK 私有函数无法直接断言的权宜之计。 |
| S-15 | `verify_manifest_signature`（pinned-key 版）零调用 | 低 | `crates/crossh-update/src/signature.rs:25`：生产与 release.yml 均走 `verify_manifest_signature_with_key`(model.rs:163、bin:114)。合理 lib 面，信息级。 |
| S-16 | 杂项信息级 | 信息 | `Event::Stop(payload)` 产出后零读者（StreamAccumulator 忽略、应用侧 `=> {}`）；`AuthStyle::None` 仅 SDK 测试 CustomAdapter 使用（ProviderAdapter 扩展点的必要认证形态）；`config.rs:65` 测试 fixture 写入幽灵键 `terminal_show_timestamps`（serde 键名实为 show_timestamps，被静默丢弃）；`shell_quote`(core/terminal/shell.rs:426) 与 `shell_quote_remote`(ssh/connection.rs:704) 现均委托 shlex::try_quote，仅剩 5 行薄重复，收益不足以立项。 |

## 否决记录（scout 报告后被主线程复核推翻）

- ~~"`list_changes` 全仓零生产调用点，仅 #[cfg(test)] 消费"~~：**否决**。`crates/crossh-core/src/git_conflict.rs:46,74,90` 在生产冲突解析路径消费 `list_changes`（stash/conflict 操作是产品功能）。mod.rs 其余命中为测试。
- ~~"ProviderAdapter 是无人使用的预留扩展点"~~：排除。`Client::complete/stream` + `builtin_adapter`(ai-sdk/lib.rs:612) 完整内部消费链，三个内置 adapter 均经此路由。

## 处置 Backlog

| 优先级 | 编号 | 建议处置 | 说明 |
| --- | --- | --- | --- |
| P1 | S-1 | ✅ 已闭环（同日直接修） | Cargo.toml 死依赖 `ratatui`/`tui-markdown` 已删除，净减编译依赖。 |
| P1 | S-4 | ⛔ 结构性受阻，维持现状 | 落地时发现：`features/git` 仅经 `#[path]` 挂载于 `crossh-git` 独立二进制（ADR 0008），主二进制模块树中不存在 `features::git`，两处调用点在不同编译单元；`src/shared/` 禁 GPUI 类型，唯一共享点是把它推进 crossh-ui-component 并镜像 GitStatus 字段——以小结构体重复换整链重复属 ADR 级取舍。已回退合并尝试，双实现为拆分边界的结构性代价。 |
| P2 | S-2+S-3 | ✅ 已闭环（ADR 0015 修订） | 已删除 runtime.rs/manager.rs/EventBus 及全部落空 emit 点与死变体；生产直调自由函数确立为正式契约，详见 ADR 0015 修订节。 |
| P2 | S-5 | spec 认领 | 删除影子队列涉及时序语义（steer 注入时机、取消恢复路径），建议短 spec 以 `spec_20260821_agent_runtime_*` 测试为回归哨兵。 |
| P2 | S-6+S-7 | 决策（批次裁定） | ui-component 投机 API 批次 + 死枚举变体一并裁定：若 P0-2 形态迁移仍在规划，标注 owner 与目标 spec；否则整批删除（含不可达 match 臂）。 |
| P3 | S-8 | 随 S-7 顺带 | 主机 rail 若恢复则消费 `active_remote_key` 并解除 AvatarKind::Host；否则删 :274-280、:315-333。 |
| P3 | S-10 | ✅ 已闭环（同日直接修） | compose_bar line_bounds 死副本与 `let _ = line_bounds;` 自引用已删。 |
| P3 | S-11 | ✅ 已闭环（同日直接修） | pop_next/pop_steering/has_steering/pending_count/take_all 已删，take_all 内联进 clear_queue。 |
| P3 | S-12 | ✅ 已闭环（同日直接修） | entry.rs 与 session.rs 的公开 CURRENT_SESSION_VERSION、lib.rs 的 ENTRY_VERSION 别名均已删，session.rs 私有 SESSION_VERSION 为单源。 |
| P3 | S-9 | 随功能变更顺带 | 进程标题注入若有产品计划由 spec 接管；否则删 TerminalProcessInfo 与 None 参数通路。 |

## 执行记录（2026-08-23，用户授权全权处理）

- 删除：`crates/crossh-agent/src/runtime.rs`、`manager.rs` 整文件；event.rs 的 EventBus/AgentSessionEvent/Listener 与 2 个测试；MessageQueue 5 个死方法；agent_cli 三文件共 9 处落空 emit 与 event_bus 字段；Cargo.toml 两个死依赖；compose_bar 死副本；版本常量三份收敛为一份。净 -916 行。
- 门禁：`cargo fmt --check` 通过、`scripts/check-architecture.sh` 通过、`cargo clippy --workspace --all-targets` 零警告、`cargo test --workspace` 全绿（`spec_20260820_terminal_compose_bar__no_view_send_is_noop` 单次失败为 Zed 测试调度器检测 PTY 线程活动的既有抖动，隔离重跑 3 次及整 bin 重跑均通过）。
- 文档：ADR 0015 追加修订节、architecture.md 的 crossh-agent 条目同步。
- 未动：S-5 影子队列（需 spec）、S-6/S-7/S-8 P0-2 批次与主机 rail 尸体（需产品裁定）、S-9 TerminalProcessInfo seam、S-13~S-16 信息级。

## 受保护表面中"有意保留"项（本轮验证过，不入 backlog）

- `#[allow(dead_code)]` 全部 9 处豁免（git_launcher 双 binary、visual-tests fixtures、toaster NoticeLevel、ansi.rs 测试赋值）。
- `crossh-tui` 与 core/terminal 无镜像类型；同步输出管线（BEGIN_SYNC/OSC52）为移植契约。
- IconName 41 变体、theme 21 token、TerminalEvent/TerminalViewEvent、ShellMenuAction 抽查变体、widgets 7 helper：逐一验证均有生产消费。
- `crossh-ssh` run_connection select! 生命周期、known_hosts 决策链、Zed/GPUI 固定 revision、Lucide 1.27.0。

## 与 SDD 工作流的衔接

- 已直接修：S-1、S-2+S-3（经 ADR 0015 修订）、S-10、S-11、S-12；门禁全过（fmt / check-architecture / clippy 零警告 / cargo test --workspace 全绿）。
- S-4 经落地验证为 ADR 0008 二进制拆分的结构性代价，维持现状并回退合并尝试。
- 仍需 spec：S-5（影子队列删除，涉及时序）。
- 仍需决策：S-6+S-7（P0-2 批次去留）、S-8（主机 rail 恢复与否）。

## 本轮未决项

- S-6/S-7/S-8 的裁定需产品输入：P0-2 形态迁移与主机 rail 是否仍在路线图上，代码本身无法回答。
- 演进史说明：仓库 git 历史仅 8 个压缩提交，演进脉络以 specs/ADRs 为准；本报告按用户要求未将其作为证据。

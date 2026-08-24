# Crossh 简化扫描报告（2026-08-24）

触发原因：用户手动触发简化扫描（"发现历史残留"）。距上一轮（2026-08-23）一天，本轮定位为**增量轮**：验证上轮未决项现状、覆盖其后三个提交（b7117bc scrollback、6632793 clippy CI、937195b agent provider 合并）的残留、补扫上轮较薄的区域（i18n 资产、git 解析层字段级消费者、ui-component builder 级 API）。

扫描方式：

- 意图基线：AGENTS.md、docs/architecture.md、ADR 目录、engineering-notes 索引。
- 主线程锚点：`cargo clippy --workspace --all-targets` **零警告**（仅第三方 `block v0.1.6` 未来兼容提示）；全仓 `#[allow(dead_code|unused)]` 共 10 处，与昨日盘点一致（git_launcher×3 双 binary、features/git visual-tests×5、toaster Warning ADR 0013、ansi.rs:550 测试赋值），均有注释豁免理由。
- 并行分片四域：根 crate（ScanRoot）、core+ssh（ScanCore）、UI 五 crate+features/git 视图（ScanUI）、agent/ai-sdk/update/terminal/scripts/Cargo.toml（ScanAgentMisc）。
- 每条收录候选由主线程回到源码复核消费者计数；**ScanRoot 的 i18n 孤儿键清单经主线程全键精确核对后大幅修正**（见 T-3）。

## 总体结论

主体依然干净：connections/forwarding/settings/updates/shared/bin、agent_cli 三文件的 provider 合并残留（旧 vendor 名、protocol/url 字段引用）为零、crossh-terminal 全部符号、scripts 与 CI 引用图、Cargo 依赖表（含 [patch.crates-io]）、IconName 41 变体、theme 21 token、git 视图 15 个 action 均逐一验证有生产消费。

本轮新增的历史残留集中在三处：

1. **937195b 合并的软残留**：注释/命名漂移 ×2（T-16）；无硬残留（死配置键、孤儿 fixture 均为零）。
2. **ui-component builder 级零消费 API 簇**（T-1、T-10~T-13）：S-6 投机批次的延伸面，P0-2 形态迁移仍未落地，且新发现 PanelMetrics 连内部测试都不可达。
3. **agent crate 的死符号与测试镜像**（T-2、T-4、T-5~T-7）：compaction/entry 层遗留。

## 上轮未决项现状（全部未变化）

| 编号 | 现状 | 复核证据 |
| --- | --- | --- |
| S-5 影子队列 queued_inputs | 未变，待 spec | src/agent_cli.rs:147,581,589,594-660；agent_cli_input.rs:14,32,55-67 双记账触点仍在 |
| S-6 ui-component 投机批次 | 未变，外部消费仍全 0 | modal_field.rs:74,104、list_pane.rs:36,45、selectable_row.rs:41、split_resizer.rs:77-93、panel.rs:149-156、banner.rs:276-283、panel.rs:41 |
| S-7 枚举死变体 | 未变 | button.rs:28-30 Info/Warning/Success 构造 0；avatar.rs:11 Host 唯一构造点是注释块；banner.rs:27 Info 仅测试提及 |
| S-8 sidebar 主机 rail 尸体 | 未变 | sidebar.rs:274-280 死计算、:333-350 注释块、:351 `let _ = &active_remote_key;` 抑制行均在 |
| S-9 TerminalProcessInfo seam | 未变 | crossh-core/src/terminal/session.rs:6；唯一生产调用 view.rs:984 传 None；process_display_name(title.rs:241) 仅测试激活 |
| S-13 手写 IME/caret 渲染 ×4 | 未变 | 含 git/render.rs:1106-1161 render_commit_editor_text |
| S-14 测试镜像 SDK 私有逻辑 | 未变，另见新实例 T-4 | crossh-agent/src/providers.rs:71-116 |
| S-15 verify_manifest_signature 零调用 | **收窄** | with_key 变体已进生产链（model.rs:12,163）；仅便捷包装 signature.rs:25-32 仍零调用 |

## 新发现

| 编号 | 问题 | 严重度 | 证据与消费者结论 |
| --- | --- | --- | --- |
| T-1 | `PanelMetrics` 投机结构体：定义即终点，全仓（含本 crate 内部与测试）零构造零字段读取，连 lib.rs 都未 re-export | 中 | panel.rs:32-37；grep 全仓唯一出现即定义处（主线程复核）。反方：无。纯死代码。 |
| T-2 | `CompactionDecision` 死结构体：should_compact 返回 `Option<CompactionReason>` 已覆盖需求，CompactionResult 带 tokens_before，该结构连测试都不可达 | 中 | compaction.rs:25-28；全仓唯一出现即定义处（主线程复核）。反方："未来返回决策+快照"预留——但现有返回值已是该形状的超集。 |
| T-3 | **24 个孤儿 i18n 键**（en.yml/zh-CN.yml 成对存在、全语料完整键名零引用）。聚类① agent 设置 UI 引入即未用的说明文案：settings.agent_url(+_description)、agent_protocol(+_description)、provider_status、close 及 13 个 `*_description`；聚类② 初始 i18n 提交遗留：prompt.passphrase_for/password_for/wrong_passphrase_retry、sidebar.entry_count/local/no_matches、sftp.file_too_large（现用键为 sftp.editor_file_too_large）、compose.title | 低中 | 主线程脚本核对全部 408 键 × 完整键名匹配（src+crates+tests），修正 ScanRoot 初报：**forward.*、tooltip.*、language.* 实际均在用**（如 forward.started ← forwarding/view.rs:221），误删会造成运行时回退；同时 ScanRoot 漏报了 settings.agent_url、agent_protocol、close 三键。反方：locale 字符串零运行时代价；若计划给 provider 设置 UI 补字段级说明可留待该改动一起清。 |
| T-4 | 测试镜像新实例：crossh-agent 内逐字复制的 `Utf8StreamDecoder`（#[cfg(test)]），与 ai-sdk 私有生产版完全相同；SDK 自身反而没有对该 decoder 的直接单测——**测试保护的是副本不是真身** | 中 | 副本 providers.rs:74-107 ↔ 原件 crossh-ai-sdk/src/lib.rs:435-466（主线程逐行比对一致）；唯一消费者 tests.rs:681。建议：把该单测移入 ai-sdk（其 mod tests 同文件 :1301 起）后删副本。反方：SDK 私有导致跨 crate 无法直接测的权宜；但移动测试即可消除。 |
| T-5 | `summarize_for_compaction_messages` 自注释 "Legacy helper for tests"，生产零调用，唯一消费者是同文件 cfg(test) 测试 | 低中 | compaction.rs:96-104、唯一调用 :125（cfg(test)）；生产路径走 summarize_for_compaction（agent_cli.rs:690）。测试改为直接构造 AgentSession 即可删。 |
| T-6 | `SessionEntry::{is_message, as_message}` 死方法 ×2，全仓含测试零调用 | 低 | entry.rs:55-64；data 字段可直接 match。反方：pi SessionEntry 对齐面——但同文件已有 ::message 构造器，访问器并非契约必需。 |
| T-7 | `thinking_level_label` 死函数：零调用者，且是对 SDK `ThinkingLevel::label` 的一行 String 化转发包装（重复表示） | 低 | entry.rs:67-69 ↔ ai-sdk lib.rs:40-52 已有 const fn label。 |
| T-8 | `BranchSummary.commit` 死字段：解析 %(objectname:short) 写入后全仓零读者，纯解析 + String 分配成本 | 低中 | git_branch.rs:21(字段)、:29(format)、:65(写入)；渲染只消费 name/current/upstream/ahead/behind/subject（branch_render.rs:107-110 等）。删除需同步缩 format 与 chunks(6)→chunks(5)，净删为正。反方：分支行展示短哈希是自然扩展。 |
| T-9 | `HistoryGraphRow.{lane_count, commit_id}` 生产零读，仅 #[cfg(test)] 断言激活 | 低 | git_history_graph.rs:21,24,138；读者仅本文件 :175 与 src/features/git/history.rs:338。layout 内部 lanes 匹配用的是 ActiveLane.commit_id 而非此字段。 |
| T-10 | `SidePanel::{resolved_width, resolved_width_expanded, clamp_width}` 三方法全仓零外部消费，组件内部渲染路径（panel.rs:199）也直接调 clamp_panel_width 绕开它们 | 低中 | panel.rs:171-183。**spec 层裁定**：spec_20260820_side_panel_rail__* 系列测试把这些方法当形态契约钉住，:176 注释自认 expanded 参数为满足 spec 描述保留——删除需同步修订 spec 文本。 |
| T-11 | `TextInput::{selection, cursor}` 两 builder 零调用者，对应渲染分支（选区高亮拆分 ：226-253、显式光标位拆分 ：296-310）生产不可达；5 个 TextInput::new 生产调用点均未链式设置 | 中低 | text_input.rs:99-107(builder)、226-253、296-310；调用点 sidebar.rs:204、sftp/render.rs:237、sftp/view.rs:774、connections/prompt.rs:126、git/history_render.rs:138 逐一核对。反方较强：这是统一输入路径（S-13 迁移目标）的文本选区契约，重加需重写约 30 行选区高亮逻辑，保留成本为零运行时代价。 |
| T-12 | `SelectOption::disabled` 与 `Select::disabled` 零调用者 → 所有选项恒可用，disabled 渲染样式与键盘导航 next_enabled_index 跳过逻辑生产永不触发 | 低中 | select.rs:52-55,114-117,136,365；Select 唯一调用点 settings/window.rs:327。反方：禁用态是下拉组件的自然完备性，成本一个 bool。 |
| T-13 | 零调用 builder setter 簇 ×3：`TabItem::max_label_width`(tab.rs:129-131)、`CountBadge::height`(count_badge.rs:41-44)、`ModalField::{caret_height, text_size}`(modal_field.rs:139-146) | 低 | 各自生产链式调用点逐一核对为零。与 S-6 同根因，随批次裁定。 |
| T-14 | Git Viewer 三套近乎逐字平行的列表状态机（BranchState/StashState 完全同构，HistoryState 同构+查询过滤）+ 4 份重复 ListState 映射样板 | 低中 | branch.rs:17-155 ↔ stash.rs:17-139（begin_list generation 守卫完全一致）；映射四份 branch_render.rs:30-37、stash_render.rs:29-36、history_render.rs:70-82、render.rs:357-364。反方：泛型化需引入 trait 抽象 summary 类型，抽象成本可能超过省下的 ~200 行；建议仅在下次触碰其中两个以上文件时顺带合并映射部分。 |
| T-15 | lib.rs re-export 过度发布簇（非死代码）：crossh-agent 的 OPENCODE_GO_ID(:29)、crossh-update 的 parse_manifest/UpdateManifest/ManifestError/UpdateArtifact(lib.rs:18-23)、ui-component 的 h_flex/list_empty(lib.rs:45,47)、crossh-agent session_root(session.rs:572 pub 但仅 crate 内两处调用) | 信息 | workspace 内部 crate 的 pub 面留作二进制间共享 API 可辩；清理收益低。 |
| T-16 | 937195b 合并后命名/注释漂移 ×2：policy.rs:113 doc 仍写「opencode-go 三协议」（实为单供应商 opencode）；presets.rs:23 常量名 OPENCODE_GO_ID 现持有值 "opencode"，名不副实 | 信息 | 一行修复候选（豁免清单内）。presets.rs:3-5 数据源出处头注释应保留。 |
| T-17 | 杂项信息级：verify_manifest_signature 便捷包装仍零调用（S-15 残余）；proxyjump.rs:21-23 resolve_jump 与 forward.rs:45-47 parse_remote 两处 3 行直通包装可内联 | 信息 | 收益趋近于零，随下次触碰顺带。 |

## 否决记录（scout 报告后被主线程复核推翻/降级）

- ~~ScanRoot C-2 "~25 个孤儿 i18n 键（含 forward.*、tooltip.*、language.* 等）"~~：**部分否决**。主线程全键严格核对：forward.started/kind_*/stopping/start_failed 有生产引用（forwarding/view.rs:221 等），tooltip.* 全部在用，language.* 为 settings.language* 在用。真实孤儿集收敛为 24 键（T-3），并反向补上了 ScanRoot 漏掉的 settings.agent_url/agent_protocol/close。
- ~~"parse_count 双定义是重复表示"~~（ScanCore 排除记录，维持）：numstat.rs:63 缺省 0 vs git/mod.rs:580 hunk 缺省 1，语义不同。
- ~~"HostConfig::matches/glob 链疑似死代码"~~：经 ssh_config.rs:91 resolve() 存活。

## 干净域结论（本轮验证过，不入 backlog）

- 根 crate：connections、forwarding（ForwardTracker 双集合为防陈旧回调所需）、settings persistence（Snapshot↔File 是 domain↔serde 标准转换对）、updates（UpdateStatus 7 变体全构造全渲染）、shared/、main.rs+bin+CLI 衔接面、20 个 action 键绑定派发闭环。
- agent_cli*.rs：937195b 无任何旧 provider 名/protocol/url 字段残留；slash 命令表与 handle_command 分支一一对应。
- cx.subscribe/on_action：shell.rs/tabs.rs 订阅的全部 TerminalEvent 变体均有 emit 点（terminal/view.rs:426-459,757），不存在订阅永不触发事件。
- b7117bc 无 stale 默认值残留（window.rs:397 选项表含新默认 10000）。
- Cargo 依赖表（根 + agent/ai-sdk/terminal/update 四 crate）逐个 grep ≥1 引用，零死依赖；[patch.crates-io] 三项均在锁文件图内。
- scripts/ 9 个脚本与 ci.yml/release.yml 引用图闭合（含签名 bin fail-closed 闭环）。
- crossh-terminal：events/settings/timestamps 全部 pub 符号有生产消费者。
- IconName 41 变体、theme 21 token、ShellMenuAction 31 变体、GitViewer 15 个 action、Badge/BadgeTone/ToastTone/ModalDialog 等高消费组件全部有生产消费。

## 处置 Backlog

| 优先级 | 编号 | 建议处置 | 说明 |
| --- | --- | --- | --- |
| P1 | T-1、T-2 | 直接修（豁免清单：死符号删除） | PanelMetrics、CompactionDecision 均为纯死结构体，删除无行为影响。可与下轮任一改动顺带交付。 |
| P1 | T-16 | 直接修 | 注释/常量名漂移一行修复；改名 OPENCODE_GO_ID → OPENCODE_ID 需同步 presets.rs 自用点与测试。 |
| P2 | T-3 | 直接修或随 provider 设置 UI 改动顺带 | 删 24 对孤儿键前先跑一次 `rg` 终验（防动态拼键漏网——已验证 format! 动态拼 key = 0）；若 UI 计划补字段说明文案则整体顺延到那次改动。 |
| P2 | T-4 | 直接修（测试搬家） | 把 utf8_stream_decoder 单测从 crossh-agent/tests 移入 crossh-ai-sdk 同文件 mod tests，随后删 providers.rs:74-107 副本。净效果：真身获得直接单测，副本消失。 |
| P2 | T-10 | 决策（spec 层） | SidePanel 宽度方法三件套被 spec 测试钉住：要么修订 spec 后删除，要么接线到渲染路径。与 S-6 批次一并裁定。 |
| P2 | S-6+S-7+S-8、T-12、T-13 | 决策（批次裁定，沿用上轮） | P0-2 形态迁移与主机 rail 是否仍在路线图仍是产品输入问题；T-11/T-12 属同一"组件契约完备性"批次。 |
| P3 | T-5、T-6、T-7 | 直接修 | 死方法/死函数/Legacy helper 删除；T-5 需先把测试改为直接构造 AgentSession。 |
| P3 | T-8、T-9 | 随功能变更顺带 | 字段级死重量，删除涉及解析 format 联动，单独立项不值；下次触碰对应解析器时一并处理。 |
| P3 | T-11 | 保留观察 | 反方论点强（S-13 迁移目标的选区契约、零运行时成本）；除非 S-13 方向改变否则不动。 |
| P3 | T-14 | 随功能变更顺带 | 下次触碰其中两个以上文件时合并 ListState 映射样板；状态机整体泛型化不建议。 |
| 信息 | T-15、T-17、S-9、S-15 残余 | 观察 | 收益不足以立项，随下次触碰顺带。 |
| 未决 | S-5 | spec 认领（沿用上轮） | 影子队列删除涉及时序语义，需短 spec。 |

## 与 SDD 工作流的衔接

- 本轮为纯发现轮，未修改任何生产代码；所有处置按上表进入既有流程（豁免清单直接修 / SDD spec / 产品决策）。
- 若用户授权执行 P1/P2 直接修项（T-1、T-2、T-4、T-16、T-3），预计净删除约 250 行 + 48 行 locale，门禁按惯例 fmt / check-architecture / clippy --workspace / cargo test --workspace。
- 上轮 S-5 影子队列 spec 仍无人认领，继续挂起。

## 受保护表面中"有意保留"项（本轮再次验证，不入 backlog）

- 全部 10 处 `#[allow(dead_code|unused)]` 豁免（双 binary 挂载、visual-tests feature 门、toaster ADR 0013、ansi.rs 测试赋值）。
- LEGACY_SPLIT_IDS 迁移 shim（policy.rs:120）与 presets pi 缓存兼容回退：937195b 的有意兼容面，非残留。
- ForwardTracker pending+active 双集合、WorkspacePane trait 默认方法（ADR 0007）、SSH select! 生命周期与 known_hosts 链、Zed/GPUI 固定 revision、Lucide 1.27.0。

## 执行记录（2026-08-24，用户授权全权裁定）

已删除（生产零消费均经主线程/分片二次 grep 复核）：

- T-1 `PanelMetrics`、T-2 `CompactionDecision`、T-5 `summarize_for_compaction_messages`（测试改写为直接构造 AgentSession）、T-6 `is_message/as_message`、T-7 `thinking_level_label`。
- T-3 全部 24 对孤儿 i18n 键（en/zh 同步，剩余 384 键两侧对称，脚本终验零孤儿）。
- T-4 crossh-agent 内 Utf8StreamDecoder 测试副本；原单测迁入 crossh-ai-sdk mod tests（真身获得直接覆盖），agent 侧 lib.rs 残留 import 一并清理。
- T-8 `BranchSummary.commit`（format 缩为 chunks(5)，fixture 同步）、T-9 `HistoryGraphRow.{commit_id, lane_count}`。
- S-6 批次：ModalTextInput/ModalDialogActions、ListPane/PaneFrame、SelectableRow 结构体形态、SidePanel 宽度方法三件套、danger_banner/warning_banner、RAIL_AVATAR_PITCH、TabItem::max_label_width、CountBadge::height、ModalField::{caret_height,text_size} builder。
- S-7 死变体：ButtonVariant::{Info,Warning,Success}、AvatarKind::Host、BannerTone::Info；S-8 sidebar 主机 rail 尸体代码与 `let _` 抑制行。
- T-10 裁定升级：SidePanel 手柄方向改为纯推导（模块级 `handle_side_for(side)` 纯函数 + spec 测试改写，handle_side 覆盖字段随批次删除）；T-12 SelectOption/Select 的 disabled 全链路（next_enabled_index 化简为 next_index 普通循环，全可用集键盘导航语义不变）。**P0-2 形态迁移与主机 rail 经授权裁定不再保留。**
- T-16 policy.rs 注释漂移修正、OPENCODE_GO_ID → OPENCODE_ID。

裁定保留：

- T-11 TextInput selection/cursor（统一输入路径的选区契约，零运行时成本）。
- SplitResizer::handle_side 与 SplitHandleSide::Left（panel.rs 渲染路径可达）、pane_operation_error（git branch/stash_render 生产消费者，分片执行中新发现的误报风险点）。
- S-9 TerminalProcessInfo seam（成本一个 Option 参数的自然扩展点）。
- S-14 其余 cfg(test) 镜像 helper（apply_model_options 等）仍有测试消费者，未动。

门禁状态：

- `cargo fmt --check` ✅、`scripts/check-architecture.sh` ✅、`cargo clippy --workspace --all-targets` **零警告** ✅。
- `cargo test --workspace` 受环境故障阻塞：BookDrive 外置 NVMe 盘（RTL9210 USB 桥接）在编译负载下读 I/O 错误风暴（kernel log 14:31–14:32 连续 retry 1–5），rustc SIGBUS，随后触发 DART panic 整机重启（panic-full-2026-08-24-151103）。已降速（用户改 jobs=1 + RUST_TEST_THREADS=2）由用户手动重跑中，结果待回填。详见 engineering-notes「rustc SIGBUS 与外置盘 I/O 故障」笔记。

## 本轮未决项

- cargo test 结果回填（用户侧降速重跑中）。
- T-14 列表状态机合并、T-15 re-export 收窄、T-17 杂项：维持"随下次触碰顺带"。
- S-5 影子队列 spec 待认领；S-13/S-15/S-16 维持上轮处置不变。

# Crossh 简化扫描报告（2026-08-26）

触发原因：用户手动触发简化扫描（"帮我看看可以简化和删除的"）。距上一轮（2026-08-24）两天，其间仓库发生结构性变化：**5daa8ce 完全移除 crossh-agent / crossh-ai-sdk / crossh-tui / src/agent_cli\*（约 -12k 行）**，另有 a8f058a（ContextMenu 抽到 crossh-ui-component）、bfdee95（git 菜单修复）、d2121d6（新增 scripts/mac_local_install.sh）、645f15d（release v0.23.0）。本轮定位为**增量轮**：验证移除残留、覆盖新提交、复核开放项现状。

## 扫描方式

- 意图基线：AGENTS.md、docs/architecture.md、全部 15 篇 ADR 目录、engineering-notes 索引。
- 主线程锚点：`cargo clippy --workspace --all-targets` 对 `dead_code|unused` **零告警**；全仓 `#[allow(dead_code|unused)]` 共 10 处（git_launcher×3 双 binary、features/git visual-tests×5、toaster Warning ADR 0013、**text_editing.rs:335 clamp_char_boundary 为新出现豁免**→ 见 N-5）。
- 并行分片四域：ScanRoot（根 crate 全 features/shared/infrastructure/app/bin）、ScanCore（crossh-core+crossh-ssh）、ScanUI（UI 四 crate+features/git 视图）、ScanMisc（update/terminal/scripts/Cargo.toml/locales/docs/aur）。
- 全部关键候选由主线程回到源码二次计数后才收录；一处与历史否决冲突的结论（N-16）经逐行读文件裁定。

## 总体结论

5daa8ce 的移除本身很干净：src/crates/scripts/aur/architecture.md/Cargo.toml 中 agent/ai-sdk/tui 残留引用为 **0**，workspace 成员表、[patch.crates-io]、八个 crate README 均闭合。ContextMenu 抽取无双份并存。但移除在**依赖表和图标词表上留下了尾巴**（3 个幽灵依赖、3 个零消费图标变体），且 core 层暴露出一批此前被 agent 相关噪音掩盖的死面——最大一块是**后台任务输出捕获管道全程零读**（每任务白付 2 个线程）。

## 上轮未决项现状

| 编号 | 现状 | 证据 |
| --- | --- | --- |
| S-5 影子队列 queued_inputs | **结构性关闭** | src/agent_cli.rs 整文件已随 5daa8ce 删除 |
| S-13 手写 IME 渲染 ×4 | **收敛至 ×2** | settings/agent.rs 已删除；sftp/view.rs:772-779 注释确认已迁 TextInput（新 end_caret.rs 是 Path/Upload 分支的合理 DRY 归并，非候选）；存活：workspace/compose_bar.rs:77-220 多行输入、git/render.rs render_commit_editor_text（唯一消费者 :1056） |
| S-15 verify_manifest_signature 零调用 | 维持，本轮收口为 N-4 | signature.rs:25-32 全仓（含 bin/tests/scripts/.github）零调用；签名链完好——crate 自带 src/bin/crossh-sign-manifest.rs 消费其余签名符号，release.yml:207-210 引用有效 |
| T-11 TextInput selection/cursor | 裁定不变（保留） | — |
| T-14 git 列表状态机同构 | 维持信息级 | branch.rs(154 行)/stash.rs(139 行) begin_list/generation 守卫仍逐字同构 |
| T-15 re-export 过度发布簇 | **收窄**（见 N-10/N-13） | crossh-agent 侧全灭；ui-component 余 h_flex/list_empty/theme 模块；update 侧余 record_update_result 等 |
| T-17 杂项薄包装 | 维持（见 N-22/N-23） | shell_quote/shell_quote_remote 双份委托仍在；resolve_jump/parse_remote 别名仍在 |

## 新发现

| 编号 | 问题 | 严重度 | 证据与消费者结论 |
| --- | --- | --- | --- |
| N-1 | 幽灵依赖 ×3：根 Cargo.toml `[dependencies] serde_json`（5daa8ce agent wire-format 残留）、根 `[dev-dependencies] pretty_assertions`、crossh-ssh `[dev-dependencies] env_logger` | 高 | 主线程复核：三者在对应语料 grep 均 **0 命中**。[patch.crates-io] 三项均活（Cargo.lock 解析到 patch rev）。反方：无。 |
| N-2 | locales 孤儿键 `project.open`（en「Open local project」/zh「打开本地项目」成对存在、零消费者；实际用的是 project.open_folder 与 project.choose_directory） | 低中 | 主线程 grep `project\.open"` = 0；en.yml/zh-CN.yml 各删一行。 |
| N-3 | zh-CN.yml 漏译 `tooltip.open_in_editor`（en.yml:114 有、zh 该节缺此键）；代码在消费：workspace/view.rs:408 无编辑器分支回退该键，中文 UI 显示英文 fallback | 低中 | 主线程排序 diff 全量核对：两侧**仅此一键**不对称。修复=补 zh 键而非删 en 键。动态拼键 format! 排查为零。 |
| N-4 | `verify_manifest_signature` 裸便捷包装 + lib.rs:26 re-export 全仓零调用（S-15 最终收口） | 低中 | 主线程 grep 非 `_with_key` 调用点=0；spec_20260818 已覆盖 pinned-key 路径；bin 走 parse_verifying_key+with_key。 |
| N-5 | `clamp_char_boundary`（shared/text_editing.rs:334-341）生产零消费，唯一消费者是 settings/window.rs:897-920 的 cfg(test) 断言，靠新加的 `#[allow(dead_code)]` 压制——生产调用已随输入路径统一消失，测试与豁免是漂移残留 | 低中 | 主线程 grep：生产 0、测试 4 断言。删函数+测试块，allow 豁免清单一并减一。反方：stdlib `floor_char_boundary` 尚未稳定，未来或有用——但无消费者的预留应删而不应压 lint。 |
| N-6 | forward.rs:275 `let _ = &mut tcp;` 无操作残留 | 低 | 一行删除，无反方。 |
| N-7 | `I18nState` GPUI 全局态**纯写不读**：locale_state.rs:20 set_global、:27-29 update_global 是仅有的触点，全仓无 try_global/observe_global 读方；运行期真源是 rust_i18n 内部 locale（set_locale），UI 真源是 AppShell.language_preference。init/set_language 可缩为纯 set_locale 调用，连带修正 shell.rs:154 与 i18n.rs:3 两处误导注释 | 中 | 主线程读全文证实两函数均"先干正事、再写死全局"。反方：未来 observe_global 语言广播的自然挂点；删除是行为保持的。 |
| N-8 | `effective_path_with`（editor_launcher.rs:102）pub(crate) 零外部消费，唯一调用点是同文件 :99，可内联省 ~6 行 | 低 | 反方：与 detect_editors/resolve_editor 注入风格同构的有意测试缝，但当前无调用证明必要性。 |
| N-9 | `MenuEntry::CheckedItem` 变体（ui-component/context_menu.rs:49-52）全仓 0 构造点，仅 4 处防御性 match 臂（渲染 ：70,:189 + git/terminal 测试 helper），勾选列渲染路径永不可达 | 中低 | 主线程复核非 context_menu 文件命中=0。上轮同类死变体（ButtonVariant 等）已裁定删除，标准一致。反方：组件库前向 API 完备性。 |
| N-10 | ui-component 零消费 API 小簇：ListState::loading/error/empty 构造 helper（list_state.rs:20-28，5 个映射点全用枚举字面量）、Stepper::font_weight（stepper.rs:49-52，唯二使用点均未链）、Banner::actions 复数批量 setter（banner.rs:125-128，生产只用单数 .action()）、list_empty 与 theme 嵌套模块的 lib.rs re-export 外部零导入 | 低中 | 主线程抽查 `.actions(`/`ui_component::theme`=仅 ModalDialog 场景命中。反方（theme 模块）：为组件消费者预留的稳定路径，删除改变公开面语义——建议只收紧 list_empty re-export，theme 模块挂决策。 |
| N-11 | `IconName::{Bot,Eye,EyeOff}` 三变体 + 对应 SVG 零代码消费，疑似 5daa8ce 移除 agent 头像/可见性切换后的遗留；41 变体↔41 SVG 映射本身仍一一对应 | 低 | 主线程 grep `IconName::Bot\b|Eye\b|EyeOff\b` = 0。**需用户裁定**：删（变体+SVG 同步删保持映射闭合，符合资产纪律——纪律约束的是新增/替换必须官方源，删除不禁）还是作为策展词表保留。反方：成本≈3 个小文件，保留便于复用。 |
| N-12 | `UpdateArtifact.signature` 预留字段写 never 读 never：generate-update-manifest.sh 不产出该键，注释引用的逐 artifact 签名方案已被 manifest 级实现（ADR 0014）取代；serde 忽略未知字段→删除保持 wire 兼容 | 中低 | 主线程核实 .signature 访问全部落在 manifest 层。反方：协议前向兼容预留声明；pub API 形状收缩→建议 spec 层裁定。 |
| N-13 | update lib.rs re-export 收窄：`record_update_result` 外部零命名（内部仅 installer.rs:131,135）可降 pub(crate)；DEFAULT_ACCELERATE_PREFIX 外部零消费可降 pub(crate)（弱，spec 文档记载其为加速常量）。其余零外部命名的类型（UpdateArtifact/ArtifactFormat/UpdateResult 等）出现在 pub 签名中必须保留 | 信息 | ScanMisc 逐符号计数 + 主线程抽查 updates/mod.rs/crossh-updater.rs。 |
| N-14 | system_stats.rs 死面族：`should_sample`:260 零消费者（shell.rs 内联 visible 判断）；`SystemSnapshot.memory_available`/`disk_available` 字段及参数链零读者；`DiskSnapshot.name`/`available_space` 零读者；`compute_disk_rates`:71 是 compute_network_rates 纯别名；`select_system_disk`:110 与 `_with_mount`:121 同循环双实现（后者仅测试）；`build_snapshot`:158 仅测试调用 ×6 | 中 | 主线程 grep `.memory_available|.disk_available|.available_space` 出 system_stats.rs 外=0。机械删除+签名收缩，净删约 50-70 行。反方（逐条）：监视卡规范指标齐备/同名多盘消歧/注入接缝——均属"字段超供"，渲染端从未跟上。 |
| N-15 | `unix_timestamp_millis`（format.rs:33-39）全仓零消费 | 低 | 主线程 grep 非定义命中=0。与 unix_timestamp_secs 成对的通用工具，7 行。 |
| N-16 | `list_changes`（git/mod.rs:122-124）生产零消费，仅测试投影（cfg(test) 调用 ×16：git/mod.rs 测试 + git_conflict.rs 测试模块）——**更正 2026-08-23 报告的否决记录** | 低中 | 主线程逐行裁定：git_conflict.rs:41 `mod tests {` 之后 :46/:74/:90 全在测试模块内（文件止于 :131），上轮否决引用的"生产冲突解析路径消费"不成立（当时误读或文件此后变动）。生产链走 scan_changes（window.rs:15）。处置：测试改 `scan_changes()?.changes` 后删除，或降 #[cfg(test)]。反方："只要列表不要 status"的语义化入口使十余处测试更短。 |
| N-17 | **后台任务输出捕获管道全程零读**（本轮最大候选）：`BackgroundTaskEvent.{output, exit_code}`（commands.rs:421-426）被 apply_event 显式 `drop((output, exit_code))`（:519-530）；唯一外部消费者 shell.rs:798-805 只读 id/status 打日志。连带死重量：本地 spawn_output_reader 双线程/任务（:621,:624,:680）+ Arc<Mutex<String>> + MAX_OUTPUT_BYTES(:19) + append_output(:699) + final_exit_code(:664)；远端 run_remote_command 输出累积+append_remote_output(connection.rs:698-702)+MAX_REMOTE_COMMAND_OUTPUT(:589)。净删约 70-90 行 + 每后台任务省 2 线程 | 高 | 主线程读 apply_background_event 与 apply_event 证实丢弃语义。**反方强**：「查看后台任务输出」是显然的产品下一步，跨线程收集重写成本高且有测试锁定（background_manager_runs_a_command_and_reports_output）→ 需产品裁定：删，或接线展示（spec 认领）。注意级联：删除后 truncate_to_limit 失去全部生产消费者（余 connection.rs:701 亦在删除面内）。 |
| N-18 | settings/input.rs:6-79 整个 EntityInputHandler impl 九方法全 no-op/None，而 SettingsWindow 无任何文本输入（对比 AppShell/GitWindow/SftpPane 同名 impl 均真实承载 IME） | 中 | 反方（最强）：macOS 键窗 IME 事件可能回落查询根视图 handler，删除有丢键风险 → **删前必须 macOS 真机 GUI 冒烟**（CJK 输入法开设置窗口验证），本机可做但涉产行为面，列决策项。~75 行样板。 |
| N-19 | `HistoryFile.version` 写入从不校验（read_history_file 不 match version），纯格式标记 | 信息 | 版本号为迁移预留，缺校验≠无用；随下次触碰 history 格式时补校验或删。 |
| N-20 | `BackgroundControl.pid` 双终止路径（直杀 :442-452 vs 循环观察标志再杀 :630-637，同进程组两次 SIGTERM） | 信息 | 合并需证明 ≤40ms 时序等价；引擎关闭时序微妙，不动。 |
| N-21 | connection.rs StopForward(:352-367) 与 Err(_) 断开收尾(:377-401) 的 fw_state 拆除重复 ~25 行，可提 teardown helper | 信息 | 「正常停单条」与「断开清全场」语义不同，提取割裂可读性；下次触碰时顺带。 |
| N-22 | shell_quote（core/terminal/shell.rs:426）× shell_quote_remote（ssh/connection.rs:704）双份逐字委托 shlex::try_quote + 相同手写回退；回退实际不可达（shlex 仅对 NUL 返 Err，输入不含 NUL）（T-17 维持+具体化） | 信息 | 至少可收敛两处重复回退为一份；跨 crate 单源须扩 crossh-core pub 面，收益低。 |
| N-23 | proxyjump resolve_jump:24=config.resolve 别名（生产 1 处+测试锚 2）；forward parse_remote:61=parse_host_port 别名（生产 2+测试 5）（T-17 维持） | 信息 | 测试以其为契约锚点，内联需同步改名。 |
| N-24 | HostEntry.detail 与 pool connection_key 双份相同字符串（host.rs:32-36 ↔ pool.rs:6-14） | 信息 | 显示串与身份键允许分化；每条目一 String，不动。 |
| N-25 | ssh_config split_target(:487-518) 与 forward parse_host_port(:66-77) 各有 bracket/rsplit host:port ~20 行重叠，但 user@ 处理/port 可选性/失败回退语义不同 | 信息 | 合并不净，维持。 |
| N-26 | 可见性收窄簇（均 pub 但无外部消费者）：crossh-terminal DEFAULT_FONT_SIZE/DEFAULT_SCROLLBACK（仅 crate 内 serde default 与 normalized 兜底）、terminal/mod.rs:15 truncate_title 再导出、core 的 MAX_HISTORY_ENTRIES/DISPLAY_LIMIT/numstat::parse_count/git_stdout/command_history_cache_path/quick_commands_config_path/HostConfig::matches | 信息 | 降私有属机械清理，收益小，随触碰顺带。 |
| N-27 | scripts 三处事实镜像：① package.sh:13/package-linux.sh:15 内联版本提取绕开 package-version.sh helper（其头注明文 charter "version extraction lives in one place"，release.sh/release.yml 已遵守）；② package.sh 结尾 echo 的手动安装步骤与 mac_local_install.sh:20-28 可执行实现逐条同构（BUNDLE_ID 亦双份）；③ apt 依赖清单 ci.yml terminal-compat 与 release.yml 两 matrix 近拷贝正在分叉（nasm/libxkbcommon-x11-dev/librsvg2-bin 有无不一） | 低 | ①②建议改调 helper/指向脚本一行；③仅报漂移事实不强推合并（job 需求集本就不同，且 2026-08-17 backlog 对同类跨平台重复已有 keep 裁定）。 |
| N-28 | 文档漂移 ×3：architecture.md:39 "shared by the GPUI and ratatui surfaces"（ratatui 表面已随 5daa8ce 移除）与 :47 crossh-git 条目"agent"特征已不存在；README.md:47 "格式、架构、Clippy 和测试检查由 tag 触发的 Actions 在构建发布产物前执行"与 release.yml validate job 注释(:17-22 明示刻意不在 tag 重跑，门禁由 main 的 ci.yml 承担)矛盾；aur/crossh-bin PKGBUILD pkgver=0.20.0 落后 workspace 0.23.0 三个发布（头部注释记载人工 updpkgsums 流程，属设计内节奏但当前未跟进） | 低中 | 前两处一句措辞修正；AUR 属状态记录非删除候选。 |
| N-29 | settings/window.rs:215-260 ids/options 平行数组双存同一 id 列表（Select::on_select 只回传 index 所致） | 轻微 | 反方强：不改组件 API 下最简映射；观察，除非顺路扩展 Select 回传值。 |

## 否决 / 更正记录

- ~~2026-08-23 报告对 "list_changes 零生产调用" 的否决~~：**更正**。当时引用 git_conflict.rs:46,74,90 为生产消费者，实际三者均在 `mod tests`（:41 起）内。本轮以文件结构为准，收录为 N-16。
- ~~ScanCore "HostEntry.detail 双份表示" 升级为正式候选~~：降级信息级（N-24），显示串/身份键允许分化的边界是合理设计。
- ~~ScanUI "SplitHandleSide::Left 疑似死变体"~~：排除。SidePanel::right @ workspace/view.rs:843 → panel.rs:181-183 传递性生产消费。
- ~~"settings/window.rs task 字段无人读取"~~：排除。GPUI 语义下持有 task 防 cancel，非死面。
- ~~"cache/config 双持久化是重复表示"~~：排除。struct 文档明示设计（commands.rs:41-43），persist() 双 clone 是性能小疵非简化项。
- ContextMenu 抽取（a8f058a）：验证**干净**，通用渲染仅存于 ui-component，crates/crossh-ui/src/context_menu.rs 只剩域动作枚举（文档明示归属理由），四宿主全走组件版，无双份并存。
- editor_launcher 全面复查（上轮未覆盖文件）：除 N-8 外全部符号有活消费者（effective_path ×3、detect_editors/resolve_editor/command_display_name/executable_exists/editor_process_command 各有生产调用点）。

## 干净域结论（本轮验证过，不入 backlog）

- **移除残留**：agent/ai-sdk/tui 在 src/crates/scripts/aur/architecture.md/Cargo.toml 零残留；locales 动态拼键=0。
- 根 crate：main.rs 17 action 全路由；app/cli.rs；infrastructure 三文件闭环（手写字节扫描无 stdlib 替代）；connections/manager 全方法有调用点；ForwardTracker（受保护）；updates 七变体全消费；sftp logic/end_caret/view_input/utf16 全活跃；terminal feature 事件六变体全匹配、on_action 十五项闭环；workspace/shell.rs 全字段有读写消费者；全部 cx.subscribe/observe/on_action 目标实体存活、WeakEntity 升级均有使用方、后台任务均为 detach+weak.update 自管。
- UI 四 crate：theme 21 token + 15 布局常量全消费（易漏者逐一证实：danger_hover/diff_*/selection/scrim/focus_ring 等）；git 视图 GitWindow 26 字段、GitOperation 14 变体、model.rs 全符号活跃；assets 41↔41 映射闭合（除 N-11 三变体外均有消费）；pane_toolbar 双入口各有消费者无绕开失效。
- crossh-core：git_branch 七字段（含 upstream_gone）、git_conflict/stash/history/graph/status 解析层、command.rs 七 helper、LocalShellEnvironment、CommandHistory 全方法、config/ssh_config 全 pub 符号——逐一核对有生产链；glob/retry/parser 手写面均有正当性（glob 已用 crate；porcelain 窄格式无 crate 能净替换；认证重试固定 1 次）。
- crossh-ssh：sftp/session/pool/runtime/ConnectionHandle 全方法、HostKeyDecision/CredentialKind/ForwardKind/RemoteCommandStatus 全活跃；除 N-1 env_logger 外依赖全消费。
- crossh-terminal：全部 pub 符号外部消费确认（除 N-26 两个 DEFAULT_* 可降私有的弱点）；上轮"全消费"结论维持。
- 依赖图：根与其余 crate 依赖逐项 grep ≥1 引用；zed 系七依赖全有 use；[patch.crates-io] 三项均活。

## 处置 Backlog

| 优先级 | 编号 | 建议处置 | 说明 |
| --- | --- | --- | --- |
| P1 | N-1 | 直接修（豁免清单：死依赖清理） | 删 3 个幽灵依赖，`cargo build`+clippy 即证。预计显著减编译图（serde_json 链）。 |
| P1 | N-2、N-3、N-6 | 直接修（一行修复） | locale 孤儿键 ×2 行、zh 补译 1 行、`let _ = &mut tcp;` 1 行。 |
| P1 | N-4、N-5 | 直接修（死符号删除） | verify_manifest_signature+re-export；clamp_char_boundary+其测试块（allow 豁免清单随之 -1）。 |
| P2 | N-7、N-8、N-15、N-16 | 直接修（小规模行为保持删除） | I18nState 全局（连注释/文档同步）、effective_path_with 内联、unix_timestamp_millis、list_changes（测试改 scan_changes 投影后删）。 |
| P2 | N-9、N-10 | 批次裁定后删 | ui-component 零消费 API 簇，沿用上轮死变体裁定标准；theme 嵌套模块单独挂决策。 |
| P2 | N-12、N-13 | spec 层裁定 / 直接修 | UpdateArtifact.signature 预留字段（协议形状收缩）；record_update_result/DEFAULT_ACCELERATE_PREFIX 降 pub(crate)。 |
| P2 | N-14 | 直接修（机械删除族） | system_stats 死面族，涉及 build_snapshot* 两签名收缩与测试适配。 |
| P3 | N-11 | 用户裁定 | 三图标变体+SVG 删或留（策展词表 vs 映射闭合）。 |
| P3 | N-17 | **spec 认领** | 后台任务输出管道：删（省 2 线程/任务）或接线展示（产品功能），二选一需产品输入；注意 truncate_to_limit 级联。 |
| P3 | N-18 | 验证门后删 | settings 空 EntityInputHandler：macOS 真机 CJK IME 冒烟通过即可删 ~75 行。 |
| P3 | N-27、N-28 | 直接修（脚本/文档） | 版本提取改调 helper、安装配方指向脚本、architecture.md 两句、README 门禁句。AUR 落后为状态记录。 |
| 观察 | N-19~N-26、N-29 | 随触碰顺带 | 信息级薄包装/可见性收窄/平行数组；T-14、S-13×2 维持原判。 |

## 与 SDD 工作流的衔接

- 本轮发现阶段为纯只读；随后用户授权执行 P1+P2，执行记录见下节。
- 若授权执行 P1+P2 机械项（N-1~N-10、N-13~N-16，除 N-11/N-12 外），预计净删约 **350-400 行 + 3 个依赖 + 3 行 locale**，门禁按惯例 fmt / check-architecture / clippy --workspace / cargo test --workspace。
- N-17（输出管道）与 N-18（空 input handler）若推进，各需一个短 spec（前者产品方向输入，后者真机冒烟证据）；上轮 S-5 影子队列 spec 已因 agent 移除结构性关闭，无遗留挂起 spec。

## 受保护表面中"有意保留"项（本轮再次验证，不入 backlog）

- 全部既有 allow(dead_code) 豁免（git_launcher 双 binary ×3、visual-tests ×5、toaster Warning ADR 0013）；N-5 的新豁免属漂移非有意。
- TerminalProcessInfo seam（S-9 原判）、TextInput selection/cursor（T-11）、S-4 git metric 双实现（未现第三副本）、SSH select! 生命周期与 known_hosts 链、ForwardTracker 双集合、Zed/GPUI 固定 revision、Lucide 1.27.0 资产纪律、check-architecture.sh 白名单、package-windows.ps1 Zed-checkout 复制（2026-08-17 backlog Option-A keep）、ADR 0003/0009/0015 历史 agent 决策记录。

## 本轮未决项

- N-11 图标词表、N-12 协议预留字段、N-17 输出管道去向、N-18 真机冒烟：需用户输入或 spec。

## 执行记录（2026-08-26，用户授权 P1+P2）

已落地（N-1~N-10、N-13~N-16，未含需单独裁定的 N-11/N-12）：

- N-1：删根 `serde_json`、根 dev `pretty_assertions`、crossh-ssh dev `env_logger`（Cargo.lock 同步收敛）。
- N-2/N-3：删孤儿键 `project.open`（en/zh 各一行）；zh-CN 补译 `tooltip.open_in_editor`。
- N-4：删 `verify_manifest_signature` 裸包装与 lib.rs re-export（`pinned_verifying_key` 因 model.rs 生产链保留）。
- N-5：删 `clamp_char_boundary` 及 settings/window.rs 与 text_editing.rs 自身测试模块中的全部断言（后者为本轮 grep 失误漏计的消费者，clippy 兜住）；allow(dead_code) 豁免清单随之 -1，现余 9 处。
- N-6：删 forward.rs 无操作行；连带消除 tcp 的多余 `mut`。
- N-7：整删 locale_state.rs 与 I18nState 全局；settings::init 去 cx 参数化（main.rs 调用点同步），settings_actions 直调 `shared::i18n::set_locale`；shell.rs/i18n.rs 注释同步改写。
- N-8：`effective_path_with` 内联进 `effective_path`。
- N-9：删 `MenuEntry::CheckedItem` 变体 + estimate/render 两处 match 臂 + 勾选列渲染块 + git/terminal 测试 helper 防御分支。
- N-10：删 ListState 三构造 helper、Stepper font_weight 全链（字段/setter/渲染/测试）、Banner::actions 复数 setter；`list_empty` 降私有并摘出 lib.rs re-export（theme 嵌套模块按裁定挂决策未动）。
- N-13：update lib.rs 摘除 `record_update_result` 再导出（降 pub(crate)）；`DEFAULT_ACCELERATE_PREFIX` 降 pub(crate)。
- N-14：system_stats 删 `should_sample`、`memory_available`/`disk_available` 快照字段及参数链、`DiskSnapshot.{name, available_space}`、`build_snapshot` 包装、`compute_disk_rates` 别名及其三个重复网络契约的测试用例；`select_system_disk` 双实现合并为 `_with_mount` 单源（采样器传 `system_mount_path()`），主盘 used 推导上移采样器。
- N-15：删 `unix_timestamp_millis`。
- N-16：删 `list_changes`；git/mod.rs 13 处与 git_conflict.rs 2 处测试改为 `scan_changes(..)?.changes` 投影——**同时坐实对 08-23 否决记录的更正**。

门禁状态：

- `cargo fmt` ✅、`scripts/check-architecture.sh` ✅、`cargo clippy --workspace --all-targets` **零警告** ✅（仅第三方 block v0.1.6 既有提示）。
- `cargo test --workspace --no-fail-fast`：除 crossh-core lib 的 **3 个既有失败**外全绿（root bin 46、ui-component 67、update 33+7、ssh 11、terminal 4、assets 3、theme 1 等）。3 个失败为 `background_manager_runs_a_command_and_reports_output`（被用户 `~/.profile` 中失效的 rye 行污染输出）与 git_history_graph 两例 lane 断言；经 `git stash` 基线复跑确认**与本轮改动无关**（本轮未触碰 commands.rs/git_history_graph.rs），属既有问题，建议另立跟进。

净变更：28 文件（不含 Cargo.lock），+124/-306 行。

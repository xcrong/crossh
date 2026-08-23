# Crossh 简化扫描报告（2026-08-23）

触发原因：用户 `@find-simplifications` 手动触发，要求对全仓可简化点进行证据化审计，重点关注死代码 / 重复表示 / 投机泛化 / 手写轮子。

扫描方式：意图基线 `AGENTS.md`、`docs/architecture.md`、全部 ADR（0001-0015）、`docs/engineering-notes/README.md` 与近四轮审计（2026-08-17 code-review、2026-08-18 架构冗余、2026-08-20 简化、2026-08-21 文档漂移、2026-08-22 简化）。分片扫描（因 subagent 延迟，改为主线程串行巡检 + 交叉验证）：

- 根 crate feature（workspace/terminal/git/sftp/forwarding/connections）与 `src/shared`、`src/bin`、`src/agent_cli*`
- `crates/crossh-ssh` / `crates/crossh-core` / `crates/crossh-agent` / `crates/crossh-terminal`
- `crates/crossh-ui` / `crates/crossh-ui-component` / `crates/crossh-theme` / `crates/crossh-assets` / `crates/crossh-tui` 与 `src/features/*/view+render`
- settings / updater / infrastructure / scripts / `Cargo.toml` 依赖图

锚点命令：`cargo check --workspace`（通过）、`cargo clippy --workspace --all-targets 2>&1`（1 处 `deny(clippy::overly_complex_bool_expr)` 阻断，`crates/crossh-tui/src/scroll_view.rs:125`）、`grep -rn "allow(dead_code|unused"`（`src` 剩余 11 处、`crates` 剩余 0 处，见 N-2）、`wc -l` 文件尺寸（`shell.rs:1679 view.rs:1584 agent_cli.rs:1871` 均 < 2000）、`grep -rn "host_entry_matches|available_main_width|clamp_panel_width|terminal_split|format_size|unix_millis|passphrase|SessionMessage|shell_quote_remote|truncate_to_limit"` 逐项核验跨 crate 复用。未改动生产代码，报告本身为文档变更。

## 总体结论

仓库已连续四轮收敛，**2026-08-22 的 S-1~S-12 中有 9 项已在 24 小时内闭环**（见“与上一轮衔接”），未引入新的整模块债务。本轮仅有 **1 项阻断级逻辑缺陷 + 3 项低危残留 shim/死符号** 需要处置，其余为受保护表面的有意保留，不建议单独立项：

1. **阻断缺陷**：`crossh-tui` 的 `scroll_view.rs:125` 逻辑错误触发 `#[deny(clippy::overly_complex_bool_expr)]`，`cargo clippy --workspace --all-targets` 编译失败（N-1，高，阻断 CI）。
2. **薄 shim 残留**：`src/features/sftp/logic.rs:155 format_size -> format_bytes` 的 2 行委托在 `format.rs` 权威化后仍保留，`render.rs:98,322` 可直呼 `format_bytes`（N-2，低）。
3. **死符号 3 处**：`quick_commands_rail.rs:276 background_task_badge`（永远 `rail_status_badge` 直调）、`registry.rs:318 compose_entry`（`compose` 已由 `compose_state_for/compose_visible` 消费）、`agent_cli_slash.rs:35 SlashCandidate.desc`（渲染管线未消费）（N-3~N-5，均低）。
4. **已闭环的重复**：`host_entry_matches`→`HostEntry::matches_query`、`shell_quote_remote`→`shlex::try_quote`、`truncate_to_limit` 下沉到 `crossh-core::format`、`unix_timestamp` 双 helper 收敛、`AuthChoice::Key.passphrase` 删除、`git/command.rs` + `git/numstat.rs` 抽离、`terminal_split_left_width` 已委托 `clamp_panel_width`——均不再立项。

已验证无问题的受保护表面：`crossh-ssh` 的 `run_connection select!` + `WeakEntity` 池、`known_hosts` 决策链、`crossh-update` 的 `signature`（ADR 0014）、`crossh-theme`→`crossh-ui/theme` 的透传（22 个 `color()` 为 ADR 0003 有意隔离）、`crossh-assets` 的 `IconName` 嵌入、`check-architecture.sh` 的 `terminal_element.rs:2196` 白名单与逻辑 crate 零 `gpui` 污染（`cargo check` 全绿）、`git/mod.rs:70-113` 的 5 处与 `git_launcher.rs:34/40/52` 的 3 处 `visual-tests` / 双 binary 豁免（已注释）、`ansi.rs:550` 的 `allow(unused_assignments)` 与 `select.rs/panel.rs` 的 `allow(clippy::*)` 为测试/类型复杂度豁免。`crossh-tui` 的 `visible_width/wrap_text_with_ansi/MAIN_SCREEN` 等与 `ScreenRenderer` 的同步输出管线（`BEGIN_SYNC/OSC52`）属新 crate 内聚，无跨 crate 重复。

## 发现

| 编号 | 问题 | 严重度 | 证据与消费者结论 |
| --- | --- | --- | --- |
| N-1 | `scroll_view.rs:125` 逻辑错误阻断 `clippy --deny` | 高 | `crates/crossh-tui/src/scroll_view.rs:125 self.following_end = self.follow_end && next == max && lines >= 0 \|\| (lines < 0 && false);` 中 `\|\| (lines < 0 && false)` 恒为 `false`（clippy `overly_complex_bool_expr` 已 `deny`，`cargo clippy --workspace --all-targets` 编译失败），且 `126-128 if lines < 0 { self.following_end = false; }` 与该分支语义重复——负向滚动的 `following_end` 被两处同时置 false。消费者：生产（`ScrollView::scroll_by` 被 `crates/crossh-tui/src/alt_screen.rs` 与 TUI 滚动容器生产调用）。修复为 `self.following_end = self.follow_end && next == max && lines >= 0;` 并保留 `if lines < 0` 分支或合并为单一 `if`，`cargo clippy --workspace --all-targets -- -D warnings` 为门禁。**保留论点**：无——`deny` 已阻断，必修。 |
| N-2 | `sftp/logic.rs:155` 的 `format_size` 薄 shim 残留 | 低 | `src/features/sftp/logic.rs:155-157 pub(crate) fn format_size(bytes:u64)->String { format_bytes(bytes) }` 仅被 `src/features/sftp/render.rs:98,322` 与 `view.rs:1167` 测试调用；`crates/crossh-core/src/format.rs:10 format_bytes` 为权威（`sftp.rs:10` 与 `logic.rs:7` 已直连）。`crates/crossh-core/src/format.rs` 的历史 `format_size` 别名已删除，本处为唯一残留 shim。消费者：生产但可直呼 `format_bytes`（crate 内 2 处）。**保留论点**：`format_size` 语义与 `format_bytes` 完全等价，迁移仅改 2 行导入+测试别名，无行为差异；若侧栏/状态栏需统一命名可保留一处别名，但当前 `crossh-core::format_bytes` 已为唯一真相。 |
| N-3 | `quick_commands_rail.rs:276` 死函数 `background_task_badge` | 低 | `src/features/workspace/quick_commands_rail.rs:276 #[allow(dead_code)] fn background_task_badge(status: BackgroundTaskStatus) -> impl IntoElement { rail_status_badge(...) }` 全仓仅定义处命中（`grep` 无生产调用），渲染路径直接 `rail_status_badge(background_task_color(status), theme::surface())`。消费者：非生产（`allow(dead_code)` 压制）。历史注释"保留独立函数以便后续轨道与状态栏样式统一"——但 `status_bar` 与 `sidebar` 已各自直调 `rail_status_badge`，抽离未发生。**保留论点**：若后续需统一 `BadgeTone` 映射可保留包装，但当前 `rail_status_badge` 已为统一入口，死函数仅增 1 符号噪音。 |
| N-4 | `registry.rs:318` 死 getter `compose_entry` | 低 | `src/features/workspace/registry.rs:318 #[allow(dead_code)] pub(crate) fn compose_entry(&self, view: ActiveView) -> Option<&ComposeEntry>` 生产零调用（注释"预留查询接口：当前生产路径通过 `compose_state_for/compose_visible` 访问"），`compose` 的权威访问为 `registry.rs:340 compose_state_for` 与 `compose_visible`。消费者：非生产（`allow(dead_code)` 压制）。**保留论点**：`compose_entry` 为调试/未来复用预留，但 `compose_visible` 已暴露 `bool` 语义，保留 raw `Option<&ComposeEntry>` 无额外能力；可改为 `#[cfg(test)]` 或删除。 |
| N-5 | `agent_cli_slash.rs:35` 死字段 `SlashCandidate.desc` | 低 | `src/agent_cli_slash.rs:35 #[allow(dead_code)] pub(super) desc: String` 注释"候选说明（自动补全浮层展示用；当前渲染管线暂未消费）"，`grep` 仅 `slash_candidates` 构造处写入、无读取。消费者：非生产（测试/渲染均未消费）。**保留论点**：为后续 slash 浮层说明文案预留，但保留未消费字段使 `SlashCandidate` 多 1 分配/克隆；可待浮层消费时再补，或改为 `#[cfg(test)]` 桩。 |
| — | `toaster.rs:13` `Warning`/`Error` 枚举 | 信息 | `src/features/workspace/toaster.rs:12 #[allow(dead_code)] Warning, Error` 注释"ADR 0013 契约预留语气；构造点只在 `cfg(test)` 测试中"，`grep` 仅测试 `toaster.rs:94-120` 的 `NoticeLevel::Warning/Error` 断言。消费者：非生产但为 ADR 0013 的有意预留（`NoticeLevel` 的 `Info/Warning/Error` 三态为 `AppShell` toaster 的契约），**本轮不进入 backlog**，属受保护表面。 |

薄候选（不足单独立项，收集备查）：`crates/crossh-tui/src/ansi.rs:550 #[allow(unused_assignments)] { current_visible = 0; }` 为 `non_snake_case` 测试模块内的临时抑制，非生产；`crates/crossh-ui-component/src/panel.rs:392 #[allow(non_snake_case)]` 等 6 处 `non_snake_case`/`clippy::type_complexity` 为 GPUI 测试/类型豁免，已最小化。

## 处置 Backlog

| 优先级 | 编号 | 建议处置 | 说明 |
| --- | --- | --- | --- |
| P0 | N-1 | 直接修（阻断） | `crates/crossh-tui/src/scroll_view.rs:125` 改为 `self.following_end = self.follow_end && next == max && lines >= 0;`（删除 `\|\| (lines < 0 && false)`），`126-128` 的 `if lines < 0 { self.following_end = false; }` 保留或与前式合并为 `if` 三分支；`cargo clippy --workspace --all-targets -- -D warnings` 与 `cargo test -p crossh-tui` 为门禁。 |
| P3 | N-2 | 直接修（删 shim） | 删除 `src/features/sftp/logic.rs:155-157 format_size`，`render.rs:8` `use crate::features::sftp::logic::format_size` 改为 `use crossh_core::format::format_bytes;`，`render.rs:98,322` 直呼 `format_bytes`，`view.rs:1167` 测试随之 `format_bytes` 或保留本地 `use super::logic::format_bytes` 别名；`cargo test --workspace --lib` 含 `sftp::logic` 仍绿。 |
| P3 | N-3 | 直接修（删死代码） | 删除 `quick_commands_rail.rs:275-279 background_task_badge` 及其 `#[allow(dead_code)]`；若后续需统一 `BadgeTone` 再由 `rail_status_badge` 包装重建。`cargo check --workspace` 零 `dead_code` 新增。 |
| P3 | N-4 | 直接修（可选） | 二选一：删除 `registry.rs:317-321 compose_entry`，或收窄为 `#[cfg(test)]` / `#[cfg(debug_assertions)]` 调试接口；同步删除 `#[allow(dead_code)]`。与 `compose` 的 per-view 响应式重构同批亦可。 |
| P3 | N-5 | 随功能变更顺带 | 保留 `desc` 直至 slash 浮层消费说明文案（`slash_candidates` 已产 `display/insert`，`desc` 为第三列），或立即删除并在浮层 spec 中重建；不单独立项，随 `agent_cli` 浮层渲染顺带。`cargo clippy` 零 `dead_code` 过期为门禁。 |

受保护表面中"有意保留"项（本轮不进入 backlog）：

- `toaster.rs:13 Warning/Error`（ADR 0013）、`git/mod.rs:70/82/93/103/113` 的 5 处 `visual-tests` 夹具与 `git_launcher.rs:34/40/52` 的双 binary 挂载（ADR 0008）、`ansi.rs:550` 的 `unused_assignments` 与 `select.rs:543` 的 `clippy::type_complexity` 等最小豁免、`terminal_element.rs:2196` 白名单、`crossh-tui` 的 `ansi::visible_width/wrap_text_with_ansi` 与 `MainScreenRenderer` 的同步输出管线（`BEGIN_SYNC`/`END_SYNC`/`OSC52` 为 pi-tui 1:1 移植契约，不与 `crossh-core` 重复）。

## 与 SDD 工作流的衔接

- **直接修类（豁免清单内）**：N-1 为 `deny` 阻断的逻辑修复 + 冗余分支清理，N-2~N-4 为死代码/薄 shim 删除，均无行为变更（或仅收缩未暴露的 `pub(crate)` 符号），可直接修并以 `cargo fmt --check` / `scripts/check-architecture.sh` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test --workspace`（含 `env -u CROSSH_UPDATE_SIGNING_KEY` 干净环境，`--workspace` 覆盖 `crossh-tui`）验证。N-1 需优先合入以解阻 CI。
- **需 spec 认领类**：无。本轮未发现需新增 spec 的行为变更；N-5 的 `desc` 字段若需消费应随 slash 浮层渲染 spec 一并处置。
- **决策类（ADR）**：无。本轮未发现需新增 ADR 的结构性边界变更；`crossh-tui` 的引入（`20260822-agent-tui-pi-parity.md`）与 `scroll_view` 的移植属既有 spec 范畴。

## 本轮未决项

- N-1 的 `scroll_by` 在 `following_end == true` 时取 `start = max` 的语义需与 `update_layout` 的 `following_end` 保持一致（`update_layout:93 following_end = follow_end && content_height > viewport_height`），修复后需以 `spec_20260822_agent_tui_pi_parity` 的滚动契约（`scroll_by` 的 `followingEnd` 与 `lines < 0` 重置）为回归哨兵。
- N-2 删除 `format_size` 后 `view.rs:1167` 测试的 `format_size_uses_human_readable_units` 可直接复用 `crossh_core::format::tests` 或保留本地 `format_bytes` 包装的 thin re-export 测试，两种形态均满足 `docs/testing.md` 的行为矩阵（Matrix `format_bytes` 条目）。
- N-3~N-5 的删除与 `allow(dead_code)` 清理需与 `cargo clippy --workspace --all-targets` 的零 `dead_code` 过期保持同步；`git/mod.rs` 与 `git_launcher.rs` 的 8 处必要豁免已注释，不应随清理误删。
- 与历史审计衔接：`2026-08-22` 的 S-1（`git/command.rs`）、S-2（`git/numstat.rs`）、S-3（`crossh_core::format::truncate_to_limit`）、S-4（`shell_quote_remote→shlex::try_quote`）、S-5（`HostEntry::matches_query`）、S-8（`AuthChoice::Key.passphrase` 删除）、S-12（`unix_timestamp_{secs,millis}` 下沉到 `format.rs`）已闭环；S-6（`shell.rs` 的 `available_main_width` 死克隆）已在巡检中确认删除（`shell.rs:1555-1580` 已无该函数）；S-7（`terminal_split_left_width`）已委托 `clamp_panel_width`（`view.rs:327`）；S-10（`SshConfig.hosts` 的 getter 双真相）已收敛为仅字段（`ssh_config.rs:63` 仅 `pub hosts`）；S-9 的 3 处过期抑制（`agent_cli_input.rs` 的 `allow(dead_code)`、`crossh-agent/src/session.rs:SessionMessage`、`manager.rs:fs`）已删除，剩余 11 处均为必要豁免；S-11 的 `format_size` 别名在 `crossh-core/src/format.rs` 已删除，本轮 N-2 为 `sftp/logic.rs` 的最后一层 shim。`2026-08-20` 的 S-1~S-10 与 `2026-08-18` 的 S-1/S-2 已全部完成，无回退。

每条候选的最强反方论证（为什么保留）：

- N-1：`|| (lines < 0 && false)` 是否为"负向滚动永不跟随"的显式防御？——该防御已由 `126-128 if lines < 0 { self.following_end = false; }` 显式实现，`|| false` 仅使前式恒等，下次阅读者会误判为"正向跟随或负向某条件"，删后语义更清晰且 `deny` 已阻断。
- N-2：`format_size` 包装是否隔离了 `sftp` 对 `crossh-core` 的直接依赖？——`logic.rs:7` 已 `use crossh_core::format::format_bytes;`，`sftp/logic` 本就属 `crossh-core` 消费侧，保留包装仅增 1 跳，无隔离收益。
- N-3：`background_task_badge` 是否为后续 `StatusBar` 与 `Rail` 的 Badge 统一样式预留？——预留已由 `rail_status_badge` + `background_task_color` 实现，轨道徽章仅 `rail_status_badge` 一处调用，包装无额外映射。
- N-4：`compose_entry` 是否为 `compose` 的调试探针？——`compose_state_for` 已暴露 `(String,usize,Option<usize>,String,Option<usize>)` 全量，`compose_visible` 暴露 `bool`，raw `Option<&ComposeEntry>` 未在 `src/features/workspace` 生产路径消费，调试可由 `#[cfg(test)]` 桩覆盖。
- N-5：`SlashCandidate.desc` 是否为浮层第二列说明的占位？——确为占位，但"占而不消"使 `SlashCandidate` 的 `desc: String` 在每次 `slash_candidates` 中分配 28 个空串（`SLASH_COMMANDS` 全量），真需第二列时重建成本仅 1 字段+渲染分支，当前保留为负收益。

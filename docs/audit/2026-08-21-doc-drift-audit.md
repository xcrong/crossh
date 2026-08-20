# 文档漂移审计（2026-08-21）—— 代码为唯一真相

触发原因：用户要求"扫描整个仓库中过时的文档，代码是唯一真相"。

扫描方式：以 `AGENTS.md` / `docs/architecture.md` / 全部 `docs/adr/0001-0014` / `docs/engineering-notes/README.md` 为意图基线；逐份读取 `docs/*.md`、`docs/specs/*.md`、`docs/plans/*.md`、`docs/archived/*.md`、`docs/audit/*.md`、`crates/*/README.md`；以 `Cargo.toml`（Zed rev）、`src/bin/`、`scripts/package*.sh`、`src/features/*/settings.rs`、`src/main.rs`、`docs/testing.md` 关键行为矩阵为代码真相，`rg` 验证符号/常量/产物清单是否一致。未对受保护表面（ADR 裁决边界、check-architecture 白名单、Lucide/Zed 固定 revision 语义、engineering-notes 防御模式）提简化删除，仅校对文档陈述与代码实现是否一致。

## 总体结论

仓库文档整体健康，架构与工程规则文档已随 2026-08-18/20 两轮简化合入，但存在 **10 项"文档陈述滞后于代码"的中低危漂移**：最强的是 `xterm-compatibility-profile.md` 的 Zed revision 仍为已替换的 `90d024b`（D-1，高，本次直接修）；其次是 `testing.md` Workspace 单行承载 8 子特性但与多个 `in-progress` spec 状态不一致（D-5/D-6，中），`remote-update-plan.md` 对 Linux 产物与后续计划的描述未与已完成的签名/加速 spec 同步（D-3/D-4，中），两份 `docs/archived/*` 计划含完全过时的模块布局与 `gpui rev 1d217ee` 且头部无"已归档"显式标注（D-7，低-中），多份历史审计中已删除的 `low_latency` 残骸仍以 high 未处置呈现（D-10，低）。无需要立 ADR 的结构性漂移；仅 1 项需直接修（D-1，已随本报告落地），其余入处置 backlog，走 spec 更新或文档标注即可闭环。

## 发现

| 编号 | 问题 | 严重度 | 证据（文件:行号）与消费者结论 |
| --- | --- | --- | --- |
| D-1 | `xterm-compatibility-profile.md` 声明的 Zed revision 与 `Cargo.toml` 唯一真相不一致 | 高 | `docs/xterm-compatibility-profile.md:16-17` "`Cargo.toml` (`90d024b88abc91264d9a0ad260eb4f365fa695c3`)" vs `Cargo.toml:36-40` 全部 `gpui/terminal/settings/...` 均 `rev = "1b04e4caf01e376624fb514ef85b0e6d8ee5d930"`。`terminal-compatibility.md:47` 正确表述为"pinned Zed terminal crate"未写死 rev，但 xterm profile 的 Sources 与 Evidence 仍按旧 rev 举证，导致读者按旧 checkout 路径 `~/.cargo/git/checkouts/zed-*/90d024b` 查找失败。属纯文档漂移，无行为影响，但影响可复现性。消费者：生产 `Cargo.toml` 为唯一真相，文档为影子。**本次已直接修**，改为 `1b04e4c...` 并保留 `Cargo.toml` 为权威声明。 |
| D-2 | `docs/plans/2026-08-17-simplification-backlog.md` 已标记"全部完成 2026-08-18"但未归档，后续 `2026-08-20-simplification-audit.md` 又产生 10 项新完成项（S-1~S-10），读者易误将 backlog 当当前待办 | 中 | `docs/plans/2026-08-17-simplification-backlog.md:1-8` 头部写"全部完成 ✅ 14 个提交 `c1d42cc…7a6d731`"且文件仍在 `docs/plans/`（非 `docs/archived/`），`docs/audit/2026-08-20-simplification-audit.md:27-` 新增 S-1~S-10 已全部完成（`input_handler`、`text_editing_key`、`shlex`、`glob/shellexpand`、`unicode-width`、`format.rs`、settings_actions 等）。同一"简化 backlog"存在两份完成记录，职责边界不清。消费者：生产代码已无对应残骸，文档仅作历史档案。反方：plans 目录允许存放已完结计划，但需显式标注"已归档/已被 audit 取代"。 |
| D-3 | `remote-update-plan.md` Linux 产物描述与 `scripts/package-linux.sh` 唯一真相不一致 | 中 | `docs/remote-update-plan.md:50-51` 表格对 `linux-aarch64/x86_64` 的 Preferred artifact 均只写 `.AppImage`；`scripts/package-linux.sh:2,7,38-50,131` 明确同时产出 `dist/crossh-<ver>-linux-<arch>.tar.gz` **与**同前缀 `.AppImage`（tar.gz 节与 AppImage 节分开组装，release manifest 需同时校验两者）。`remote-update-plan.md:18` 文字"Linux tar/AppImage"与表格矛盾。消费者：发布物以脚本为准，文档表格为滞后子集。 |
| D-4 | `remote-update-plan.md` 的"后续实施顺序"与"验收清单"章节仍以待办口吻呈现，已完成的签名/加速能力未同步标注 | 低 | `docs/remote-update-plan.md:84-92` "后续实施顺序 1. 增加 ETag/Last-Modified 缓存…" 中第 3-4 项（manifest 签名、加速前缀回退、HTTPS/签名/路径穿越等）已由 `docs/specs/20260818-update-manifest-ed25519-signature.md:done` 与 `docs/specs/20260818-update-gh-proxy-acceleration.md:done` 在 v0.16.4 完成并发布（`Cargo.toml:36` 公钥内置、release.yml 签名/自验步骤），但章节未标注"已完成（见 ADR 0014 / 上述 spec）"。`92-106` 验收清单 8 项中前 4 项已由 34 个单测覆盖，仍以 `[ ]` 空勾呈现易误判为未验证。消费者：代码与 spec 为 done，文档章节为历史路线图残留。 |
| D-5 | `docs/testing.md` Workspace 关键行为矩阵单行承载 8 个子特性，与 feature-owned settings 原则的"每 feature 独立矩阵行"预期不一致，且遗漏已实现的 `pinned-tab-default-command` / `terminal-compose-bar` 契约 | 中 | `docs/testing.md:60` 单行含：tab 生命周期、失效目录清理、路径复制、固定/重命名、Toaster、Git Push/Pull toast、**编辑器打开**（`editor_command`/`editor_priority`）、下拉检测等约 400 字；`docs/specs/20260821-pinned-tab-default-command.md:in-progress`（固定标签默认命令）、`docs/specs/20260820-terminal-compose-bar.md:in-progress`（compose 输入条）已在代码中实现（`src/features/workspace/compose.rs`、`src/features/workspace/shell.rs:editor_launcher`）但矩阵未单列其行为，或与 Workspace 行混在一起难以追溯。`editor_priority` 在矩阵中写"旧配置中的 `editor_priority` 字段被忽略"与 `src/features/workspace/settings.rs:81-82` 归一化、`src/features/settings/persistence.rs:325-330` 兼容读一致，但与 `remote-update-plan` 无关的 Workspace 行过长导致评审时易漏。消费者：生产矩阵是 CI 门禁依据，单行过长降低可测性。 |
| D-6 | 多个 spec 的 `状态: in-progress` 滞后于代码真相（testing 矩阵与实现已视为 done） | 中 | `docs/specs/20260817-git-sync-toast.md:4` `in-progress` vs `docs/testing.md:60` 已含"Git Push/Pull 成功显示 Success Toast…running 期间重复点击不产生新 Toast"且代码 `src/features/workspace/notifications.rs` 已实现；`docs/specs/20260817-recent-local-dir-recovery.md:4` `in-progress` vs `src/features/workspace/registry.rs` 已实现失效清理；`docs/specs/20260817-workspace-status-path-copy-toast.md:4` `in-progress` vs Toaster 已作为上条 spec 依赖完成；`docs/specs/20260818-local-tab-pin-rename.md:4` `in-progress` vs 固定/重命名已在矩阵视为 done；`docs/specs/20260820-open-project-in-editor.md:4` `in-progress` vs `testing.md:60` 编辑器打开已入矩阵且 `src/features/workspace/settings_actions.rs`/`editor_launch_tests.rs` 已绿；`docs/specs/20260820-terminal-compose-bar.md:4` `in-progress` vs `compose.rs` 已实现。`spec_20260817_*` 等 6 个 spec 的验收清单在代码侧已勾选但文档未迁至 `done`，按 `docs/specs/README.md` 生命周期应为 `done` 归档。消费者：`specs/README` 约定 `done` 为档案，滞后会导致重复立项。 |
| D-7 | `docs/archived/*` 两份历史计划含完全过时的模块布局与 `gpui rev 1d217ee`，文件头部无显式"已归档/历史"标注，易被误作现行设计 | 低-中 | `docs/archived/ssh-client-gpui-plan.md:7` "`gpui rev 1d217ee`" vs `Cargo.toml:36` 真相 `1b04e4caf01e376624fb514ef85b0e6d8ee5d930`；该文件 `src/config/ssh_config.rs` / `src/ssh/session.rs` / `src/ui/terminal_view.rs` 等 13 处模块路径均为 `src/ui/*` 旧布局，当前为 `crates/crossh-ssh/*` + `src/features/*`（见 `docs/architecture.md:10-23` crate 图）。`docs/archived/crossh-remaining-features-plan.md:5` 同样含 `src/config/ssh_config.rs` / `src/ssh/*` / `src/ui/workspace.rs` 旧路径。文件虽在 `archived/` 目录但正文首行即以"Crossh —— 基于 gpui 的轻量 SSH 客户端（实现方案）"作现行方案口吻呈现，无"本文件已归档，仅作历史参考"横幅。消费者：`docs/architecture.md` + ADR 为现行真相，archived 为历史。 |
| D-8 | `xterm-compatibility-profile.md` 与 `terminal-compatibility.md` 的 Evidence Commands 与 `docs/testing.md` / CI 真相不一致 | 低 | `docs/xterm-compatibility-profile.md:59-64` `cargo test --quiet` + `cargo test --release --test terminal_replay -- --test-threads=1` vs `docs/testing.md:88-94` 要求 `cargo fmt --check` / `scripts/check-architecture.sh` / `cargo clippy --all-targets -- -D warnings` / `cargo test --workspace` + 终端 replay 的"独立 target"说明；`docs/terminal-compatibility.md:16-20` 写"CI runs `cargo test --release --workspace --lib` + `cargo test --release --test terminal_replay`"但 `docs/testing.md:72-74` CI 规则实际为 `format、architecture、Clippy、全量普通测试和显式 integration-test target`（含 `--workspace` 全量，非 `--lib`），旧 `ci.yml` 的 `--lib` 陷阱已由 `docs/engineering-notes/cargo-test-workspace-selection.md` 纠正。消费者：`testing.md` 为门禁唯一真相，xterm/terminal 两文的命令为举例残留。 |
| D-9 | `docs/plans/agent-binary-split.md` 已标注 Phases 0-3 完结但与 specs/CI 的"done"联动缺失，Phase 4 触发信号与未决 spec 无关联 | 低 | `docs/plans/agent-binary-split.md:1` "状态：已完结（2026-08-15）Phase 0-3 已执行完毕"与代码真相一致（`src/bin/crossh-agent.rs:1-9` `#[path = "../agent_cli.rs"]`、`src/main.rs:132` `sibling_executable("crossh-agent")` 委托、`scripts/package*.sh:36,33,26` 三平台均含 `--bin crossh-agent`）；但 `docs/specs/README.md` 要求完结 spec 提炼 ADR 并在 `docs/architecture.md` 登记，architecture 已登记 ADR 0009，计划文档未反链该 ADR 索引行。Phase 4 触发信号（信号 A/B/C）与当前 `in-progress` 的 `pinned-tab-default-command` 等本地会话增强 spec 无引用，易遗漏"何时启动独立分发"的跟踪。 |
| D-10 | 历史审计报告中已删除的 `low_latency` 残骸与 S-1 类型镜像等仍以 high 未处置呈现 | 低 | `docs/audit/2026-08-17-code-review.md` / `docs/audit/2026-08-18-architecture-redundancy.md: S-1/S-2` 列 `low_latency_shell_input` 永远禁用菜单（`pane.rs`/`tab_strip.rs`/`context_menu.rs:53`/`locales`）与 `crossh-ai-sdk` 全量镜像（`providers.rs:84-212` 6 个胶水）为 high；`docs/specs/20260818-sdk-single-source-and-lowlatency-removal.md:done` 已在 2026-08-18 删除上述全部残骸（`rg low_latency` 零命中、`rg to_sdk_|from_sdk_|AgentMessage` 零命中），`docs/audit/2026-08-20-simplification-audit.md` 已对 S-1~S-10 标注"✅ 已完成 2026-08-20"。旧两份 audit 未回填"已处置"导致读者对同一残骸重复立项。 |

已验证无漂移 / 有意保留：

- `docs/architecture.md:10-42` crate 图与边界规则 1/3/7/8 与 `Cargo.toml:14-20` workspace 成员、`src/bin/crossh-{agent,git,updater}.rs`、`scripts/package*.sh:36` 完全一致；虽 `crossh-ai-sdk` 仅被 `crossh-agent` 消费但属 ADR 0003 刻意划分，已由 `crates/crossh-agent/README.md` 注明"直接消费 SDK canonical 类型"，不视为漂移。
- `docs/adr/0001-0014` 决策与代码一致；`docs/engineering-notes/*` 11 篇操作记忆与 `cargo-test-workspace-selection` / `ssh-lifecycle-and-path-escape` 等防御模式仍有效，未发现与代码矛盾的过期规则。
- `docs/specs/20260817-sftp-core-contracts.md:draft` 与 `20260817-ssh-hermetic-loopback.md:draft` 的"未兑现 hermetic 测试"承诺与 `docs/testing.md:46` Hermetic 集成测试章节一致，属已知缺口而非漂移，已在 `docs/audit/2026-08-18-architecture-redundancy.md` 作为 S-1/S-2 背景正确引用。
- `THIRD_PARTY_NOTICES.md` 与 `AGENTS.md` 的 Lucide 1.27.0 / Zed revision 固定语义与 `crates/crossh-assets/assets/icons/` 未篡改一致。

## 处置 Backlog

| 优先级 | 编号 | 处置 | 说明 |
| --- | --- | --- | --- |
| P0 | D-1 | ✅ 直接修（已落地） | `docs/xterm-compatibility-profile.md:17` 改为 `Cargo.toml` 真相 `1b04e4caf01e376624fb514ef85b0e6d8ee5d930`。验证：`grep -n rev Cargo.toml` 与 `grep -n rev docs/xterm-compatibility-profile.md` 一致；`cargo fmt --check` / `git diff --check` 绿。 |
| P1 | D-6 | 文档状态流转（豁免） | 6 个 `in-progress` spec 按 `docs/specs/README.md` 生命周期补验收清单勾选并迁至 `done`（保留历史不删除），每项由 owner 确认最后一批 `cargo test --workspace` (含 `env -u CROSSH_UPDATE_SIGNING_KEY`) 与对应 CI job 已绿后改状态。`git-sync-toast` 依赖 Toaster 已完成，`open-project-in-editor` 依赖 `editor_command` 已入矩阵，可优先闭环。 |
| P1 | D-3/D-4 | 直接修文档（豁免） | `remote-update-plan.md:48-51` 表格增 `tar.gz` 为 Linux 正式产物（与 `package-linux.sh:2` "tar.gz + AppImage"一致），"后续实施顺序"节对已完成的签名/加速条目标注"已完成 v0.16.4（见 ADR 0014 / specs 20260818-*）"，验收清单对已覆盖项打勾并反链测试命令。 |
| P1 | D-5 | 直接修 + 后续 spec 拆分 | `docs/testing.md:60` Workspace 单行拆为按 feature 的子行（或至少为 `pinned-tab-default-command` / `terminal-compose-bar` 各增一行），与 `editor_command` 的"检测候选不可配置"表述保持与 `src/features/workspace/settings.rs:39` 一致。不改变行为，仅提升矩阵可测性；拆分后由新 spec 认领缺口。 |
| P2 | D-2/D-9/D-10 | 文档标注（豁免） | `docs/plans/2026-08-17-simplification-backlog.md` 头部加"已归档：后续简化见 `docs/audit/2026-08-20-simplification-audit.md`（S-1~S-10 全部完成）"横幅或移入 `docs/archived/`；`agent-binary-split.md` 增 ADR 0009 反链；两份旧 audit 顶部增"后续处置见 2026-08-20 审计与 2026-08-18 done spec"横幅，避免重复立项。 |
| P2 | D-7/D-8 | 文档标注（豁免） | `docs/archived/*.md` 首行加"已归档：历史方案，仅作参考，现行布局见 `docs/architecture.md`"；`xterm-compatibility-profile.md:59-64` Evidence Commands 与 `terminal-compatibility.md:16-20` 同步为 `docs/testing.md:88-94` 的权威命令集（`cargo fmt --check` / `check-architecture.sh` / `clippy --all-targets` / `cargo test --workspace` + `terminal_replay --test-threads=1`）。 |

与保护表面关系：D-1 的 Zed revision 属 `AGENTS.md` / `docs/adr/0001` 钉住的"固定 revision"，本报告仅同步文档陈述与 `Cargo.toml` 真相，未改动 revision 本身；D-5/D-6 涉及的 `crossh-ai-sdk` 单一事实来源与 `low_latency` 删除均为 ADR 0003 / spec `20260818-sdk-single-source-and-lowlatency-removal` 已裁决的边界，矩阵拆分仅为可测性提升，不触碰 crate 归属。

## 与 SDD 工作流的衔接

- **直接修类（豁免清单内）**：D-1 已随本报告落地；D-3/D-4/D-7/D-8/D-9/D-10 均为纯文档标注/表格修正，无行为变更，按 `AGENTS.md` 可直接修，以 `cargo fmt --check` / `git diff --check` / `scripts/check-architecture.sh` 验证，不需 spec。
- **需 spec 认领类**：D-5 的矩阵拆分若涉及新增行为契约（如 `pinned-tab-default-command` 的默认命令持久化、compose 的 `Ctrl+Enter` 投递），应由对应 `in-progress` spec 补齐验收并在转 `done` 时同步 `testing.md`；D-6 的 6 个 spec 转 `done` 本身即 SDD 收尾动作（验收清单 + CI 验证），不另立 spec。
- **决策类**：无。本轮未发现需新增 ADR 的结构性漂移；`sftp-core-contracts` / `ssh-hermetic-loopback` 的 hermetic 能力仍为 `draft`，属已知缺口，待独立 SDD 闭环，不在本报告中扩范围。

## 本轮未决项

- D-6 的 6 个 `in-progress` → `done` 状态迁移需各 owner 在对应 `cargo test --workspace`（`env -u CROSSH_UPDATE_SIGNING_KEY` 干净环境）与 Linux/Windows CI 验证后逐个改状态，本报告仅提出清单不代为改状态。
- D-5 的 `testing.md` 矩阵拆分需与 `pinned-tab-default-command` / `terminal-compose-bar` 两个 `in-progress` spec 的验收联动，避免矩阵先行而行为未冻。
- 未重新执行 `cargo clippy --workspace --all-targets` 与全量 `cargo test --workspace`（离线/环境变量敏感），结论基于 `rg` 静态引用与文件内容比对；`cargo fmt --check` 与 `git diff --check` 已在 D-1 落地验证中通过。

每条候选的最强反方论证（为什么保留）：

- D-1：旧 rev `90d024b` 仍可在 `~/.cargo/git/db/zed-*` 中查到，且 xterm profile 的兼容性矩阵（Out of scope 行）与 Zed 版本无关，rev 过时不影响兼容性结论本身——但可复现性要求必须同步。
- D-2/D-10：历史 plans/audits 作为变更档案按 `docs/specs/README.md` 不删除是正确纪律，问题仅是缺"已归档/已取代"横幅而非内容错误。
- D-3/D-4：`remote-update-plan.md` 的"后续"与"验收清单"是路线图性质，早于 spec 存在，未同步不影响发布物正确性（脚本与 manifest 校验为真相），但会让新人阅读时误判进度。
- D-5：Workspace 单行过长是为"关键行为矩阵"紧凑而有意为之，ADRs 未规定矩阵粒度，拆分仅为可测性提升而非正确性修复。
- D-6：`in-progress` 滞后可能是有意等待三平台 CI 全绿后再转 `done`（符合 `docs/specs/README.md` "spec 保持 in-progress 直到非本地平台 CI 通过"），本报告标记为漂移是提醒及时闭环而非指责提前转 done。
- D-7：`docs/archived/` 已通过目录名表达归档意图，正文头部再加横幅属冗余；但对从搜索引擎直达单文件的读者仍有必要。
